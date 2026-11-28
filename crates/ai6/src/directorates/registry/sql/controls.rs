//! Controls in SQL: one row per control, holding its record as JSON.

use std::io;

use super::SqlRegistry;
use crate::directorates::registry::controls::{
    ControlRecord, ControlStore, control_bytes, is_open_status, oldest_first, parse_control,
};
use crate::directorates::registry::store::validate_registry_key;

impl SqlRegistry {
    fn controls_where(&self, filter: &str, params: &[&str]) -> io::Result<Vec<ControlRecord>> {
        let rows = self.read(|tx| {
            let values: Vec<_> = params.iter().map(|param| (*param).into()).collect();
            tx.query(
                &format!("SELECT record FROM ai6_controls WHERE {filter}"),
                &values,
            )
        })?;
        let mut controls = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(control) = parse_control(row.text(0)?.as_bytes()) {
                controls.push(control);
            }
        }
        oldest_first(&mut controls);
        Ok(controls)
    }
}

impl ControlStore for SqlRegistry {
    fn put_control(&self, control: &ControlRecord) -> io::Result<()> {
        control.validate()?;
        let text = String::from_utf8(control_bytes(control)?).map_err(io::Error::other)?;
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_controls (control_id, station, status, record, revision) \
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (control_id) \
                 DO UPDATE SET station = excluded.station, status = excluded.status, \
                 record = excluded.record, revision = excluded.revision",
                &[
                    control.control_id.as_str().into(),
                    control.station.as_str().into(),
                    control.status.as_str().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn open_controls(&self, station: &str) -> io::Result<Vec<ControlRecord>> {
        self.controls_where(
            "station = $1 AND status IN ('queued', 'delivered')",
            &[station],
        )
    }

    fn all_open_controls(&self) -> io::Result<Vec<ControlRecord>> {
        self.controls_where("status IN ('queued', 'delivered')", &[])
    }

    fn control(&self, id: &str) -> io::Result<Option<ControlRecord>> {
        validate_registry_key(id)?;
        let row = self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_controls WHERE control_id = $1",
                &[id.into()],
            )
        })?;
        row.map(|row| parse_control(row.text(0)?.as_bytes()))
            .transpose()
    }

    fn forget_controls(&self, finished_before: f64) -> io::Result<usize> {
        self.write(|tx, _| {
            let rows = tx.query("SELECT record FROM ai6_controls", &[])?;
            let mut forgotten = 0;
            for row in rows {
                let Ok(control) = parse_control(row.text(0)?.as_bytes()) else {
                    continue;
                };
                if !is_open_status(&control.status)
                    && control.finished_at.is_some_and(|at| at < finished_before)
                {
                    tx.execute(
                        "DELETE FROM ai6_controls WHERE control_id = $1",
                        &[control.control_id.as_str().into()],
                    )?;
                    forgotten += 1;
                }
            }
            Ok(forgotten)
        })
    }
}
