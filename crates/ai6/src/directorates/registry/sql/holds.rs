//! Holds in SQL: one row per Agent, Operation and route, each holding its record as JSON.

use std::io;

use super::SqlRegistry;
use crate::directorates::m::roster::RouteRef;
use crate::directorates::registry::holds::{HoldStore, RosterHold, hold_bytes, parse_hold};
use crate::directorates::registry::store::{invalid_data, validate_registry_key};

impl HoldStore for SqlRegistry {
    fn put_hold(&self, hold: &RosterHold) -> io::Result<()> {
        hold.validate(&self.limits.holds)?;
        let text =
            String::from_utf8(hold_bytes(hold, &self.limits.holds)?).map_err(io::Error::other)?;
        let route = RosterHold::route_key(hold.route.as_ref());
        self.write(|tx, revision| {
            let exists = tx
                .query_opt(
                    "SELECT agent FROM ai6_roster_holds \
                     WHERE operation = $1 AND agent = $2 AND route = $3",
                    &[
                        hold.operation.as_str().into(),
                        hold.agent.as_str().into(),
                        route.as_str().into(),
                    ],
                )?
                .is_some();
            if !exists {
                let count = tx
                    .query_opt("SELECT count(*) FROM ai6_roster_holds", &[])?
                    .map_or(Ok(0), |row| row.int(0))?;
                if !self.limits.holds.holds.allows(count + 1) {
                    return Err(invalid_data("too many holds"));
                }
            }
            tx.execute(
                "INSERT INTO ai6_roster_holds (operation, agent, route, record, revision) \
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (operation, agent, route) \
                 DO UPDATE SET record = excluded.record, revision = excluded.revision",
                &[
                    hold.operation.as_str().into(),
                    hold.agent.as_str().into(),
                    route.as_str().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn holds(&self, operation: &str) -> io::Result<Vec<RosterHold>> {
        validate_registry_key(operation)?;
        let rows = self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_roster_holds WHERE operation = $1",
                &[operation.into()],
            )
        })?;
        let mut holds = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(hold) = parse_hold(row.text(0)?.as_bytes(), &self.limits.holds) {
                holds.push(hold);
            }
        }
        holds.sort_by(|left, right| {
            left.agent.cmp(&right.agent).then_with(|| {
                RosterHold::route_key(left.route.as_ref())
                    .cmp(&RosterHold::route_key(right.route.as_ref()))
            })
        });
        Ok(holds)
    }

    fn lift_hold(
        &self,
        operation: &str,
        agent: &str,
        route: Option<&RouteRef>,
    ) -> io::Result<Option<RosterHold>> {
        validate_registry_key(operation)?;
        validate_registry_key(agent)?;
        let route = RosterHold::route_key(route);
        self.write(|tx, _| {
            let Some(row) = tx.query_opt(
                "SELECT record FROM ai6_roster_holds \
                 WHERE operation = $1 AND agent = $2 AND route = $3",
                &[operation.into(), agent.into(), route.as_str().into()],
            )?
            else {
                return Ok(None);
            };
            let hold = parse_hold(row.text(0)?.as_bytes(), &self.limits.holds)?;
            tx.execute(
                "DELETE FROM ai6_roster_holds WHERE operation = $1 AND agent = $2 AND route = $3",
                &[operation.into(), agent.into(), route.as_str().into()],
            )?;
            Ok(Some(hold))
        })
    }
}
