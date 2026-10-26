//! Recovery: how a Mission carries on when the Station supervising its Session goes dark.
//!
//! A Session whose harness a runner supervises on a Theater other Stations reach outlives its
//! Station: another Station takes the Session lease and re-attaches to the runner (adoption).
//! Any other Session ended with its Station's host, so it is lost and the Mission is tried
//! again as a new attempt. [`adopter`] decides which, with the reason when no Station may
//! adopt.

use crate::directorates::m::placement::{
    self, ExecutionMode, OperationPolicy, PlacementChoice, PlacementRequest, StationFacts,
};
use crate::directorates::m::theaters::LOCAL_THEATER;
use crate::directorates::registry::sessions::SessionRecord;

/// The live Station that should re-attach to `session`'s harness, or why none may.
///
/// The adopter is placed like a new Session under `policy` and `request`, but only on the
/// Session's own Theater, in field mode, on a Station that reaches that Theater's runner,
/// and never on the Station that held the Session. A Session on a Station's own host
/// (`local`), or without a runner, cannot be adopted: its harness ended with the Station.
pub fn adopter(
    policy: &OperationPolicy,
    request: &PlacementRequest,
    stations: &[StationFacts],
    session: &SessionRecord,
) -> Result<PlacementChoice, String> {
    let theater = session
        .theater
        .as_deref()
        .ok_or("its Theater was not recorded")?;
    if theater == LOCAL_THEATER {
        return Err("its harness ran on that Station's own host".into());
    }
    let launch = session
        .launch
        .as_ref()
        .ok_or("its launch was not recorded, so nothing can re-attach to it")?;
    if launch.runner.is_none() || launch.mode != ExecutionMode::Field {
        return Err(
            "no runner supervised its harness, so the harness ended with the Station".into(),
        );
    }
    let others: Vec<StationFacts> = stations
        .iter()
        .filter(|station| station.station != session.station)
        .filter(|station| {
            station
                .theaters
                .iter()
                .any(|reached| reached.id == theater && reached.runner)
        })
        .cloned()
        .collect();
    if others.is_empty() {
        return Err(format!(
            "no other Station reaches the runner on Theater {theater:?}"
        ));
    }
    let placement = placement::place(policy, request, &others);
    if let Some(choice) = placement
        .choices
        .iter()
        .find(|choice| choice.theater == theater && choice.mode == ExecutionMode::Field)
    {
        return Ok(choice.clone());
    }
    let reasons = placement
        .rejections
        .iter()
        .filter(|rejection| rejection.theater.as_deref().is_none_or(|id| id == theater))
        .map(|rejection| match &rejection.station {
            Some(station) => format!("{station}: {}", rejection.reason),
            None => rejection.reason.clone(),
        })
        .collect::<Vec<_>>();
    Err(format!(
        "no Station may adopt it on Theater {theater:?}: {}",
        if reasons.is_empty() {
            "none qualifies".to_owned()
        } else {
            reasons.join("; ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::*;
    use crate::directorates::m::placement::AgentFacts;
    use crate::directorates::m::theaters::{Theater, TheaterAccess, TheaterSummary};
    use crate::directorates::registry::backend::*;
    use crate::directorates::registry::sessions::{NewSession, SessionLaunch};
    use crate::directorates::registry::store::FileRegistry;
    use crate::{Capabilities, TransportAccess};

    fn field_theater(runner: bool) -> TheaterSummary {
        let mut theater =
            Theater::new("box", TheaterAccess::Transport(TransportAccess::new("ssh")))
                .with_workdir("/srv/work")
                .with_harness("claude");
        if runner {
            theater = theater.with_runner(crate::RunnerSettings::default());
        }
        let mut summary = theater.summary();
        summary.capabilities = Some(Capabilities {
            terminal: true,
            run: true,
            files: true,
            os: Some("linux".to_owned()),
            ..Capabilities::default()
        });
        summary
    }

    fn station(id: &str, theaters: Vec<TheaterSummary>) -> StationFacts {
        StationFacts {
            station: id.to_owned(),
            healthy: true,
            silent_for: 1,
            roles: vec!["execution".to_owned()],
            capacity: 2,
            load: 0,
            running: 0,
            labels: BTreeMap::new(),
            checkouts: Vec::new(),
            handlers: Some(vec!["claude".to_owned()]),
            theaters,
            resources: None,
            limits: Default::default(),
        }
    }

    fn policy() -> OperationPolicy {
        let mut policy = OperationPolicy::new("G");
        policy.theater = Some("box".to_owned());
        policy.agents = vec![AgentFacts {
            id: "007".to_owned(),
            handler: Some("claude".to_owned()),
            priority: 0,
            roles: Vec::new(),
            tags: Vec::new(),
        }];
        policy
    }

    fn session(theater: &str, runner: bool) -> SessionRecord {
        let root = tempfile::tempdir().unwrap();
        let mut launch = SessionLaunch::new("claude", "pty", "/srv/work");
        if runner {
            launch = launch.with_runner("key-1");
        }
        FileRegistry::open(root.path())
            .open_session(
                NewSession::new("s-1", "mission-1", "dark")
                    .with_theater(theater)
                    .with_launch(launch),
                Duration::from_secs(60),
            )
            .unwrap()
    }

    #[test]
    fn a_live_station_reaching_the_runner_adopts_and_the_dark_one_never_does() {
        let stations = [
            station("dark", vec![field_theater(true)]),
            station("no-runner", vec![field_theater(false)]),
            station("live", vec![TheaterSummary::local(), field_theater(true)]),
        ];
        let request = PlacementRequest {
            agent: Some("007".to_owned()),
            ..PlacementRequest::default()
        };
        let choice = adopter(&policy(), &request, &stations, &session("box", true)).unwrap();
        assert_eq!(
            (choice.station.as_str(), choice.theater.as_str()),
            ("live", "box")
        );
    }

    #[test]
    fn a_session_without_a_runner_or_on_a_stations_own_host_is_not_adopted() {
        let stations = [station("live", vec![field_theater(true)])];
        let request = PlacementRequest::default();
        let without_runner = adopter(&policy(), &request, &stations, &session("box", false));
        assert!(without_runner.unwrap_err().contains("no runner"));
        let local = adopter(
            &policy(),
            &request,
            &stations,
            &session(LOCAL_THEATER, true),
        );
        assert!(local.unwrap_err().contains("own host"));
    }

    #[test]
    fn no_adopter_names_why() {
        let mut busy = station("live", vec![field_theater(true)]);
        busy.load = 2;
        let refused = adopter(
            &policy(),
            &PlacementRequest::default(),
            &[busy],
            &session("box", true),
        )
        .unwrap_err();
        assert!(refused.contains("live: it is at capacity"), "{refused}");
        let nobody = adopter(
            &policy(),
            &PlacementRequest::default(),
            &[station("other", vec![field_theater(false)])],
            &session("box", true),
        )
        .unwrap_err();
        assert!(
            nobody.contains("no other Station reaches the runner"),
            "{nobody}"
        );
    }
}
