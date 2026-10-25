//! The transports a Station built for its Theaters, and what they report.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use guatiao::ToValue;

use crate::directorates::m::theaters::{Theater, TheaterAccess, TheaterSummary, Theaters};
use crate::directorates::q::transport::{Capabilities, TransportClient, TransportConfig, describe};
use crate::station::plugins::Plugins;

/// How long after a failed contact the Station tries a Theater again.
const REPROBE_AFTER: Duration = Duration::from_secs(60);

/// The transport for each Theater a Station reaches.
///
/// Building one never contacts a host. A Theater whose transport cannot be built is kept
/// with the reason, so placement and the operator see it instead of the Theater vanishing.
pub(crate) struct Transports {
    entries: BTreeMap<String, Entry>,
}

struct Entry {
    theater: Theater,
    client: Result<Arc<TransportClient>, String>,
    probe: Arc<Mutex<Probe>>,
}

#[derive(Default)]
struct Probe {
    capabilities: Option<Capabilities>,
    error: Option<String>,
    last: Option<Instant>,
    running: bool,
}

impl Transports {
    /// No Theaters at all; the implicit `local` Theater is still described by
    /// [`Transports::summaries`].
    pub(crate) fn none() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Builds the transport for every reachable Theater from the loaded providers.
    pub(crate) fn build(theaters: &Theaters, plugins: &Plugins) -> Self {
        let mut transports = Self::none();
        for theater in theaters.reachable() {
            let client = build_client(&theater.access, plugins);
            transports.insert(theater, client);
        }
        transports
    }

    /// Adds a Theater with an already built transport, replacing one with the same ID.
    #[cfg(test)]
    pub(crate) fn with_transport(
        mut self,
        theater: Theater,
        transport: Box<dyn crate::directorates::q::transport::Transport>,
    ) -> Self {
        let client = Ok(Arc::new(TransportClient::new(transport)));
        self.insert(theater, client);
        self
    }

    fn insert(&mut self, theater: Theater, client: Result<Arc<TransportClient>, String>) {
        self.entries.insert(
            theater.id.clone(),
            Entry {
                theater,
                client,
                probe: Arc::new(Mutex::new(Probe::default())),
            },
        );
    }

    /// The Theater declared under `id`.
    pub(crate) fn theater(&self, id: &str) -> Option<&Theater> {
        self.entries.get(id).map(|entry| &entry.theater)
    }

    /// The transport that reaches the Theater `id`, or why there is none.
    pub(crate) fn client(&self, id: &str) -> Result<Arc<TransportClient>, String> {
        match self.entries.get(id) {
            Some(entry) => entry.client.clone(),
            None => Err(format!("this Station does not reach a Theater {id:?}")),
        }
    }

    /// The Theaters as published at check-in, each with what its transport can do.
    ///
    /// A remote Theater that has not answered yet is contacted in the background, so a
    /// slow or dead host never delays a check-in; its summary says it is unreachable until
    /// the contact succeeds.
    pub(crate) fn summaries(&self, terminal_available: bool) -> Vec<TheaterSummary> {
        self.entries
            .values()
            .map(|entry| entry.summary(terminal_available))
            .collect()
    }

    /// One Theater as published at check-in.
    pub(crate) fn summary(&self, id: &str, terminal_available: bool) -> Option<TheaterSummary> {
        self.entries
            .get(id)
            .map(|entry| entry.summary(terminal_available))
    }
}

fn build_client(access: &TheaterAccess, plugins: &Plugins) -> Result<Arc<TransportClient>, String> {
    let (provider, settings_toml) = access.transport().map_err(|error| error.to_string())?;
    let config = match access {
        TheaterAccess::Local => None,
        _ => Some(
            TransportConfig { settings_toml }
                .to_value()
                .map_err(|error| error.to_string())?,
        ),
    };
    plugins
        .transport(&provider, config.as_ref())
        .map(|transport| Arc::new(TransportClient::new(transport)))
        .map_err(|error| error.to_string())
}

impl Entry {
    fn summary(&self, terminal_available: bool) -> TheaterSummary {
        let mut summary = self.theater.summary();
        let own_host = self.theater.access == TheaterAccess::Local;
        let mut capabilities = None;
        match &self.client {
            Ok(client) => match self.known_capabilities(client) {
                Ok(known) => capabilities = Some(known),
                Err(reason) => summary.unavailable = Some(reason),
            },
            // The Station's own host stays usable without a `local` transport library:
            // it can host the harness, but cannot be driven as a remote.
            Err(_) if own_host => {
                capabilities = Some(Capabilities {
                    files: true,
                    ..Capabilities::default()
                });
            }
            Err(reason) => summary.unavailable = Some(reason.clone()),
        }
        if let Some(capabilities) = &mut capabilities
            && own_host
        {
            // The Station opens terminals on its own host with its Terminal provider.
            capabilities.terminal |= terminal_available;
            capabilities
                .os
                .get_or_insert_with(|| std::env::consts::OS.to_owned());
            capabilities
                .arch
                .get_or_insert_with(|| std::env::consts::ARCH.to_owned());
        }
        summary.capabilities = capabilities;
        summary
    }

    /// What the transport reported, contacting a remote host in the background until it
    /// answers.
    fn known_capabilities(&self, client: &Arc<TransportClient>) -> Result<Capabilities, String> {
        let static_capabilities = client.capabilities().map_err(|error| describe(&error))?;
        if self.theater.access == TheaterAccess::Local {
            return Ok(static_capabilities);
        }
        let Ok(mut probe) = self.probe.lock() else {
            return Ok(static_capabilities);
        };
        if let Some(known) = &probe.capabilities {
            return Ok(known.clone());
        }
        let due = probe
            .last
            .is_none_or(|last| last.elapsed() >= REPROBE_AFTER);
        if due && !probe.running {
            probe.running = true;
            probe.last = Some(Instant::now());
            let (client, state) = (client.clone(), self.probe.clone());
            let spawned = std::thread::Builder::new()
                .name("ai6-transport-probe".to_owned())
                .spawn(move || {
                    let outcome = client.probe();
                    if let Ok(mut probe) = state.lock() {
                        probe.running = false;
                        probe.last = Some(Instant::now());
                        match outcome {
                            Ok(capabilities) => {
                                probe.capabilities = Some(capabilities);
                                probe.error = None;
                            }
                            Err(error) => probe.error = Some(describe(&error)),
                        }
                    }
                });
            if spawned.is_err() {
                probe.running = false;
            }
        }
        match &probe.error {
            Some(error) => Err(format!("unreachable: {error}")),
            None => Ok(static_capabilities),
        }
    }
}

#[cfg(test)]
mod tests {
    use guatiao::library::ProviderError;
    use guatiao::{Status, Value};

    use super::*;
    use crate::directorates::m::theaters::SshAccess;
    use crate::directorates::q::transport::Transport;

    struct Fixed(Capabilities, Option<&'static str>);

    impl Transport for Fixed {
        fn capabilities(&self) -> Result<Value, ProviderError> {
            self.0
                .to_value()
                .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &e.to_string()))
        }

        fn probe(&self) -> Result<Value, ProviderError> {
            match self.1 {
                Some(error) => Err(ProviderError::new(Status::GUATIAO_ERR_INTERNAL, error)),
                None => {
                    let mut probed = self.0.clone();
                    probed.os = Some("linux".to_owned());
                    probed.to_value().map_err(|e| {
                        ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &e.to_string())
                    })
                }
            }
        }
    }

    fn ssh_theater() -> Theater {
        Theater::new("box", TheaterAccess::Ssh(SshAccess::new("b.example")))
    }

    fn wait_for(transports: &Transports, done: impl Fn(&TheaterSummary) -> bool) -> TheaterSummary {
        for _ in 0..200 {
            let summary = transports.summary("box", true).unwrap();
            if done(&summary) {
                return summary;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the probe did not finish");
    }

    #[test]
    fn a_remote_theater_reports_what_its_probe_found() {
        let runs = Capabilities {
            run: true,
            ..Capabilities::default()
        };
        let transports =
            Transports::none().with_transport(ssh_theater(), Box::new(Fixed(runs, None)));
        let summary = wait_for(&transports, |s| {
            s.capabilities.as_ref().is_some_and(|c| c.os.is_some())
        });
        assert_eq!(summary.capabilities.unwrap().os.as_deref(), Some("linux"));
        assert!(summary.unavailable.is_none());
    }

    #[test]
    fn an_unreachable_theater_says_so_and_is_not_probed_in_a_loop() {
        let transports = Transports::none().with_transport(
            ssh_theater(),
            Box::new(Fixed(Capabilities::default(), Some("connection refused"))),
        );
        let summary = wait_for(&transports, |s| s.unavailable.is_some());
        assert_eq!(
            summary.unavailable.as_deref(),
            Some("unreachable: connection refused")
        );
        assert!(summary.capabilities.is_none());
    }

    #[test]
    fn the_own_host_is_usable_without_a_local_transport_and_gets_the_stations_terminal() {
        let transports = Transports::build(&Theaters::default(), &Plugins::scan(&[]));
        let local = transports.summary("local", true).unwrap();
        let capabilities = local.capabilities.unwrap();
        assert!(capabilities.terminal && capabilities.files && !capabilities.run);
        assert_eq!(capabilities.os.as_deref(), Some(std::env::consts::OS));
        assert!(local.unavailable.is_none());
        let without = transports.summary("local", false).unwrap();
        assert!(!without.capabilities.unwrap().terminal);
    }

    #[test]
    fn a_remote_theater_without_its_provider_is_kept_with_the_reason() {
        let mut theaters = Theaters::default();
        theaters.insert(ssh_theater()).unwrap();
        let transports = Transports::build(&theaters, &Plugins::scan(&[]));
        let summary = transports.summary("box", true).unwrap();
        assert_eq!(
            summary.unavailable.as_deref(),
            Some("no provider `ssh` is loaded")
        );
        assert!(transports.client("box").is_err());
        assert!(transports.client("absent").is_err());
        assert_eq!(transports.summaries(true).len(), 2);
    }
}
