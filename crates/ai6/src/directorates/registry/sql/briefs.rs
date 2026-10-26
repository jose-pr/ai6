//! The brief queue, Mission proposals and Source briefs in `ai6_briefs`,
//! `ai6_brief_leases` and `ai6_mission_proposals`.

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use super::db::Tx;
use super::{SqlRegistry, bounded_json, from_json, to_json};
use crate::directorates::registry::bona_fides::{self, Provenance, proposal_provenance};
use crate::directorates::registry::sources::{
    FinishedSourceBrief, Ingest, MAX_REPORT_ATTEMPTS, SOURCE_BRIEF_PREFIX, SourceItem, SourceStore,
    is_source_key, refresh_queued, source_brief, source_provenance,
};
use crate::directorates::registry::store::{
    Brief, BriefStore, Lease, LeaseRecovery, MAX_BRIEF_BYTES, MAX_BRIEFS,
    MAX_MISSION_PROPOSAL_BYTES, MAX_MISSION_PROPOSAL_RESULTS, MAX_MISSION_PROPOSALS,
    MissionProposal, invalid_data, lost_outcome, unix_time, validate_registry_key,
};

/// The brief record, without its provenance.
fn brief_json(brief: &Brief) -> io::Result<String> {
    let (record, _) = bona_fides::seal(brief)?;
    bounded_json(&record, MAX_BRIEF_BYTES, "brief")
}

/// The `provenance` column for `brief`.
fn provenance_json(brief: &Brief) -> io::Result<Option<String>> {
    bona_fides::seal(brief)?
        .1
        .map(|provenance| to_json(&provenance))
        .transpose()
}

/// A stored brief and its `provenance` column; a record's own `provenance` is never read.
fn stored_brief(record: &str, provenance: Option<String>) -> io::Result<Brief> {
    let mut brief: Brief = from_json(record, "brief")?;
    let kept = provenance.and_then(|text| serde_json::from_str::<Provenance>(&text).ok());
    bona_fides::unseal(&mut brief, kept);
    Ok(brief)
}

/// The queued briefs in Registry order: priority, creation time, key.
fn queued(tx: &mut Tx<'_>) -> io::Result<Vec<Brief>> {
    let rows = tx.query(
        "SELECT brief, provenance FROM ai6_briefs WHERE state = 'new' LIMIT $1",
        &[(MAX_BRIEFS + 1).into()],
    )?;
    if rows.len() > MAX_BRIEFS {
        return Err(invalid_data(format!(
            "Registry has more than {MAX_BRIEFS} queued briefs"
        )));
    }
    let mut briefs = Vec::with_capacity(rows.len());
    for row in rows {
        if let Ok(brief) = stored_brief(&row.text(0)?, row.opt_text(1)?)
            && brief.validate().is_ok()
        {
            briefs.push(brief);
        }
    }
    briefs.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.created.cmp(&right.created))
            .then_with(|| left.key().cmp(&right.key()))
    });
    Ok(briefs)
}

fn brief_state(tx: &mut Tx<'_>, key: &str) -> io::Result<Option<String>> {
    tx.query_opt("SELECT state FROM ai6_briefs WHERE key = $1", &[key.into()])?
        .map(|row| row.text(0))
        .transpose()
}

fn insert_brief(tx: &mut Tx<'_>, brief: &Brief, revision: i64) -> io::Result<()> {
    tx.execute(
        "INSERT INTO ai6_briefs (key, state, brief, provenance, revision) \
         VALUES ($1, 'new', $2, $3, $4)",
        &[
            brief.key().into(),
            brief_json(brief)?.into(),
            provenance_json(brief)?.into(),
            revision.into(),
        ],
    )?;
    Ok(())
}

fn lease_in(tx: &mut Tx<'_>, key: &str) -> io::Result<Option<Lease>> {
    let Some(row) = tx.query_opt(
        "SELECT station, expires, attempt, meta FROM ai6_brief_leases WHERE key = $1",
        &[key.into()],
    )?
    else {
        return Ok(None);
    };
    let station = row.text(0)?;
    let expires = row.real(1)?;
    if station.trim().is_empty() || !expires.is_finite() {
        return Err(invalid_data("invalid Registry lease"));
    }
    Ok(Some(Lease {
        key: key.to_owned(),
        station,
        expires,
        attempt: u32::try_from(row.int(2)?)
            .map_err(|_| invalid_data("invalid Registry attempt count"))?,
        meta: from_json(&row.text(3)?, "Registry lease")?,
    }))
}

fn put_lease(tx: &mut Tx<'_>, lease: &Lease, revision: i64) -> io::Result<()> {
    tx.execute(
        "INSERT INTO ai6_brief_leases (key, station, expires, attempt, meta, revision) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (key) DO UPDATE SET station = excluded.station, expires = excluded.expires, \
         attempt = excluded.attempt, meta = excluded.meta, revision = excluded.revision",
        &[
            lease.key.as_str().into(),
            lease.station.as_str().into(),
            lease.expires.into(),
            lease.attempt.into(),
            to_json(&lease.meta)?.into(),
            revision.into(),
        ],
    )?;
    Ok(())
}

/// The leases of claimed briefs, in key order.
fn leases_in(tx: &mut Tx<'_>) -> io::Result<Vec<Lease>> {
    let keys = tx.query(
        "SELECT l.key FROM ai6_brief_leases l JOIN ai6_briefs b ON b.key = l.key \
         WHERE b.state = 'claimed' ORDER BY l.key",
        &[],
    )?;
    let mut leases = Vec::with_capacity(keys.len());
    for row in keys {
        if let Some(lease) = lease_in(tx, &row.text(0)?)? {
            leases.push(lease);
        }
    }
    Ok(leases)
}

fn is_claimed(tx: &mut Tx<'_>, key: &str) -> io::Result<bool> {
    Ok(brief_state(tx, key)?.as_deref() == Some("claimed"))
}

fn check_lease_request(station: &str, duration: Duration) -> io::Result<()> {
    if station.trim().is_empty() || duration.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a Registry lease needs a station and positive duration",
        ));
    }
    Ok(())
}

/// Files `brief` as done or failed with its outcome, keeping the claimed copy when there is
/// one, and drops its lease.
fn finish_in(
    tx: &mut Tx<'_>,
    brief: &Brief,
    outcome: &serde_json::Value,
    ok: bool,
    transcript: Option<&[u8]>,
    revision: i64,
) -> io::Result<()> {
    let state = if ok { "done" } else { "failed" };
    let key = brief.key();
    let outcome = to_json(outcome)?;
    if is_claimed(tx, &key)? {
        // The provenance is spent: a row queued again under this key finds none.
        tx.execute(
            "UPDATE ai6_briefs SET state = $2, outcome = $3, \
             transcript = coalesce($4, transcript), revision = $5, \
             finished_provenance = provenance, provenance = NULL WHERE key = $1",
            &[
                key.as_str().into(),
                state.into(),
                outcome.into(),
                transcript.into(),
                revision.into(),
            ],
        )?;
    } else {
        tx.execute(
            "INSERT INTO ai6_briefs \
             (key, state, brief, outcome, transcript, revision, finished_provenance) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (key) DO UPDATE SET state = excluded.state, brief = excluded.brief, \
             outcome = excluded.outcome, \
             transcript = coalesce(excluded.transcript, ai6_briefs.transcript), \
             revision = excluded.revision, \
             finished_provenance = coalesce(ai6_briefs.provenance, excluded.finished_provenance), \
             provenance = NULL",
            &[
                key.as_str().into(),
                state.into(),
                brief_json(brief)?.into(),
                outcome.into(),
                transcript.into(),
                revision.into(),
                provenance_json(brief)?.into(),
            ],
        )?;
    }
    tx.execute("DELETE FROM ai6_brief_leases WHERE key = $1", &[key.into()])?;
    Ok(())
}

fn proposal(
    tx: &mut Tx<'_>,
    operation: &str,
    id: &str,
) -> io::Result<Option<(String, MissionProposal)>> {
    let Some(row) = tx.query_opt(
        "SELECT state, proposal FROM ai6_mission_proposals WHERE operation = $1 AND id = $2",
        &[operation.into(), id.into()],
    )?
    else {
        return Ok(None);
    };
    let proposal: MissionProposal = from_json(&row.text(1)?, "Mission proposal")?;
    proposal
        .validate()
        .map_err(|error| invalid_data(error.to_string()))?;
    if proposal.operation_id != operation || proposal.id != id {
        return Err(invalid_data(
            "Mission proposal record does not match its identity",
        ));
    }
    Ok(Some((row.text(0)?, proposal)))
}

fn set_proposal_state(
    tx: &mut Tx<'_>,
    operation: &str,
    id: &str,
    state: &str,
    revision: i64,
) -> io::Result<()> {
    tx.execute(
        "UPDATE ai6_mission_proposals SET state = $3, revision = $4 WHERE operation = $1 AND id = $2",
        &[operation.into(), id.into(), state.into(), revision.into()],
    )?;
    Ok(())
}

#[cfg(test)]
impl SqlRegistry {
    pub(crate) fn drop_brief(&self, key: &str, record: &str) {
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_briefs (key, state, brief, revision) VALUES ($1, 'new', $2, $3) \
                 ON CONFLICT (key) DO UPDATE SET state = 'new', brief = excluded.brief",
                &[key.into(), record.into(), revision.into()],
            )
            .map(drop)
        })
        .unwrap();
    }
}

impl BriefStore for SqlRegistry {
    fn submit_mission_proposal(&self, proposal: &MissionProposal) -> io::Result<()> {
        proposal.validate()?;
        self.write(|tx, revision| {
            let mut stored = proposal.clone();
            if let Some((_, existing)) = self::proposal(tx, &proposal.operation_id, &proposal.id)? {
                stored.created_at = existing.created_at;
                if existing == stored {
                    return Ok(());
                }
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Mission proposal identity already has different content",
                ));
            }
            let pending = tx
                .query_opt(
                    "SELECT count(*) FROM ai6_mission_proposals WHERE operation = $1 AND state = 'pending'",
                    &[proposal.operation_id.as_str().into()],
                )?
                .map_or(Ok(0), |row| row.int(0))?;
            if pending >= MAX_MISSION_PROPOSALS as i64 {
                return Err(invalid_data(format!(
                    "Operation has reached the {MAX_MISSION_PROPOSALS} pending Mission proposal limit"
                )));
            }
            stored.created_at = unix_time()?;
            let text = bounded_json(&stored, MAX_MISSION_PROPOSAL_BYTES, "Mission proposal")?;
            tx.execute(
                "INSERT INTO ai6_mission_proposals (operation, id, state, proposal, created_at, revision) \
                 VALUES ($1, $2, 'pending', $3, $4, $5)",
                &[
                    stored.operation_id.as_str().into(),
                    stored.id.as_str().into(),
                    text.into(),
                    stored.created_at.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn list_mission_proposals(
        &self,
        operation: &str,
        limit: usize,
    ) -> io::Result<Vec<MissionProposal>> {
        validate_registry_key(operation)?;
        if limit == 0 || limit > MAX_MISSION_PROPOSAL_RESULTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Mission proposal result limit must be between 1 and {MAX_MISSION_PROPOSAL_RESULTS}"
                ),
            ));
        }
        self.read(|tx| {
            let rows = tx.query(
                "SELECT proposal FROM ai6_mission_proposals \
                 WHERE operation = $1 AND state = 'pending' ORDER BY created_at, id LIMIT $2",
                &[operation.into(), limit.into()],
            )?;
            let mut proposals = Vec::with_capacity(rows.len());
            for row in rows {
                if let Ok(proposal) = serde_json::from_str::<MissionProposal>(&row.text(0)?)
                    && proposal.validate().is_ok()
                    && proposal.operation_id == operation
                {
                    proposals.push(proposal);
                }
            }
            Ok(proposals)
        })
    }

    fn promote_mission_proposal(
        &self,
        operation: &str,
        proposal_id: &str,
        received_by: &str,
    ) -> io::Result<Brief> {
        validate_registry_key(operation)?;
        validate_registry_key(proposal_id)?;
        self.write(|tx, revision| {
            let (state, proposal) = proposal(tx, operation, proposal_id)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Mission proposal does not exist")
            })?;
            let stored = |tx: &mut Tx<'_>, key: String| -> io::Result<Option<Brief>> {
                tx.query_opt(
                    "SELECT brief, provenance FROM ai6_briefs WHERE key = $1",
                    &[key.into()],
                )?
                .map(|row| stored_brief(&row.text(0)?, row.opt_text(1)?))
                .transpose()
            };
            match state.as_str() {
                "approved" => {
                    let key = proposal.brief.key();
                    return Ok(stored(tx, key)?.unwrap_or(proposal.brief));
                }
                "rejected" => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Mission proposal has already been rejected",
                    ));
                }
                _ => {}
            }
            let mut brief = proposal.brief.clone();
            let key = brief.key();
            match brief_state(tx, &key)?.as_deref() {
                Some("new") => {
                    let queued =
                        stored(tx, key)?.ok_or_else(|| invalid_data("queued brief vanished"))?;
                    let record = Brief {
                        provenance: None,
                        ..queued.clone()
                    };
                    if record != brief {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "Mission proposal brief conflicts with queued intake",
                        ));
                    }
                    brief = queued;
                }
                Some(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Mission proposal brief already exists in a terminal intake state",
                    ));
                }
                None => {
                    brief.provenance = Some(proposal_provenance(&proposal, received_by));
                    insert_brief(tx, &brief, revision)?;
                }
            }
            set_proposal_state(tx, operation, proposal_id, "approved", revision)?;
            Ok(brief)
        })
    }

    fn reject_mission_proposal(
        &self,
        operation: &str,
        proposal_id: &str,
    ) -> io::Result<MissionProposal> {
        validate_registry_key(operation)?;
        validate_registry_key(proposal_id)?;
        self.write(|tx, revision| {
            let (state, proposal) = proposal(tx, operation, proposal_id)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Mission proposal does not exist")
            })?;
            match state.as_str() {
                "rejected" => Ok(proposal),
                "approved" => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Mission proposal has already been approved",
                )),
                _ => {
                    set_proposal_state(tx, operation, proposal_id, "rejected", revision)?;
                    Ok(proposal)
                }
            }
        })
    }

    fn list_new(&self) -> io::Result<Vec<Brief>> {
        self.read(queued)
    }

    fn submit_brief(&self, brief: &Brief) -> io::Result<()> {
        brief.validate()?;
        let key = brief.key();
        self.write(|tx, revision| {
            if let Some(state) = brief_state(tx, &key)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("brief {key} already exists in {state}"),
                ));
            }
            insert_brief(tx, brief, revision)
        })
    }

    fn claim_new(&self, key: Option<&str>) -> io::Result<Option<Brief>> {
        self.write(|tx, revision| {
            for brief in queued(tx)? {
                if key.is_some_and(|key| brief.key() != key) {
                    continue;
                }
                tx.execute(
                    "UPDATE ai6_briefs SET state = 'claimed', revision = $2 WHERE key = $1",
                    &[brief.key().into(), revision.into()],
                )?;
                return Ok(Some(brief));
            }
            Ok(None)
        })
    }

    fn claim_with_lease(
        &self,
        key: Option<&str>,
        station: &str,
        duration: Duration,
        meta: BTreeMap<String, serde_json::Value>,
    ) -> io::Result<Option<Lease>> {
        check_lease_request(station, duration)?;
        self.write(|tx, revision| {
            let Some(brief) = queued(tx)?
                .into_iter()
                .find(|brief| key.is_none_or(|key| brief.key() == key))
            else {
                return Ok(None);
            };
            let key = brief.key();
            let previous = tx
                .query_opt("SELECT attempts FROM ai6_briefs WHERE key = $1", &[key.as_str().into()])?
                .map_or(Ok(0), |row| row.int(0))?;
            let attempt = u32::try_from(previous)
                .ok()
                .and_then(|previous| previous.checked_add(1))
                .ok_or_else(|| invalid_data("Registry attempt count is exhausted"))?;
            tx.execute(
                "UPDATE ai6_briefs SET state = 'claimed', attempts = $2, revision = $3 WHERE key = $1",
                &[key.as_str().into(), attempt.into(), revision.into()],
            )?;
            let lease = Lease {
                key,
                station: station.to_owned(),
                expires: tx.now()? + duration.as_secs_f64(),
                attempt,
                meta,
            };
            put_lease(tx, &lease, revision)?;
            Ok(Some(lease))
        })
    }

    fn lease(&self, key: &str) -> io::Result<Option<Lease>> {
        validate_registry_key(key)?;
        self.read(|tx| lease_in(tx, key))
    }

    fn read_claimed(&self, key: &str) -> io::Result<Option<Brief>> {
        validate_registry_key(key)?;
        self.read(|tx| {
            let Some(row) = tx.query_opt(
                "SELECT brief, provenance FROM ai6_briefs WHERE key = $1 AND state = 'claimed'",
                &[key.into()],
            )?
            else {
                return Ok(None);
            };
            let brief = stored_brief(&row.text(0)?, row.opt_text(1)?)?;
            brief.validate()?;
            Ok(Some(brief))
        })
    }

    fn leases(&self) -> io::Result<Vec<Lease>> {
        self.read(leases_in)
    }

    fn renew(&self, key: &str, station: &str, duration: Duration) -> io::Result<bool> {
        validate_registry_key(key)?;
        check_lease_request(station, duration)?;
        self.write(|tx, revision| {
            let Some(mut lease) = lease_in(tx, key)? else {
                return Ok(false);
            };
            if lease.station != station || !is_claimed(tx, key)? {
                return Ok(false);
            }
            lease.expires = tx.now()? + duration.as_secs_f64();
            put_lease(tx, &lease, revision)?;
            Ok(true)
        })
    }

    fn expired(&self) -> io::Result<Vec<Lease>> {
        self.read(|tx| {
            let now = tx.now()?;
            Ok(leases_in(tx)?
                .into_iter()
                .filter(|lease| lease.expires < now)
                .collect())
        })
    }

    fn recover(
        &self,
        lease: &Lease,
        max_attempts: u32,
        by: &str,
    ) -> io::Result<Option<LeaseRecovery>> {
        validate_registry_key(lease.key())?;
        self.write(|tx, revision| {
            if lease_in(tx, lease.key())?.as_ref() != Some(lease) || lease.expires >= tx.now()? {
                return Ok(None);
            }
            if !is_claimed(tx, lease.key())? {
                return Ok(None);
            }
            if lease.attempt() < max_attempts {
                tx.execute(
                    "UPDATE ai6_briefs SET state = 'new', revision = $2 WHERE key = $1",
                    &[lease.key().into(), revision.into()],
                )?;
                tx.execute(
                    "DELETE FROM ai6_brief_leases WHERE key = $1",
                    &[lease.key().into()],
                )?;
                return Ok(Some(LeaseRecovery::Requeued));
            }
            let row = tx
                .query_opt(
                    "SELECT brief FROM ai6_briefs WHERE key = $1",
                    &[lease.key().into()],
                )?
                .ok_or_else(|| invalid_data("claimed brief vanished"))?;
            let brief = match serde_json::from_str::<Brief>(&row.text(0)?) {
                Ok(brief) if brief.validate().is_ok() => brief,
                _ => return Ok(None),
            };
            finish_in(tx, &brief, &lost_outcome(lease, by), false, None, revision)?;
            Ok(Some(LeaseRecovery::Failed))
        })
    }

    fn hand_over(
        &self,
        lease: &Lease,
        station: &str,
        duration: Duration,
        meta: BTreeMap<String, serde_json::Value>,
    ) -> io::Result<Option<Lease>> {
        validate_registry_key(lease.key())?;
        check_lease_request(station, duration)?;
        self.write(|tx, revision| {
            let now = tx.now()?;
            if lease_in(tx, lease.key())?.as_ref() != Some(lease)
                || lease.expires >= now
                || !is_claimed(tx, lease.key())?
            {
                return Ok(None);
            }
            let mut handed = lease.clone();
            handed.station = station.to_owned();
            handed.expires = now + duration.as_secs_f64();
            handed.meta.extend(meta);
            put_lease(tx, &handed, revision)?;
            Ok(Some(handed))
        })
    }

    fn finish(
        &self,
        brief: &Brief,
        outcome: &serde_json::Value,
        ok: bool,
        transcript: Option<&[u8]>,
        station: Option<&str>,
    ) -> io::Result<bool> {
        if !outcome.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Registry outcomes must be JSON objects",
            ));
        }
        self.write(|tx, revision| {
            if let Some(station) = station {
                let key = brief.key();
                let held = lease_in(tx, &key)?.is_some_and(|lease| lease.station() == station);
                if !held || !is_claimed(tx, &key)? {
                    return Ok(false);
                }
            }
            finish_in(tx, brief, outcome, ok, transcript, revision)?;
            Ok(true)
        })
    }
}

impl SourceStore for SqlRegistry {
    fn source_cursor(&self, operation: &str, provider: &str) -> io::Result<Option<String>> {
        self.read(|tx| {
            tx.query_opt(
                "SELECT cursor FROM ai6_source_cursors WHERE operation = $1 AND provider = $2",
                &[operation.into(), provider.into()],
            )?
            .map(|row| row.text(0))
            .transpose()
        })
    }

    fn set_source_cursor(&self, operation: &str, provider: &str, cursor: &str) -> io::Result<()> {
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_source_cursors (operation, provider, cursor, revision) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (operation, provider) \
                 DO UPDATE SET cursor = excluded.cursor, revision = excluded.revision",
                &[
                    operation.into(),
                    provider.into(),
                    cursor.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn ingest_source_item(
        &self,
        operation: &str,
        provider: &str,
        item: &SourceItem,
        received_by: &str,
    ) -> io::Result<Ingest> {
        let mut brief = source_brief(operation, provider, item);
        brief.validate()?;
        brief.provenance = Some(source_provenance(provider, item, &brief, received_by));
        let key = brief.key();
        self.write(|tx, revision| {
            let Some(row) = tx.query_opt(
                "SELECT state, brief FROM ai6_briefs WHERE key = $1",
                &[key.as_str().into()],
            )?
            else {
                insert_brief(tx, &brief, revision)?;
                return Ok(Ingest::Queued);
            };
            match row.text(0)?.as_str() {
                "done" | "failed" => return Ok(Ingest::Finished),
                "claimed" => return Ok(Ingest::InFlight),
                _ => {}
            }
            let outcome = refresh_queued(&mut brief, from_json(&row.text(1)?, "brief")?);
            if outcome == Ingest::Updated {
                tx.execute(
                    "UPDATE ai6_briefs SET brief = $2, provenance = $3, revision = $4 WHERE key = $1",
                    &[
                        key.as_str().into(),
                        brief_json(&brief)?.into(),
                        provenance_json(&brief)?.into(),
                        revision.into(),
                    ],
                )?;
            }
            Ok(outcome)
        })
    }

    fn unreported_source_briefs(&self) -> io::Result<Vec<FinishedSourceBrief>> {
        self.read(|tx| {
            let rows = tx.query(
                "SELECT key, state, brief, outcome, finished_provenance FROM ai6_briefs \
                 WHERE state IN ('done', 'failed') AND reported = 0 AND report_attempts < $1 \
                 AND key LIKE 'source!_%' ESCAPE '!' ORDER BY key LIMIT $2",
                &[MAX_REPORT_ATTEMPTS.into(), (MAX_BRIEFS * 8).into()],
            )?;
            let mut finished = Vec::new();
            for row in rows {
                if !is_source_key(&row.text(0)?) {
                    continue;
                }
                let Ok(brief) = stored_brief(&row.text(2)?, row.opt_text(4)?) else {
                    continue;
                };
                if !brief.source.starts_with(SOURCE_BRIEF_PREFIX) {
                    continue;
                }
                let outcome = row
                    .opt_text(3)?
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                finished.push(FinishedSourceBrief {
                    brief,
                    outcome,
                    ok: row.text(1)? == "done",
                });
            }
            finished.sort_by_key(|entry| entry.brief.key());
            Ok(finished)
        })
    }

    fn mark_source_reported(
        &self,
        finished: &FinishedSourceBrief,
        reported: bool,
    ) -> io::Result<()> {
        self.write(|tx, revision| {
            tx.execute(
                "UPDATE ai6_briefs SET report_attempts = report_attempts + 1, reported = $2, \
                 revision = $3 WHERE key = $1",
                &[
                    finished.brief.key().into(),
                    reported.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }
}
