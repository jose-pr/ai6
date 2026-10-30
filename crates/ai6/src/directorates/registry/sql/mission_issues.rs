//! Linked issues and the outbox in SQL: the shared logic of the file Registry, run in one
//! write transaction on two tables.

use std::io;

use super::db::Tx;
use super::{SqlRegistry, from_json, to_json};
use crate::directorates::registry::mission_issues::{
    IssueLink, IssueTables, LinkState, MissionIssueStore, OutboxEntry, OutboxSettle, OutboxState,
    adopt_in, begin_in, due_in, link_issue_in, queue_in, set_status_in, settle_in, unlink_in,
};

/// The two tables, inside one transaction at `revision`.
struct SqlTables<'a, 'b> {
    tx: &'a mut Tx<'b>,
    revision: i64,
}

impl IssueTables for SqlTables<'_, '_> {
    fn links(&mut self, mission: &str) -> io::Result<Vec<IssueLink>> {
        self.tx
            .query_opt(
                "SELECT links FROM ai6_mission_issues WHERE mission_id = $1",
                &[mission.into()],
            )?
            .map(|row| from_json(&row.text(0)?, "linked issues"))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn all_links(&mut self) -> io::Result<Vec<IssueLink>> {
        let rows = self.tx.query("SELECT links FROM ai6_mission_issues", &[])?;
        let mut all = Vec::new();
        for row in rows {
            if let Ok(links) = from_json::<Vec<IssueLink>>(&row.text(0)?, "linked issues") {
                all.extend(links);
            }
        }
        Ok(all)
    }

    fn put_links(&mut self, mission: &str, links: &[IssueLink]) -> io::Result<()> {
        self.tx.execute(
            "INSERT INTO ai6_mission_issues (mission_id, links, revision) VALUES ($1, $2, $3) \
             ON CONFLICT (mission_id) DO UPDATE SET links = excluded.links, \
             revision = excluded.revision",
            &[
                mission.into(),
                to_json(&links)?.into(),
                self.revision.into(),
            ],
        )?;
        Ok(())
    }

    fn outbox(&mut self) -> io::Result<Vec<OutboxEntry>> {
        let rows = self.tx.query("SELECT record FROM ai6_issue_outbox", &[])?;
        let mut all = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(entry) = from_json::<OutboxEntry>(&row.text(0)?, "outbox entry") {
                all.push(entry);
            }
        }
        all.sort_by(|a, b| a.at.total_cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        Ok(all)
    }

    fn put_entry(&mut self, entry: &OutboxEntry) -> io::Result<()> {
        let state = serde_json::to_value(entry.state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        self.tx.execute(
            "INSERT INTO ai6_issue_outbox (id, state, record, revision) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (id) DO UPDATE SET state = excluded.state, record = excluded.record, \
             revision = excluded.revision",
            &[
                entry.id.as_str().into(),
                state.into(),
                to_json(entry)?.into(),
                self.revision.into(),
            ],
        )?;
        Ok(())
    }

    fn drop_entry(&mut self, id: &str) -> io::Result<()> {
        self.tx
            .execute("DELETE FROM ai6_issue_outbox WHERE id = $1", &[id.into()])?;
        Ok(())
    }
}

impl MissionIssueStore for SqlRegistry {
    fn link_issue(&self, link: &IssueLink) -> io::Result<IssueLink> {
        self.write(|tx, revision| {
            link_issue_in(&mut SqlTables { tx, revision }, link, &self.limits.issues)
        })
    }

    fn issue_links(&self, mission_id: &str) -> io::Result<Vec<IssueLink>> {
        self.read(|tx| SqlTables { tx, revision: 0 }.links(mission_id))
    }

    fn active_issue_links(&self) -> io::Result<Vec<IssueLink>> {
        Ok(self
            .read(|tx| SqlTables { tx, revision: 0 }.all_links())?
            .into_iter()
            .filter(|link| link.state == LinkState::Active)
            .collect())
    }

    fn unlink_issue(&self, mission_id: &str, tracker: &str, item: &str) -> io::Result<bool> {
        self.write(|tx, revision| {
            unlink_in(&mut SqlTables { tx, revision }, mission_id, tracker, item)
        })
    }

    fn set_issue_status(
        &self,
        mission_id: &str,
        tracker: &str,
        item: &str,
        comment: Option<&str>,
        phase: Option<&str>,
    ) -> io::Result<()> {
        self.write(|tx, revision| {
            set_status_in(
                &mut SqlTables { tx, revision },
                mission_id,
                tracker,
                item,
                comment,
                phase,
            )
        })
    }

    fn queue_outbox(&self, entry: &OutboxEntry) -> io::Result<OutboxEntry> {
        self.write(|tx, revision| {
            queue_in(&mut SqlTables { tx, revision }, entry, &self.limits.issues)
        })
    }

    fn outbox_entry(&self, id: &str) -> io::Result<Option<OutboxEntry>> {
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_issue_outbox WHERE id = $1",
                &[id.into()],
            )?
            .map(|row| from_json(&row.text(0)?, "outbox entry"))
            .transpose()
        })
    }

    fn outbox_entries(&self, mission_id: Option<&str>) -> io::Result<Vec<OutboxEntry>> {
        Ok(self
            .read(|tx| SqlTables { tx, revision: 0 }.outbox())?
            .into_iter()
            .filter(|entry| mission_id.is_none_or(|mission| entry.mission_id == mission))
            .collect())
    }

    fn due_outbox(&self, now: f64, limit: usize) -> io::Result<Vec<OutboxEntry>> {
        self.read(|tx| due_in(&mut SqlTables { tx, revision: 0 }, now, limit))
    }

    fn begin_outbox_send(&self, id: &str, term: u64, now: f64) -> io::Result<Option<OutboxEntry>> {
        self.write(|tx, revision| begin_in(&mut SqlTables { tx, revision }, id, term, now))
    }

    fn sending_outbox(&self) -> io::Result<Vec<OutboxEntry>> {
        Ok(self
            .read(|tx| SqlTables { tx, revision: 0 }.outbox())?
            .into_iter()
            .filter(|entry| entry.state == OutboxState::Sending)
            .collect())
    }

    fn adopt_outbox_send(&self, id: &str, term: u64) -> io::Result<Option<OutboxEntry>> {
        self.write(|tx, revision| adopt_in(&mut SqlTables { tx, revision }, id, term))
    }

    fn settle_outbox(&self, id: &str, term: u64, settle: OutboxSettle) -> io::Result<OutboxEntry> {
        self.write(|tx, revision| settle_in(&mut SqlTables { tx, revision }, id, term, settle))
    }
}
