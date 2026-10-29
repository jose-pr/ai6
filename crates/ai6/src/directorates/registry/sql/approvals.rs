//! Green ink requests, decisions and class authorisations in `ai6_approvals` and
//! `ai6_class_authorizations`.

use std::io;

use super::db::Tx;
use super::{SqlRegistry, bounded_json, from_json};
use crate::directorates::m::approvals::ActionScope;
use crate::directorates::registry::approvals::{
    Approval, ApprovalRequest, ApprovalStatus, ApprovalStore, ClassAuthorization, Decision,
    Revocation, Ruling, class_approval, expired, ruled, validate_ruling,
};
use crate::directorates::registry::limits::ApprovalLimits;
use crate::directorates::registry::store::{invalid_data, validate_registry_key};

fn record_json(value: &impl serde::Serialize, limits: &ApprovalLimits) -> io::Result<String> {
    bounded_json(value, limits.record_bytes, "green ink record")
}

/// The request and its decision, expiring a pending request at `now`; `now` of zero never
/// expires.
fn status_in(
    tx: &mut Tx<'_>,
    operation: &str,
    id: &str,
    now: f64,
    revision: i64,
    limits: &ApprovalLimits,
) -> io::Result<Option<ApprovalStatus>> {
    let Some(row) = tx.query_opt(
        "SELECT state, record FROM ai6_approvals WHERE operation = $1 AND id = $2",
        &[operation.into(), id.into()],
    )?
    else {
        return Ok(None);
    };
    if row.text(0)? == "decided" {
        return from_json(&row.text(1)?, "green ink record")
            .map(|a| Some(ApprovalStatus::Decided(a)));
    }
    let request: ApprovalRequest = from_json(&row.text(1)?, "green ink record")?;
    if now > 0.0 && request.expires_at <= now {
        let approval = expired(&request);
        settle(tx, &approval, revision, limits)?;
        return Ok(Some(ApprovalStatus::Decided(approval)));
    }
    Ok(Some(ApprovalStatus::Pending(request)))
}

/// Stores a decision in place of its pending request.
fn settle(
    tx: &mut Tx<'_>,
    approval: &Approval,
    revision: i64,
    limits: &ApprovalLimits,
) -> io::Result<()> {
    tx.execute(
        "UPDATE ai6_approvals SET state = 'decided', record = $3, revision = $4 \
         WHERE operation = $1 AND id = $2",
        &[
            approval.request.operation.as_str().into(),
            approval.request.id.as_str().into(),
            record_json(approval, limits)?.into(),
            revision.into(),
        ],
    )?;
    Ok(())
}

/// Adds a request to its Operation's records, counting it against its Mission's bound.
fn insert(
    tx: &mut Tx<'_>,
    request: &ApprovalRequest,
    state: &str,
    record: String,
    revision: i64,
    limits: &ApprovalLimits,
) -> io::Result<()> {
    let made = tx
        .query_opt(
            "SELECT count(*) FROM ai6_approvals WHERE operation = $1 AND mission = $2",
            &[
                request.operation.as_str().into(),
                request.mission.as_str().into(),
            ],
        )?
        .map_or(Ok(0), |row| row.int(0))?;
    let most = limits.mission_requests;
    if !most.allows(made + 1) {
        return Err(invalid_data(format!(
            "the Mission has made {most} green ink requests"
        )));
    }
    tx.execute(
        "INSERT INTO ai6_approvals (operation, id, mission, state, record, seq, revision) \
         VALUES ($1, $2, $3, $4, $5, $6, $6)",
        &[
            request.operation.as_str().into(),
            request.id.as_str().into(),
            request.mission.as_str().into(),
            state.into(),
            record.into(),
            revision.into(),
        ],
    )?;
    Ok(())
}

fn decided_in(tx: &mut Tx<'_>, operation: &str, id: &str) -> io::Result<Approval> {
    let row = tx
        .query_opt(
            "SELECT record FROM ai6_approvals WHERE operation = $1 AND id = $2 AND state = 'decided'",
            &[operation.into(), id.into()],
        )?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no decided green ink request"))?;
    from_json(&row.text(0)?, "green ink record")
}

fn classes_in(tx: &mut Tx<'_>, operation: &str) -> io::Result<Vec<ClassAuthorization>> {
    let rows = tx.query(
        "SELECT record FROM ai6_class_authorizations WHERE operation = $1 ORDER BY created_at, id",
        &[operation.into()],
    )?;
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        if let Ok(record) = serde_json::from_str::<ClassAuthorization>(&row.text(0)?)
            && record.operation == operation
        {
            records.push(record);
        }
    }
    Ok(records)
}

fn put_class(
    tx: &mut Tx<'_>,
    record: &ClassAuthorization,
    revision: i64,
    limits: &ApprovalLimits,
) -> io::Result<()> {
    tx.execute(
        "INSERT INTO ai6_class_authorizations (operation, id, created_at, record, revision) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (operation, id) \
         DO UPDATE SET record = excluded.record, revision = excluded.revision",
        &[
            record.operation.as_str().into(),
            record.id.as_str().into(),
            record.created_at.into(),
            record_json(record, limits)?.into(),
            revision.into(),
        ],
    )?;
    Ok(())
}

impl ApprovalStore for SqlRegistry {
    fn request_approval(&self, request: &ApprovalRequest) -> io::Result<ApprovalStatus> {
        request.validate(&self.limits.approvals)?;
        self.write(|tx, revision| {
            if let Some(status) = status_in(
                tx,
                &request.operation,
                &request.id,
                0.0,
                revision,
                &self.limits.approvals,
            )? {
                return Ok(status);
            }
            let pending = tx
                .query_opt(
                    "SELECT count(*) FROM ai6_approvals WHERE operation = $1 AND state = 'pending'",
                    &[request.operation.as_str().into()],
                )?
                .map_or(Ok(0), |row| row.int(0))?;
            let most = self.limits.approvals.pending;
            if !most.allows(pending + 1) {
                return Err(invalid_data(format!(
                    "the Operation has {most} requests waiting for green ink"
                )));
            }
            insert(
                tx,
                request,
                "pending",
                record_json(request, &self.limits.approvals)?,
                revision,
                &self.limits.approvals,
            )?;
            Ok(ApprovalStatus::Pending(request.clone()))
        })
    }

    fn approval(&self, operation: &str, id: &str, now: f64) -> io::Result<Option<ApprovalStatus>> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        self.write(|tx, revision| {
            status_in(tx, operation, id, now, revision, &self.limits.approvals)
        })
    }

    fn pending_approvals(&self, operation: &str, now: f64) -> io::Result<Vec<ApprovalRequest>> {
        validate_registry_key(operation)?;
        self.write(|tx, revision| {
            let rows = tx.query(
                "SELECT record FROM ai6_approvals WHERE operation = $1 AND state = 'pending'",
                &[operation.into()],
            )?;
            let mut waiting = Vec::new();
            for row in rows {
                let Ok(request) = serde_json::from_str::<ApprovalRequest>(&row.text(0)?) else {
                    continue;
                };
                if request.operation != operation {
                    continue;
                }
                if request.expires_at <= now {
                    settle(tx, &expired(&request), revision, &self.limits.approvals)?;
                } else {
                    waiting.push(request);
                }
            }
            waiting.sort_by(|left, right| {
                left.requested_at
                    .total_cmp(&right.requested_at)
                    .then_with(|| left.id.cmp(&right.id))
            });
            Ok(waiting)
        })
    }

    fn mission_approvals(
        &self,
        operation: &str,
        mission: &str,
        now: f64,
    ) -> io::Result<Vec<ApprovalStatus>> {
        validate_registry_key(operation)?;
        validate_registry_key(mission)?;
        self.write(|tx, revision| {
            let ids = tx.query(
                "SELECT id FROM ai6_approvals WHERE operation = $1 AND mission = $2 ORDER BY seq, id",
                &[operation.into(), mission.into()],
            )?;
            let mut statuses = Vec::with_capacity(ids.len());
            for row in ids {
                if let Some(status) = status_in(tx, operation, &row.text(0)?, now, revision, &self.limits.approvals)? {
                    statuses.push(status);
                }
            }
            Ok(statuses)
        })
    }

    fn live_approval(
        &self,
        operation: &str,
        mission: &str,
        scope: &ActionScope,
        now: f64,
    ) -> io::Result<Option<Approval>> {
        Ok(self
            .mission_approvals(operation, mission, now)?
            .into_iter()
            .find_map(|status| match status {
                ApprovalStatus::Decided(approval)
                    if approval.request.scope.covers(scope) && approval.is_live(now) =>
                {
                    Some(approval)
                }
                _ => None,
            }))
    }

    fn decide_approval(&self, operation: &str, id: &str, ruling: &Ruling) -> io::Result<Approval> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        validate_ruling(ruling, &self.limits.approvals)?;
        // A request found expired is recorded as expired even though the ruling is refused,
        // so the transaction commits and the refusal is returned after it.
        self.write(|tx, revision| {
            Ok::<_, io::Error>(
                match status_in(
                    tx,
                    operation,
                    id,
                    ruling.at,
                    revision,
                    &self.limits.approvals,
                )? {
                    None => Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "no such green ink request",
                    )),
                    Some(ApprovalStatus::Decided(approval))
                        if approval.decision == ruling.decision =>
                    {
                        Ok(approval)
                    }
                    Some(ApprovalStatus::Decided(approval)) => Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("the request was already {}", approval.decision.as_str()),
                    )),
                    Some(ApprovalStatus::Pending(request)) => {
                        let approval = ruled(request, ruling);
                        settle(tx, &approval, revision, &self.limits.approvals)?;
                        Ok(approval)
                    }
                },
            )
        })?
    }

    fn revoke_approval(
        &self,
        operation: &str,
        id: &str,
        by: &str,
        at: f64,
    ) -> io::Result<Approval> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        self.write(|tx, revision| {
            let mut approval = decided_in(tx, operation, id)?;
            if approval.decision != Decision::Approved {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "only an approval can be revoked",
                ));
            }
            if approval.revoked.is_none() {
                approval.revoked = Some(Revocation {
                    by: by.to_owned(),
                    at,
                });
                settle(tx, &approval, revision, &self.limits.approvals)?;
            }
            Ok(approval)
        })
    }

    fn spend_approval(&self, operation: &str, id: &str, at: f64) -> io::Result<bool> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        self.write(|tx, revision| {
            let mut approval = decided_in(tx, operation, id)?;
            if approval.spent_at.is_some() || !approval.is_live(at) {
                return Ok(false);
            }
            approval.spent_at = Some(at);
            settle(tx, &approval, revision, &self.limits.approvals)?;
            Ok(true)
        })
    }

    fn mark_approval_notified(&self, operation: &str, id: &str, at: f64) -> io::Result<()> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        self.write(|tx, revision| {
            let mut approval = decided_in(tx, operation, id)?;
            if approval.notified_at.is_none() {
                approval.notified_at = Some(at);
                settle(tx, &approval, revision, &self.limits.approvals)?;
            }
            Ok(())
        })
    }

    fn approve_by_class(
        &self,
        request: &ApprovalRequest,
        role: Option<&str>,
        clearance: &str,
        now: f64,
    ) -> io::Result<Option<Approval>> {
        request.validate(&self.limits.approvals)?;
        self.write(|tx, revision| {
            if status_in(
                tx,
                &request.operation,
                &request.id,
                0.0,
                revision,
                &self.limits.approvals,
            )?
            .is_some()
            {
                return Ok(None);
            }
            let Some(mut class) = classes_in(tx, &request.operation)?
                .into_iter()
                .find(|class| {
                    class.is_live(now)
                        && class.covers(&request.agent, role, clearance, &request.scope)
                        && class.allows_proposed_role(request, role)
                })
            else {
                return Ok(None);
            };
            class.uses += 1;
            put_class(tx, &class, revision, &self.limits.approvals)?;
            let approval = class_approval(request, &class, now);
            insert(
                tx,
                request,
                "decided",
                record_json(&approval, &self.limits.approvals)?,
                revision,
                &self.limits.approvals,
            )?;
            Ok(Some(approval))
        })
    }

    fn put_class_authorization(&self, record: &ClassAuthorization) -> io::Result<()> {
        record.validate(&self.limits.approvals)?;
        self.write(|tx, revision| {
            let held = tx
                .query(
                    "SELECT id FROM ai6_class_authorizations WHERE operation = $1",
                    &[record.operation.as_str().into()],
                )?
                .into_iter()
                .map(|row| row.text(0))
                .collect::<io::Result<Vec<_>>>()?;
            if held.contains(&record.id) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "the class authorisation identity is taken",
                ));
            }
            let most = self.limits.approvals.class_authorizations;
            if !most.allows(held.len() + 1) {
                return Err(invalid_data(format!(
                    "the Operation holds {most} class authorisations"
                )));
            }
            put_class(tx, record, revision, &self.limits.approvals)
        })
    }

    fn class_authorizations(&self, operation: &str) -> io::Result<Vec<ClassAuthorization>> {
        validate_registry_key(operation)?;
        self.read(|tx| classes_in(tx, operation))
    }

    fn revoke_class_authorization(
        &self,
        operation: &str,
        id: &str,
        by: &str,
        at: f64,
    ) -> io::Result<ClassAuthorization> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        self.write(|tx, revision| {
            let row = tx
                .query_opt(
                    "SELECT record FROM ai6_class_authorizations WHERE operation = $1 AND id = $2",
                    &[operation.into(), id.into()],
                )?
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "no such class authorisation")
                })?;
            let mut record: ClassAuthorization = from_json(&row.text(0)?, "green ink record")?;
            if record.revoked.is_none() {
                record.revoked = Some(Revocation {
                    by: by.to_owned(),
                    at,
                });
                put_class(tx, &record, revision, &self.limits.approvals)?;
            }
            Ok(record)
        })
    }
}
