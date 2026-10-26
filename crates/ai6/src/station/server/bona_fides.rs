//! Bona fides on the Station: the provenance a Mission keeps from the brief it came from.

use super::DaemonState;
use crate::directorates::m::mission::MissionStore;
use crate::directorates::medical::exposure::BRIEF_REGISTRY_DROP;
use crate::directorates::registry::bona_fides::{Origin, Provenance};
use crate::directorates::registry::store::Brief;

/// Records where the Mission's brief came from. A brief the Registry kept no provenance
/// for was dropped into the queue by hand: the Mission is marked so, by this Station, and
/// its Sessions start exposed, since nobody AI6 authenticated wrote its text.
pub(super) fn record_origin(
    state: &DaemonState,
    missions: &mut MissionStore,
    mission_id: &str,
    brief: &Brief,
) -> Result<(), String> {
    let provenance = brief.provenance.clone().unwrap_or_else(|| {
        Provenance::registry_drop(brief, state.station_id.clone(), state.clock.now())
    });
    if provenance.origin == Origin::RegistryDrop {
        missions
            .expose(mission_id, &[BRIEF_REGISTRY_DROP.to_owned()])
            .map_err(|error| format!("could not record the Mission's exposure: {error:?}"))?;
    }
    missions
        .record_provenance(mission_id, provenance)
        .map_err(|error| format!("could not record the Mission's provenance: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_state;
    use super::super::{publish_station_check_in, schedule_intake_once};
    use crate::directorates::registry::bona_fides::{Origin, Provenance, record_digest};
    use crate::directorates::registry::store::Brief;

    #[test]
    fn a_mission_keeps_its_brief_s_origin_and_a_forged_drop_is_a_registry_drop() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path(), "", "[agent.007]\nhandler = \"mock-agent\"\n");
        let record = |id: &str| -> Brief {
            serde_json::from_value(serde_json::json!({
                "title": format!("Work {id}"), "id": id, "source": "manual",
                "operation": "G", "agent": "007"
            }))
            .unwrap()
        };
        let mut asked = record("asked");
        asked.provenance = Some(
            Provenance::new(Origin::Operator, &asked, "universal-exports", 1)
                .with_principal("person-m"),
        );
        state.intake.submit_brief(&asked).unwrap();
        // A hand-written file claiming to come from the gateway, digest and all.
        let forged = serde_json::json!({
            "title": "Work forged", "id": "forged", "source": "manual", "operation": "G",
            "agent": "007",
            "provenance": {
                "origin": "operator", "principal": "person-m", "received_at": 1,
                "received_by": "universal-exports",
                "digest": record_digest(&record("forged")),
            }
        });
        let queue = dir.path().join("registry").join("new");
        std::fs::write(queue.join("manual--forged.json"), forged.to_string()).unwrap();

        publish_station_check_in(&state);
        assert!(schedule_intake_once(&state));

        let missions = state.missions.lock().unwrap();
        let kept = missions.get("manual--asked").unwrap();
        assert_eq!(kept.provenance, asked.provenance);
        assert!(kept.exposure.is_empty());
        let dropped = missions.get("manual--forged").unwrap();
        let provenance = dropped.provenance.as_ref().unwrap();
        assert_eq!(provenance.origin, Origin::RegistryDrop);
        assert_eq!(provenance.principal, None);
        assert_eq!(provenance.received_by, "test");
        assert_eq!(dropped.exposure, ["brief:registry_drop"]);
    }
}
