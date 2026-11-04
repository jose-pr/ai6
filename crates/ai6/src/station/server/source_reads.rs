//! The Headquarters Station's side of live tracker reads: while it holds the lease, it asks
//! Universal Exports for reads and answers each through the Operation's own providers.

use std::sync::atomic::Ordering;
use std::time::Duration;

use super::DaemonState;
use crate::directorates::registry::tracker::{SourceRead, SourceReadAnswer};
use crate::station::publisher::gateway_agent;
use crate::station::source_reads::answer;
use crate::station::sources::{build_source, build_tracker};

/// Starts the reader when the Station publishes to Universal Exports.
pub(super) fn spawn(state: &DaemonState) -> std::io::Result<()> {
    let Some(publisher) = &state.publisher else {
        return Ok(());
    };
    let (origin, token, tls) = publisher.link();
    let base = state.clone();
    std::thread::Builder::new()
        .name("ai6-source-reads".to_owned())
        .spawn(move || {
            while !base.stopping.load(Ordering::Acquire) {
                let state = base.current();
                let limits = state.station.limits();
                if !holds_headquarters(&state) {
                    std::thread::sleep(limits.scheduler.interval.0);
                    continue;
                }
                let wait = limits.sources.read_wait.0;
                let agent = gateway_agent(&tls, wait + wait);
                let Some(read) = next(&agent, &origin, &token, &state, wait) else {
                    continue;
                };
                let plugins = state.plugins.clone();
                let answered = answer(
                    &read,
                    &state.roster,
                    &limits.sources,
                    &|provider: &str, settings: &toml::Table| {
                        build_source(&plugins, provider, settings)
                    },
                    &|provider: &str, settings: &toml::Table| {
                        build_tracker(&plugins, provider, settings)
                    },
                );
                give(
                    &agent,
                    &origin,
                    &token,
                    &state.station_id,
                    &read.id,
                    &answered,
                );
            }
        })?;
    Ok(())
}

/// Whether this Station holds the Headquarters lease now.
fn holds_headquarters(state: &DaemonState) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    state
        .intake
        .headquarters()
        .ok()
        .flatten()
        .is_some_and(|lease| lease.station() == state.station_id && lease.expires() > now)
}

/// The next read Universal Exports holds, waiting up to `wait` for one; `None` when there is
/// none or it could not be asked, after a pause.
fn next(
    agent: &ureq::Agent,
    origin: &url::Url,
    token: &str,
    state: &DaemonState,
    wait: Duration,
) -> Option<SourceRead> {
    let pause = || std::thread::sleep(state.station.limits().scheduler.interval.0);
    let mut endpoint = origin.join("v1/station/source-reads/next").ok()?;
    endpoint
        .query_pairs_mut()
        .append_pair("station", &state.station_id)
        .append_pair("wait_ms", &wait.as_millis().to_string());
    let response = agent
        .get(endpoint.as_str())
        .header("Authorization", &format!("Bearer {token}"))
        .call();
    let mut response = match response {
        Ok(response) if response.status().as_u16() == 200 => response,
        Ok(response) if response.status().as_u16() == 204 => return None,
        _ => {
            pause();
            return None;
        }
    };
    let most = state.station.limits().sources.read_bytes;
    let body = response
        .body_mut()
        .with_config()
        .limit(most.get().unwrap_or(u64::MAX))
        .read_to_vec()
        .ok()?;
    serde_json::from_slice(&body).ok()
}

/// Sends the answer to read `id`.
fn give(
    agent: &ureq::Agent,
    origin: &url::Url,
    token: &str,
    station: &str,
    id: &str,
    answered: &SourceReadAnswer,
) {
    let Ok(mut endpoint) = origin.join("v1/station/source-reads/") else {
        return;
    };
    if let Ok(mut segments) = endpoint.path_segments_mut() {
        segments.pop_if_empty().push(id).push("result");
    }
    endpoint.query_pairs_mut().append_pair("station", station);
    if let Err(error) = agent
        .post(endpoint.as_str())
        .header("Authorization", &format!("Bearer {token}"))
        .send_json(answered)
    {
        eprintln!("ai6: a live tracker read could not be answered: {error}");
    }
}
