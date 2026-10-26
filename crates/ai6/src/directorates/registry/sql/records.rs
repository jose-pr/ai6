//! The smaller record families: access tokens, audit, Agent roles, Cables and their
//! quotas, quarantines, Sleeper authorizations, the timeline and the usage ledger.

use std::io;

use super::db::Tx;
use super::{SqlRegistry, bounded_json, from_json, to_json};
use crate::directorates::m::roster::AgentRole;
use crate::directorates::medical::quarantine::Incident;
use crate::directorates::medical::usage::{MissionUsage, UsageTotals};
use crate::directorates::registry::cables::{Cable, CableState};
use crate::directorates::registry::quarantine::{
    MAX_QUARANTINES, MAX_RECORD_BYTES as MAX_QUARANTINE_BYTES, Quarantine, QuarantineStore,
    add_incident, quarantine_report, validate_quarantine,
};
use crate::directorates::registry::sleepers::{
    Fire, Sleeper, SleeperAuthorization, SleeperStore, fired_brief,
};
use crate::directorates::registry::store::{
    AccessTokenStore, AgentRoleStore, AuditRecord, AuditStore, BriefStore, CableStore,
    MAX_ACCESS_TOKEN_RECORD_BYTES, MAX_AGENT_ROLE_RECORD_BYTES, MAX_AGENT_ROLES, MAX_AUDIT_ENTRIES,
    MAX_AUDIT_RECORD_BYTES, MAX_AUDIT_RESULTS, MAX_CABLE_FILES, MAX_CABLE_RECORD_BYTES,
    cable_error, cable_sent_event, check_quota_buckets, invalid_data, quota_allows, quota_balances,
    unix_time, validate_access_token_record, validate_cable_id, validate_registry_key,
    validate_token_digest,
};
use crate::directorates::registry::timeline::{MAX_TIMELINE_EVENTS, TimelineEvent, TimelineStore};
use crate::directorates::registry::usage::{UsageStore, add_usage, cost_on_day, parse_ledger};

impl AccessTokenStore for SqlRegistry {
    fn store_access_token(&self, record: &crate::AccessTokenRecord) -> io::Result<()> {
        let record = validate_access_token_record(record)?;
        let text = bounded_json(
            &record,
            MAX_ACCESS_TOKEN_RECORD_BYTES,
            "access token record",
        )?;
        self.write(|tx, revision| {
            let taken = tx.query_opt(
                "SELECT token_id FROM ai6_access_tokens WHERE token_id = $1 OR digest = $2",
                &[record.token_id().into(), record.digest().into()],
            )?;
            if taken.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "access token ID or digest already exists",
                ));
            }
            tx.execute(
                "INSERT INTO ai6_access_tokens (token_id, digest, record, revision) \
                 VALUES ($1, $2, $3, $4)",
                &[
                    record.token_id().into(),
                    record.digest().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn find_access_token(&self, digest: &str) -> io::Result<Option<crate::AccessTokenRecord>> {
        validate_token_digest(digest)?;
        let Some(text) = self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_access_tokens WHERE digest = $1",
                &[digest.into()],
            )?
            .map(|row| row.text(0))
            .transpose()
        })?
        else {
            return Ok(None);
        };
        let Some(record) = stored_token(&text) else {
            return Ok(None);
        };
        if record.digest() != digest {
            return Ok(None);
        }
        Ok(record.active_at(unix_time()?).then_some(record))
    }

    fn revoke_access_token(&self, token_id: &str) -> io::Result<bool> {
        let parsed = uuid::Uuid::parse_str(token_id)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid token ID"))?;
        if parsed.to_string() != token_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "token ID must be a canonical lowercase UUID",
            ));
        }
        self.write(|tx, revision| {
            let Some(row) = tx.query_opt(
                "SELECT record FROM ai6_access_tokens WHERE token_id = $1",
                &[token_id.into()],
            )?
            else {
                return Ok(false);
            };
            let Some(record) = stored_token(&row.text(0)?) else {
                return Ok(false);
            };
            if record.token_id() != token_id || record.revoked_at().is_some() {
                return Ok(false);
            }
            let updated = crate::AccessTokenRecord::try_new(
                record.token_id(),
                record.digest(),
                record.person_id(),
                record.issued_at(),
                record.expires_at(),
                Some(unix_time()?),
            )
            .map_err(|_| invalid_data("invalid access token record"))?;
            tx.execute(
                "UPDATE ai6_access_tokens SET record = $2, revision = $3 WHERE token_id = $1",
                &[token_id.into(), to_json(&updated)?.into(), revision.into()],
            )?;
            Ok(true)
        })
    }
}

/// A stored token record that is within bounds and valid.
fn stored_token(text: &str) -> Option<crate::AccessTokenRecord> {
    if text.len() as u64 > MAX_ACCESS_TOKEN_RECORD_BYTES {
        return None;
    }
    let record = serde_json::from_str::<crate::AccessTokenRecord>(text).ok()?;
    validate_access_token_record(&record).ok()
}

impl AuditStore for SqlRegistry {
    fn record_audit(&self, entry: &AuditRecord) -> io::Result<()> {
        entry.validate()?;
        let text = bounded_json(entry, MAX_AUDIT_RECORD_BYTES, "audit entry")?;
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_audit (id, at, record, revision) VALUES ($1, $2, $3, $4)",
                &[
                    entry.id.as_str().into(),
                    entry.at.into(),
                    text.into(),
                    revision.into(),
                ],
            )
            .map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "audit entry ID already exists",
                ),
                _ => error,
            })?;
            Ok(())
        })
    }

    fn list_audit(&self, limit: usize) -> io::Result<Vec<AuditRecord>> {
        if limit == 0 || limit > MAX_AUDIT_RESULTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("audit result limit must be between 1 and {MAX_AUDIT_RESULTS}"),
            ));
        }
        let mut records = self.audit_records()?;
        records.sort_by(|left, right| {
            right
                .at
                .total_cmp(&left.at)
                .then_with(|| right.id.cmp(&left.id))
        });
        records.truncate(limit);
        Ok(records)
    }

    fn audit_records(&self) -> io::Result<Vec<AuditRecord>> {
        let rows = self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_audit LIMIT $1",
                &[(MAX_AUDIT_ENTRIES + 1).into()],
            )
        })?;
        if rows.len() > MAX_AUDIT_ENTRIES {
            return Err(invalid_data(format!(
                "Registry has more than {MAX_AUDIT_ENTRIES} audit entries"
            )));
        }
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(entry) = serde_json::from_str::<AuditRecord>(&row.text(0)?)
                && entry.validate().is_ok()
            {
                records.push(entry);
            }
        }
        Ok(records)
    }
}

impl AgentRoleStore for SqlRegistry {
    fn put_agent_role(&self, operation: &str, role: &AgentRole) -> io::Result<()> {
        validate_registry_key(operation)?;
        validate_registry_key(&role.id)?;
        role.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        let text = bounded_json(
            role,
            MAX_AGENT_ROLE_RECORD_BYTES,
            "Agent role configuration",
        )?;
        self.write(|tx, revision| {
            let ids = tx
                .query(
                    "SELECT id FROM ai6_agent_roles WHERE operation = $1",
                    &[operation.into()],
                )?
                .into_iter()
                .map(|row| row.text(0))
                .collect::<io::Result<Vec<_>>>()?;
            if !ids.contains(&role.id) && ids.len() >= MAX_AGENT_ROLES {
                return Err(invalid_data(format!(
                    "Operation has reached the {MAX_AGENT_ROLES} Agent role limit"
                )));
            }
            tx.execute(
                "INSERT INTO ai6_agent_roles (operation, id, role, revision) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (operation, id) DO UPDATE SET role = excluded.role, \
                 revision = excluded.revision",
                &[
                    operation.into(),
                    role.id.as_str().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn list_agent_roles(&self, operation: &str) -> io::Result<Vec<AgentRole>> {
        validate_registry_key(operation)?;
        let rows = self.read(|tx| {
            tx.query(
                "SELECT id, role FROM ai6_agent_roles WHERE operation = $1 ORDER BY id",
                &[operation.into()],
            )
        })?;
        let mut roles = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(role) = serde_json::from_str::<AgentRole>(&row.text(1)?)
                && role.validate().is_ok()
                && role.id == row.text(0)?
            {
                roles.push(role);
            }
        }
        Ok(roles)
    }

    fn agent_role(&self, operation: &str, key: &str) -> io::Result<Option<AgentRole>> {
        Ok(self
            .list_agent_roles(operation)?
            .into_iter()
            .filter(|role| key == role.id || key.starts_with(&format!("{}/", role.id)))
            .max_by_key(|role| role.id.len()))
    }

    fn remove_agent_role(&self, operation: &str, role_id: &str) -> io::Result<bool> {
        validate_registry_key(operation)?;
        validate_registry_key(role_id)?;
        self.write(|tx, _| {
            Ok(tx.execute(
                "DELETE FROM ai6_agent_roles WHERE operation = $1 AND id = $2",
                &[operation.into(), role_id.into()],
            )? > 0)
        })
    }
}

fn cable_from(text: &str) -> io::Result<Cable> {
    if text.len() as u64 > MAX_CABLE_RECORD_BYTES {
        return Err(invalid_data("Cable record exceeds the size limit"));
    }
    from_json(text, "Cable")
}

/// Unexpired Cables in creation order; unreadable ones are skipped as the file Registry
/// skips them.
fn live_cables(tx: &mut Tx<'_>, now: f64) -> io::Result<Vec<Cable>> {
    let rows = tx.query(
        "SELECT cable FROM ai6_cables WHERE expires_at > $1 ORDER BY created_at, id LIMIT $2",
        &[now.into(), MAX_CABLE_FILES.into()],
    )?;
    let mut cables = Vec::with_capacity(rows.len());
    for row in rows {
        if let Ok(cable) = cable_from(&row.text(0)?) {
            cables.push(cable);
        }
    }
    Ok(cables)
}

fn cable_in(tx: &mut Tx<'_>, id: &str) -> io::Result<Option<Cable>> {
    tx.query_opt("SELECT cable FROM ai6_cables WHERE id = $1", &[id.into()])?
        .map(|row| cable_from(&row.text(0)?))
        .transpose()
}

fn check_now(now: Option<f64>, what: &str) -> io::Result<f64> {
    let now = now.map_or_else(unix_time, Ok)?;
    if !now.is_finite() {
        return Err(invalid_data(format!("{what} time must be finite")));
    }
    Ok(now)
}

impl CableStore for SqlRegistry {
    fn submit_cable(&self, cable: &Cable) -> io::Result<Cable> {
        let text = bounded_json(cable, MAX_CABLE_RECORD_BYTES, "Cable record")?;
        self.write(|tx, revision| {
            if let Some(row) = tx.query_opt(
                "SELECT cable FROM ai6_cables WHERE sender_agent = $1 AND idempotency_key = $2",
                &[cable.sender_agent().into(), cable.idempotency_key().into()],
            )? {
                let existing = cable_from(&row.text(0)?)?;
                if existing.same_request(cable) {
                    return Ok(existing);
                }
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Cable idempotency key was reused for different content",
                ));
            }
            if cable_in(tx, cable.id())?.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("Cable {} already exists", cable.id()),
                ));
            }
            tx.execute(
                "INSERT INTO ai6_cables (id, sender_agent, idempotency_key, created_at, expires_at, cable, revision) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    cable.id().into(),
                    cable.sender_agent().into(),
                    cable.idempotency_key().into(),
                    cable.created_at().into(),
                    cable.expires_at().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            // The Cable is sent either way; an event out of bounds is not recorded.
            let event = cable_sent_event(cable);
            if event.to_record().is_ok() {
                insert_event(tx, &event, revision)?;
            }
            Ok(cable.clone())
        })
    }

    fn find_cable(&self, sender_agent: &str, idempotency_key: &str) -> io::Result<Option<Cable>> {
        self.read(|tx| {
            tx.query_opt(
                "SELECT cable FROM ai6_cables WHERE sender_agent = $1 AND idempotency_key = $2",
                &[sender_agent.into(), idempotency_key.into()],
            )?
            .map(|row| cable_from(&row.text(0)?))
            .transpose()
        })
    }

    fn get_cable(&self, cable_id: &str) -> io::Result<Option<Cable>> {
        validate_cable_id(cable_id)?;
        self.read(|tx| cable_in(tx, cable_id))
    }

    fn cable_inbox(
        &self,
        agent: &str,
        session_id: Option<&str>,
        limit: usize,
        now: Option<f64>,
    ) -> io::Result<Vec<Cable>> {
        if agent.trim().is_empty() || limit == 0 || limit > 1_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent must be non-empty and limit must be between 1 and 1000",
            ));
        }
        let now = check_now(now, "Cable inbox")?;
        let mut cables = self
            .read(|tx| live_cables(tx, now))?
            .into_iter()
            .filter(|cable| {
                cable.recipients().iter().any(|recipient| {
                    recipient.agent() == agent
                        && recipient
                            .session_id()
                            .is_none_or(|target| Some(target) == session_id)
                        && matches!(
                            recipient.state(),
                            CableState::Queued | CableState::Delivered
                        )
                })
            })
            .collect::<Vec<_>>();
        cables.truncate(limit);
        Ok(cables)
    }

    fn held_cables(&self, limit: usize, now: Option<f64>) -> io::Result<Vec<Cable>> {
        if limit == 0 || limit > 1_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "held Cable limit must be between 1 and 1000",
            ));
        }
        let now = check_now(now, "held Cable")?;
        let mut cables = self
            .read(|tx| live_cables(tx, now))?
            .into_iter()
            .filter(|cable| {
                cable
                    .recipients()
                    .iter()
                    .any(|recipient| recipient.state() == CableState::Held)
            })
            .collect::<Vec<_>>();
        cables.truncate(limit);
        Ok(cables)
    }

    fn update_cable_delivery(
        &self,
        cable_id: &str,
        agent: &str,
        session_id: Option<&str>,
        state: CableState,
        now: Option<f64>,
    ) -> io::Result<Option<Cable>> {
        validate_cable_id(cable_id)?;
        self.write(|tx, revision| {
            let Some(cable) = cable_in(tx, cable_id)? else {
                return Ok(None);
            };
            let now = check_now(now, "Cable delivery")?;
            let next_state = if cable.expires_at() <= now && state != CableState::Expired {
                CableState::Expired
            } else {
                state
            };
            let Some(updated) = cable
                .with_recipient_state(agent, session_id, next_state)
                .map_err(cable_error)?
            else {
                return Ok(None);
            };
            tx.execute(
                "UPDATE ai6_cables SET cable = $2, revision = $3 WHERE id = $1",
                &[
                    updated.id().into(),
                    bounded_json(&updated, MAX_CABLE_RECORD_BYTES, "Cable record")?.into(),
                    revision.into(),
                ],
            )?;
            Ok(Some(updated))
        })
    }

    fn take_cable_quotas(&self, buckets: &[(&str, f64, u32)], now: f64) -> io::Result<bool> {
        check_quota_buckets(buckets, now)?;
        self.write(|tx, revision| {
            let balances = quota_balances(buckets, now, |key| {
                tx.query_opt(
                    "SELECT tokens, updated FROM ai6_cable_quotas WHERE key = $1",
                    &[key.into()],
                )?
                .map(|row| Ok((row.real(0)?, row.real(1)?)))
                .transpose()
            })?;
            let allowed = quota_allows(&balances);
            for balance in &balances {
                match balance.remaining(allowed) {
                    Some(tokens) => tx.execute(
                        "INSERT INTO ai6_cable_quotas (key, tokens, updated, revision) \
                         VALUES ($1, $2, $3, $4) ON CONFLICT (key) DO UPDATE SET \
                         tokens = excluded.tokens, updated = excluded.updated, \
                         revision = excluded.revision",
                        &[
                            balance.key.as_str().into(),
                            tokens.into(),
                            now.into(),
                            revision.into(),
                        ],
                    )?,
                    None => tx.execute(
                        "DELETE FROM ai6_cable_quotas WHERE key = $1",
                        &[balance.key.as_str().into()],
                    )?,
                };
            }
            Ok(allowed)
        })
    }
}

/// Records `event` and drops the oldest past [`MAX_TIMELINE_EVENTS`].
fn insert_event(tx: &mut Tx<'_>, event: &TimelineEvent, revision: i64) -> io::Result<()> {
    let record = String::from_utf8(event.to_record()?)
        .map_err(|_| invalid_data("timeline event is not UTF-8"))?;
    tx.execute(
        "INSERT INTO ai6_timeline (id, operation, at_micros, event, revision) \
         VALUES ($1, $2, $3, $4, $5)",
        &[
            event.id.as_str().into(),
            event.operation.as_str().into(),
            i64::try_from(event.micros()).unwrap_or(i64::MAX).into(),
            record.into(),
            revision.into(),
        ],
    )?;
    let count = tx
        .query_opt("SELECT count(*) FROM ai6_timeline", &[])?
        .map_or(Ok(0), |row| row.int(0))?;
    let excess = count.saturating_sub(MAX_TIMELINE_EVENTS as i64);
    if excess > 0 {
        tx.execute(
            "DELETE FROM ai6_timeline WHERE id IN \
             (SELECT id FROM ai6_timeline ORDER BY at_micros, id LIMIT $1)",
            &[excess.into()],
        )?;
    }
    Ok(())
}

impl TimelineStore for SqlRegistry {
    fn record_event(&self, event: &TimelineEvent) -> io::Result<()> {
        event.to_record()?;
        self.write(|tx, revision| insert_event(tx, event, revision))
    }

    fn timeline_events(&self, operation: &str) -> io::Result<Vec<TimelineEvent>> {
        let rows = self.read(|tx| {
            tx.query(
                "SELECT event FROM ai6_timeline WHERE operation = $1 ORDER BY at_micros, id",
                &[operation.into()],
            )
        })?;
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(event) = TimelineEvent::from_record(row.text(0)?.as_bytes())
                && event.operation == operation
            {
                events.push(event);
            }
        }
        Ok(events)
    }
}

/// The stored scope of a quarantine: its Operation, or '' for every Operation.
fn scope(operation: Option<&str>) -> &str {
    operation.unwrap_or("")
}

fn quarantine_from(text: &str) -> io::Result<Quarantine> {
    if text.len() as u64 > MAX_QUARANTINE_BYTES {
        return Err(invalid_data("quarantine record exceeds the size limit"));
    }
    from_json(text, "quarantine record")
}

impl QuarantineStore for SqlRegistry {
    fn quarantine_agent(&self, record: &Quarantine) -> io::Result<Quarantine> {
        validate_quarantine(record)?;
        let text = to_json(record)?;
        self.write(|tx, revision| {
            let scope = scope(record.operation.as_deref());
            if let Some(row) = tx.query_opt(
                "SELECT record FROM ai6_quarantines WHERE agent = $1 AND scope = $2",
                &[record.agent.as_str().into(), scope.into()],
            )? {
                return quarantine_from(&row.text(0)?);
            }
            let count = tx
                .query_opt("SELECT count(*) FROM ai6_quarantines", &[])?
                .map_or(Ok(0), |row| row.int(0))?;
            if count >= MAX_QUARANTINES as i64 {
                return Err(io::Error::other("too many quarantine records"));
            }
            tx.execute(
                "INSERT INTO ai6_quarantines (agent, scope, record, revision) VALUES ($1, $2, $3, $4)",
                &[
                    record.agent.as_str().into(),
                    scope.into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            // A full or unreadable Agent record never keeps a quarantine from being stored.
            let _ = super::agents::count_in(tx, revision, &quarantine_report(record));
            Ok(record.clone())
        })
    }

    fn release_quarantine(&self, agent: &str, operation: Option<&str>) -> io::Result<bool> {
        self.write(|tx, _| {
            Ok(tx.execute(
                "DELETE FROM ai6_quarantines WHERE agent = $1 AND scope = $2",
                &[agent.into(), scope(operation).into()],
            )? > 0)
        })
    }

    fn quarantines(&self) -> io::Result<Vec<Quarantine>> {
        let rows = self.read(|tx| tx.query("SELECT record FROM ai6_quarantines", &[]))?;
        let mut records = rows
            .into_iter()
            .map(|row| quarantine_from(&row.text(0)?))
            .collect::<io::Result<Vec<_>>>()?;
        records.sort_by(|left, right| {
            (&left.agent, &left.operation).cmp(&(&right.agent, &right.operation))
        });
        Ok(records)
    }

    fn is_quarantined(&self, agent: &str, operation: Option<&str>) -> io::Result<bool> {
        Ok(self
            .quarantines()?
            .iter()
            .any(|record| record.covers(agent, operation)))
    }

    fn record_incident(&self, agent: &str, incident: Incident) -> io::Result<Vec<Incident>> {
        self.write(|tx, revision| {
            let mut history: Vec<Incident> = match tx.query_opt(
                "SELECT history FROM ai6_incidents WHERE agent = $1",
                &[agent.into()],
            )? {
                Some(row) => from_json(&row.text(0)?, "incident history")?,
                None => Vec::new(),
            };
            add_incident(&mut history, incident);
            tx.execute(
                "INSERT INTO ai6_incidents (agent, history, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (agent) DO UPDATE SET history = excluded.history, \
                 revision = excluded.revision",
                &[agent.into(), to_json(&history)?.into(), revision.into()],
            )?;
            Ok(history)
        })
    }
}

impl SleeperStore for SqlRegistry {
    fn authorize_sleeper(
        &self,
        operation: &str,
        name: &str,
        sleeper: &Sleeper,
        authorized_by: &str,
        at: i64,
    ) -> io::Result<()> {
        sleeper.validate().map_err(invalid_data)?;
        let record = SleeperAuthorization {
            fingerprint: sleeper.fingerprint(),
            authorized_by: authorized_by.to_owned(),
            at,
        };
        let event = TimelineEvent::new("sleeper", "authorized", operation, at as f64)
            .actor(authorized_by)
            .detail("sleeper", name);
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_sleeper_authorizations (operation, name, record, revision) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (operation, name) \
                 DO UPDATE SET record = excluded.record, revision = excluded.revision",
                &[
                    operation.into(),
                    name.into(),
                    to_json(&record)?.into(),
                    revision.into(),
                ],
            )?;
            // The approval stands either way; an event out of bounds is not recorded.
            if event.to_record().is_ok() {
                insert_event(tx, &event, revision)?;
            }
            Ok(())
        })
    }

    fn sleeper_authorization(
        &self,
        operation: &str,
        name: &str,
    ) -> io::Result<Option<SleeperAuthorization>> {
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_sleeper_authorizations WHERE operation = $1 AND name = $2",
                &[operation.into(), name.into()],
            )?
            .map(|row| from_json(&row.text(0)?, "Sleeper authorization"))
            .transpose()
        })
    }

    fn revoke_sleeper(&self, operation: &str, name: &str) -> io::Result<bool> {
        self.write(|tx, revision| {
            let removed = tx.execute(
                "DELETE FROM ai6_sleeper_authorizations WHERE operation = $1 AND name = $2",
                &[operation.into(), name.into()],
            )? > 0;
            if removed && let Ok(at) = unix_time() {
                let event =
                    TimelineEvent::new("sleeper", "revoked", operation, at).detail("sleeper", name);
                if event.to_record().is_ok() {
                    insert_event(tx, &event, revision)?;
                }
            }
            Ok(removed)
        })
    }

    fn fire_sleeper(
        &self,
        operation: &str,
        name: &str,
        sleeper: &Sleeper,
        window: i64,
        received_by: &str,
    ) -> io::Result<Fire> {
        match self.submit_brief(&fired_brief(operation, name, sleeper, window, received_by)) {
            Ok(()) => Ok(Fire::Queued),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(Fire::AlreadyFired),
            Err(error) => Err(error),
        }
    }
}

impl UsageStore for SqlRegistry {
    fn record_usage(
        &self,
        operation: Option<&str>,
        mission: &str,
        session: &str,
        delta: &UsageTotals,
        now: i64,
    ) -> io::Result<MissionUsage> {
        let operation = operation.unwrap_or("");
        self.write(|tx, revision| {
            let mut usage = match tx.query_opt(
                "SELECT ledger FROM ai6_usage WHERE operation = $1 AND mission = $2",
                &[operation.into(), mission.into()],
            )? {
                Some(row) => parse_ledger(row.text(0)?.as_bytes())?,
                None => MissionUsage::default(),
            };
            add_usage(&mut usage, session, delta, now);
            tx.execute(
                "INSERT INTO ai6_usage (operation, mission, ledger, revision) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (operation, mission) DO UPDATE SET ledger = excluded.ledger, \
                 revision = excluded.revision",
                &[
                    operation.into(),
                    mission.into(),
                    to_json(&usage)?.into(),
                    revision.into(),
                ],
            )?;
            Ok(usage)
        })
    }

    fn mission_usage(
        &self,
        operation: Option<&str>,
        mission: &str,
    ) -> io::Result<Option<MissionUsage>> {
        self.read(|tx| {
            tx.query_opt(
                "SELECT ledger FROM ai6_usage WHERE operation = $1 AND mission = $2",
                &[operation.unwrap_or("").into(), mission.into()],
            )?
            .map(|row| parse_ledger(row.text(0)?.as_bytes()))
            .transpose()
        })
    }

    fn operation_cost_on_day(&self, operation: Option<&str>, now: i64) -> io::Result<u64> {
        let rows = self.read(|tx| {
            tx.query(
                "SELECT ledger FROM ai6_usage WHERE operation = $1",
                &[operation.unwrap_or("").into()],
            )
        })?;
        let mut total = 0_u64;
        for row in rows {
            total = total.saturating_add(cost_on_day(&parse_ledger(row.text(0)?.as_bytes())?, now));
        }
        Ok(total)
    }
}
