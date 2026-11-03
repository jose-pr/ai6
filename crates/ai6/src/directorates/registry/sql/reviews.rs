//! Mission reviews in SQL: one row per Mission and gate, written only over the revision it read.

use std::io;

use super::{SqlRegistry, from_json, to_json};
use crate::directorates::m::review::ReviewGate;
use crate::directorates::registry::reviews::{MissionReview, MissionReviewStore};

impl MissionReviewStore for SqlRegistry {
    fn mission_review(
        &self,
        mission_id: &str,
        gate: ReviewGate,
    ) -> io::Result<Option<MissionReview>> {
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_mission_reviews WHERE mission_id = $1 AND gate = $2",
                &[mission_id.into(), gate.as_str().into()],
            )?
            .map(|row| from_json(&row.text(0)?, "Mission review"))
            .transpose()
        })
    }

    fn mission_reviews(&self, mission_id: &str) -> io::Result<Vec<MissionReview>> {
        self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_mission_reviews WHERE mission_id = $1 ORDER BY gate",
                &[mission_id.into()],
            )?
            .into_iter()
            .map(|row| from_json(&row.text(0)?, "Mission review"))
            .collect()
        })
    }

    fn put_mission_review(&self, review: &MissionReview) -> io::Result<Option<MissionReview>> {
        self.write(|tx, revision| {
            let stored = tx
                .query_opt(
                    "SELECT record FROM ai6_mission_reviews WHERE mission_id = $1 AND gate = $2",
                    &[
                        review.mission_id.as_str().into(),
                        review.gate.as_str().into(),
                    ],
                )?
                .map(|row| from_json::<MissionReview>(&row.text(0)?, "Mission review"))
                .transpose()?;
            if stored.map_or(0, |stored| stored.revision) != review.revision {
                return Ok(None);
            }
            let mut written = review.clone();
            written.revision += 1;
            tx.execute(
                "INSERT INTO ai6_mission_reviews (mission_id, gate, record, revision) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (mission_id, gate) DO UPDATE SET \
                 record = excluded.record, revision = excluded.revision",
                &[
                    written.mission_id.as_str().into(),
                    written.gate.as_str().into(),
                    to_json(&written)?.into(),
                    revision.into(),
                ],
            )?;
            Ok(Some(written))
        })
    }
}
