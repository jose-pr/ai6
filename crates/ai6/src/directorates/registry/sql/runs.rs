//! Run records and their fenced leases in `ai6_runs`.

use std::io;
use std::time::Duration;

use super::db::Tx;
use super::stations::headquarters_in;
use super::{SqlRegistry, from_json, to_json};
use crate::directorates::medical::exposure::Exposure;
use crate::directorates::registry::limits::RunLimits;
use crate::directorates::registry::runs::{
    FinalReport, MissionVerdict, NewRun, RunError, RunRecord, RunStore, RunUpdate, apply_fenced,
    apply_probe_request, apply_recommendation, apply_verdict, latest, lose, merged_exposure,
    opened, parse_run, run_json, same_run, take, validate_final_report, validate_new_run,
    validate_run_id, validate_verdict,
};
use crate::directorates::registry::store::{invalid_data, validate_station_id};

fn term(tx: &mut Tx<'_>) -> io::Result<u64> {
    Ok(headquarters_in(tx)?.map_or(0, |lease| lease.term()))
}

fn run_in(tx: &mut Tx<'_>, id: &str, limits: &RunLimits) -> io::Result<Option<RunRecord>> {
    tx.query_opt(
        "SELECT record FROM ai6_runs WHERE run_id = $1",
        &[id.into()],
    )?
    .map(|row| parse_run(row.text(0)?.as_bytes(), limits))
    .transpose()
}

fn put_run(
    tx: &mut Tx<'_>,
    record: &RunRecord,
    revision: i64,
    limits: &RunLimits,
) -> io::Result<()> {
    let text = String::from_utf8(run_json(record, limits)?)
        .map_err(|_| invalid_data("Run record is not UTF-8"))?;
    tx.execute(
        "INSERT INTO ai6_runs (run_id, mission_id, record, revision) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (run_id) \
         DO UPDATE SET mission_id = excluded.mission_id, record = excluded.record, \
         revision = excluded.revision",
        &[
            record.id.as_str().into(),
            record.mission_id.as_str().into(),
            text.into(),
            revision.into(),
        ],
    )?;
    Ok(())
}

fn records(
    tx: &mut Tx<'_>,
    mission: Option<&str>,
    limits: &RunLimits,
) -> io::Result<Vec<RunRecord>> {
    let rows = match mission {
        Some(mission) => tx.query(
            "SELECT record FROM ai6_runs WHERE mission_id = $1 ORDER BY run_id",
            &[mission.into()],
        )?,
        None => tx.query("SELECT record FROM ai6_runs ORDER BY run_id", &[])?,
    };
    rows.into_iter()
        .map(|row| parse_run(row.text(0)?.as_bytes(), limits))
        .collect()
}

/// When a change happens: at a given time, or now on the database's clock.
fn when(tx: &mut Tx<'_>, at: Option<f64>) -> io::Result<f64> {
    at.map_or_else(|| tx.now(), Ok)
}

impl SqlRegistry {
    /// Changes one Run record under the Registry clock at `at` (the database's clock when
    /// `None`); `change` may read the Headquarters term in the same transaction.
    fn change_run(
        &self,
        id: &str,
        at: Option<f64>,
        change: impl FnOnce(&mut RunRecord, &mut Tx<'_>, f64) -> Result<(), RunError>,
    ) -> Result<RunRecord, RunError> {
        validate_run_id(id, &self.limits.runs)?;
        self.write(|tx, revision| {
            let mut record = run_in(tx, id, &self.limits.runs)?.ok_or(RunError::NotFound)?;
            let now = when(tx, at)?;
            change(&mut record, tx, now)?;
            put_run(tx, &record, revision, &self.limits.runs)?;
            Ok(record)
        })
    }

    fn open_run_when(
        &self,
        new: NewRun,
        ttl: Duration,
        at: Option<f64>,
    ) -> Result<RunRecord, RunError> {
        validate_new_run(&new, &self.limits.runs)?;
        self.write(|tx, revision| {
            if run_in(tx, &new.run_id, &self.limits.runs)?.is_some()
                || records(tx, Some(&new.mission_id), &self.limits.runs)?
                    .iter()
                    .any(|record| same_run(record, &new))
            {
                return Err(RunError::Exists);
            }
            let count = tx
                .query_opt("SELECT count(*) FROM ai6_runs", &[])?
                .map_or(Ok(0), |row| row.int(0))?;
            if !self.limits.runs.records.allows(count + 1) {
                return Err(io::Error::other("too many Run records").into());
            }
            let now = when(tx, at)?;
            let record = opened(new, ttl, now, term(tx)?);
            put_run(tx, &record, revision, &self.limits.runs)?;
            Ok(record)
        })
    }

    fn take_run_when(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
        at: Option<f64>,
    ) -> Result<RunRecord, RunError> {
        validate_station_id(station)?;
        self.change_run(id, at, |record, tx, now| {
            take(record, station, ttl, now, || term(tx))
        })
    }

    fn write_fenced_when(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        at: Option<f64>,
        renew: Option<Duration>,
        update: RunUpdate,
    ) -> Result<RunRecord, RunError> {
        self.change_run(id, at, |record, _, now| {
            apply_fenced(
                record,
                station,
                fence,
                now,
                renew,
                update,
                &self.limits.runs,
            )
        })
    }

    fn lose_run_when(
        &self,
        id: &str,
        reason: &str,
        at: Option<f64>,
    ) -> Result<RunRecord, RunError> {
        self.change_run(id, at, |record, tx, now| {
            lose(record, reason, now, || term(tx), &self.limits.runs)
        })
    }

    #[cfg(test)]
    pub(crate) fn open_run_at(
        &self,
        new: NewRun,
        ttl: Duration,
        now: f64,
    ) -> Result<RunRecord, RunError> {
        self.open_run_when(new, ttl, Some(now))
    }

    #[cfg(test)]
    pub(crate) fn take_run_at(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
        now: f64,
    ) -> Result<RunRecord, RunError> {
        self.take_run_when(id, station, ttl, Some(now))
    }

    #[cfg(test)]
    pub(crate) fn write_fenced(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        now: f64,
        renew: Option<Duration>,
        update: RunUpdate,
    ) -> Result<RunRecord, RunError> {
        self.write_fenced_when(id, station, fence, Some(now), renew, update)
    }

    #[cfg(test)]
    pub(crate) fn lose_run_at(
        &self,
        id: &str,
        reason: &str,
        now: f64,
    ) -> Result<RunRecord, RunError> {
        self.lose_run_when(id, reason, Some(now))
    }
}

impl RunStore for SqlRegistry {
    fn open_run(&self, new: NewRun, ttl: Duration) -> Result<RunRecord, RunError> {
        self.open_run_when(new, ttl, None)
    }

    fn run(&self, id: &str) -> io::Result<Option<RunRecord>> {
        validate_run_id(id, &self.limits.runs)?;
        self.read(|tx| run_in(tx, id, &self.limits.runs))
    }

    fn mission_exposure(&self, mission_id: &str) -> io::Result<Vec<Exposure>> {
        Ok(merged_exposure(self.read(|tx| {
            records(tx, Some(mission_id), &self.limits.runs)
        })?))
    }

    fn runs(&self) -> io::Result<Vec<RunRecord>> {
        self.read(|tx| records(tx, None, &self.limits.runs))
    }

    fn take_run(&self, id: &str, station: &str, ttl: Duration) -> Result<RunRecord, RunError> {
        self.take_run_when(id, station, ttl, None)
    }

    fn renew_run(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        ttl: Duration,
    ) -> Result<RunRecord, RunError> {
        self.write_fenced_when(id, station, fence, None, Some(ttl), RunUpdate::default())
    }

    fn update_run(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        update: RunUpdate,
    ) -> Result<RunRecord, RunError> {
        self.write_fenced_when(id, station, fence, None, None, update)
    }

    fn lose_run(&self, id: &str, reason: &str) -> Result<RunRecord, RunError> {
        self.lose_run_when(id, reason, None)
    }

    fn latest_run(&self, mission_id: &str) -> io::Result<Option<RunRecord>> {
        Ok(latest(self.read(|tx| {
            records(tx, Some(mission_id), &self.limits.runs)
        })?))
    }

    fn record_verdict(
        &self,
        operation: &str,
        mission_id: &str,
        verdict: &MissionVerdict,
    ) -> Result<RunRecord, RunError> {
        validate_verdict(verdict, &self.limits.runs)?;
        self.write(|tx, revision| {
            let mut record = latest(
                records(tx, Some(mission_id), &self.limits.runs)?
                    .into_iter()
                    .filter(|record| record.operation.as_deref() == Some(operation)),
            )
            .ok_or(RunError::NotFound)?;
            apply_verdict(&mut record, verdict.clone())?;
            put_run(tx, &record, revision, &self.limits.runs)?;
            Ok(record)
        })
    }

    fn record_recommendation(
        &self,
        operation: &str,
        mission_id: &str,
        recommendation: &MissionVerdict,
    ) -> Result<RunRecord, RunError> {
        validate_verdict(recommendation, &self.limits.runs)?;
        self.write(|tx, revision| {
            let mut record = latest(
                records(tx, Some(mission_id), &self.limits.runs)?
                    .into_iter()
                    .filter(|record| record.operation.as_deref() == Some(operation)),
            )
            .ok_or(RunError::NotFound)?;
            apply_recommendation(&mut record, recommendation.clone())?;
            put_run(tx, &record, revision, &self.limits.runs)?;
            Ok(record)
        })
    }

    fn request_probe(
        &self,
        operation: &str,
        mission_id: &str,
        at: f64,
    ) -> Result<RunRecord, RunError> {
        self.write(|tx, revision| {
            let mut record = latest(
                records(tx, Some(mission_id), &self.limits.runs)?
                    .into_iter()
                    .filter(|record| record.operation.as_deref() == Some(operation)),
            )
            .ok_or(RunError::NotFound)?;
            apply_probe_request(&mut record, at)?;
            put_run(tx, &record, revision, &self.limits.runs)?;
            Ok(record)
        })
    }

    fn file_final_report(&self, report: &FinalReport) -> io::Result<()> {
        validate_final_report(report, &self.limits.runs)?;
        let text = to_json(report)?;
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_final_reports (run_id, mission_id, record, revision) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (run_id) \
                 DO UPDATE SET record = excluded.record, revision = excluded.revision",
                &[
                    report.run_id.as_str().into(),
                    report.mission_id.as_str().into(),
                    text.as_str().into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn final_report(&self, run_id: &str) -> io::Result<Option<FinalReport>> {
        validate_run_id(run_id, &self.limits.runs)?;
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_final_reports WHERE run_id = $1",
                &[run_id.into()],
            )?
            .map(|row| from_json(&row.text(0)?, "final report"))
            .transpose()
        })
    }
}
