//! The transports a Station built for its Theaters, and what they report.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use guatiao::ToValue;

use crate::directorates::m::placement::Resources;
use crate::directorates::m::theaters::{Theater, TheaterAccess, TheaterSummary, Theaters};
use crate::directorates::q::ephemeral::{TheaterMaker, TheaterProviderConfig};
use crate::directorates::q::provision::{self, ProbeReport};
use crate::directorates::q::transport::{Capabilities, TransportClient, describe};
use crate::station::limits::StationLimits;
use crate::station::plugins::Plugins;

/// The transport for each Theater a Station reaches.
///
/// Building one never contacts a host. A Theater whose transport cannot be built is kept
/// with the reason, so placement and the operator see it instead of the Theater vanishing.
/// An ephemeral template has a Theater provider instead of a transport; the Theaters made
/// from it come and go while the Station runs.
pub(crate) struct Transports {
    entries: RwLock<BTreeMap<String, Arc<Entry>>>,
    /// How often a Theater other than the Station's own host is sampled for its resources;
    /// zero leaves only the full probe's sample.
    resources_every: Duration,
    /// The `[transports]` and `[provision]` tables pace and bound the probes.
    limits: Arc<StationLimits>,
}

struct Entry {
    theater: Theater,
    client: Result<Arc<TransportClient>, String>,
    /// The provider of an ephemeral template.
    maker: Option<Result<Arc<TheaterMaker>, String>>,
    /// What an ephemeral Theater was made for.
    made: Option<Made>,
    probe: Arc<Mutex<Probe>>,
}

/// Where an ephemeral Theater came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Made {
    /// The template's Theater ID.
    pub(crate) template: String,
    /// The Mission it was made for.
    pub(crate) mission: String,
    /// When, as Unix seconds.
    pub(crate) created_at: u64,
}

#[derive(Default)]
struct Probe {
    capabilities: Option<Capabilities>,
    error: Option<String>,
    last: Option<Instant>,
    running: bool,
    report: Option<ProbeReport>,
    reported: Option<Instant>,
    reporting: bool,
    resources: Option<Resources>,
    sampled: Option<Instant>,
    sampling: bool,
}

impl Transports {
    /// No Theaters at all; the implicit `local` Theater is still described by
    /// [`Transports::summaries`].
    pub(crate) fn none() -> Self {
        Self {
            entries: RwLock::new(BTreeMap::new()),
            resources_every: crate::station::server::DEFAULT_RESOURCES_EVERY,
            limits: Arc::default(),
        }
    }

    /// Probes Theaters as the `station` record's tables say.
    #[must_use]
    pub(crate) fn with_limits(mut self, limits: Arc<StationLimits>) -> Self {
        self.limits = limits;
        self
    }

    /// Samples each Theater other than the Station's own host every `every`; zero leaves
    /// only the full probe's sample.
    #[must_use]
    pub(crate) fn with_resources_every(mut self, every: Duration) -> Self {
        self.resources_every = every;
        self
    }

    /// Builds the transport for every reachable Theater, and the Theater provider for every
    /// ephemeral template, from the loaded providers.
    pub(crate) fn build(theaters: &Theaters, plugins: &Plugins) -> Self {
        let transports = Self::none();
        for theater in theaters.reachable() {
            if let TheaterAccess::Ephemeral(template) = &theater.access {
                let maker = template
                    .settings_toml()
                    .map_err(|error| error.to_string())
                    .and_then(|settings_toml| {
                        TheaterProviderConfig { settings_toml }
                            .to_value()
                            .map_err(|error| error.to_string())
                    })
                    .and_then(|config| {
                        plugins
                            .theater_provider(&template.provider, Some(&config))
                            .map_err(|error| error.to_string())
                    })
                    .map(|provider| Arc::new(TheaterMaker::new(provider)));
                transports.insert(Entry::new(
                    theater,
                    Err("an ephemeral template has no transport of its own".to_owned()),
                    Some(maker),
                    None,
                ));
            } else {
                let client = plugins.theater_transport(&theater.access).map(Arc::new);
                transports.insert(Entry::new(theater, client, None, None));
            }
        }
        transports
    }

    /// Adds a Theater with an already built transport, replacing one with the same ID.
    #[cfg(test)]
    pub(crate) fn with_transport(
        self,
        theater: Theater,
        transport: Box<dyn crate::directorates::q::transport::Transport>,
    ) -> Self {
        let client = Ok(Arc::new(TransportClient::new(transport)));
        self.insert(Entry::new(theater, client, None, None));
        self
    }

    fn insert(&self, entry: Entry) {
        if let Ok(mut entries) = self.entries.write() {
            entries.insert(entry.theater.id.clone(), Arc::new(entry));
        }
    }

    fn entry(&self, id: &str) -> Option<Arc<Entry>> {
        self.entries.read().ok()?.get(id).cloned()
    }

    /// Adds a Theater made from an ephemeral template, reached through `client`.
    pub(crate) fn add_made(&self, theater: Theater, client: TransportClient, made: Made) {
        self.insert(Entry::new(theater, Ok(Arc::new(client)), None, Some(made)));
    }

    /// Forgets the Theater `id`.
    pub(crate) fn remove(&self, id: &str) {
        if let Ok(mut entries) = self.entries.write() {
            entries.remove(id);
        }
    }

    /// Every Theater made from an ephemeral template, by ID.
    pub(crate) fn made(&self) -> Vec<(String, Made)> {
        self.entries
            .read()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|(id, entry)| Some((id.clone(), entry.made.clone()?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every ephemeral template, with its Theater provider or why there is none.
    pub(crate) fn templates(&self) -> Vec<(Theater, Result<Arc<TheaterMaker>, String>)> {
        self.entries
            .read()
            .map(|entries| {
                entries
                    .values()
                    .filter_map(|entry| Some((entry.theater.clone(), entry.maker.clone()?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Records a probe report taken outside the background probe, such as right after
    /// provisioning.
    pub(crate) fn set_report(&self, id: &str, report: ProbeReport) {
        if let Some(entry) = self.entry(id)
            && let Ok(mut probe) = entry.probe.lock()
        {
            probe.report = Some(report);
            probe.reported = Some(Instant::now());
        }
    }

    /// The Theater declared under `id`.
    pub(crate) fn theater(&self, id: &str) -> Option<Theater> {
        self.entry(id).map(|entry| entry.theater.clone())
    }

    /// The transport that reaches the Theater `id`, or why there is none.
    #[cfg_attr(
        not(feature = "cables-mcp"),
        allow(dead_code, reason = "only the Theater tools use it")
    )]
    pub(crate) fn client(&self, id: &str) -> Result<Arc<TransportClient>, String> {
        match self.entry(id) {
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
        let entries: Vec<Arc<Entry>> = self
            .entries
            .read()
            .map(|entries| entries.values().cloned().collect())
            .unwrap_or_default();
        entries
            .iter()
            .map(|entry| entry.summary(terminal_available, self.resources_every, &self.limits))
            .collect()
    }

    /// One Theater as published at check-in.
    pub(crate) fn summary(&self, id: &str, terminal_available: bool) -> Option<TheaterSummary> {
        self.entry(id)
            .map(|entry| entry.summary(terminal_available, self.resources_every, &self.limits))
    }
}

impl Entry {
    fn new(
        theater: Theater,
        client: Result<Arc<TransportClient>, String>,
        maker: Option<Result<Arc<TheaterMaker>, String>>,
        made: Option<Made>,
    ) -> Self {
        Self {
            theater,
            client,
            maker,
            made,
            probe: Arc::new(Mutex::new(Probe::default())),
        }
    }

    /// An ephemeral template as published: what the Theaters made from it can do, and the
    /// operating system it requires when its provider does not say.
    fn template_summary(&self, maker: &Result<Arc<TheaterMaker>, String>) -> TheaterSummary {
        let mut summary = self.theater.summary();
        match maker
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|maker| maker.capabilities().map_err(|error| describe(&error)))
        {
            Ok(mut capabilities) => {
                if capabilities.os.is_none() {
                    capabilities.os = self.theater.requires.as_ref().and_then(|r| r.os.clone());
                }
                summary.capabilities = Some(capabilities);
            }
            Err(reason) => summary.unavailable = Some(reason),
        }
        summary
    }

    fn summary(
        &self,
        terminal_available: bool,
        resources_every: Duration,
        limits: &StationLimits,
    ) -> TheaterSummary {
        if let Some(maker) = &self.maker {
            return self.template_summary(maker);
        }
        let mut summary = self.theater.summary();
        summary.made_for = self.made.as_ref().map(|made| made.mission.clone());
        let own_host = self.theater.access == TheaterAccess::Local;
        let mut capabilities = None;
        match &self.client {
            Ok(client) => match self.known_capabilities(client, limits) {
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
        summary.report = match (&self.client, &capabilities) {
            (Ok(client), Some(capabilities)) if summary.unavailable.is_none() => {
                self.known_report(client, capabilities, limits)
            }
            (Err(reason), _) if self.theater.requires.is_some() => Some(ProbeReport::failed(
                format!("this Station cannot reach it to probe it: {reason}"),
            )),
            _ => None,
        };
        if let Some(report) = &summary.report {
            for ambient in &report.ambient {
                summary
                    .weakened
                    .push(format!("ambient credential on the Theater: {ambient}"));
            }
            // A harness the probe found and accepted is installed there, declared or not.
            for harness in self.theater.requires.iter().flat_map(|r| &r.harnesses) {
                let name = harness.name();
                let accepted = report.found.contains_key(name)
                    && !report.unmet.iter().any(|unmet| unmet.name == name);
                if accepted && !summary.harnesses.iter().any(|known| known == name) {
                    summary.harnesses.push(name.to_owned());
                }
            }
        }
        summary.resources = if own_host {
            crate::station::resources::last()
        } else {
            let sampled = match (&self.client, &capabilities) {
                (Ok(client), Some(capabilities)) if summary.unavailable.is_none() => {
                    self.known_resources(client, capabilities, resources_every, limits)
                }
                _ => None,
            };
            let probed = summary.report.as_ref().and_then(|r| r.resources.clone());
            match (sampled, probed) {
                (Some(sampled), Some(probed)) if probed.sampled_at > sampled.sampled_at => {
                    Some(probed)
                }
                (sampled, probed) => sampled.or(probed),
            }
        };
        summary.capabilities = capabilities;
        summary
    }

    /// The Theater's last resources sample, taking another in the background every `every`.
    /// `None` until one succeeds, with `every` zero, and for a Windows host.
    fn known_resources(
        &self,
        client: &Arc<TransportClient>,
        capabilities: &Capabilities,
        every: Duration,
        limits: &StationLimits,
    ) -> Option<Resources> {
        if every.is_zero() || capabilities.os.as_deref() == Some("windows") {
            return None;
        }
        let mut probe = self.probe.lock().ok()?;
        let due = probe
            .sampled
            .is_none_or(|sampled| sampled.elapsed() >= every);
        if due && !probe.sampling {
            probe.sampling = true;
            let (client, state, capabilities) =
                (client.clone(), self.probe.clone(), capabilities.clone());
            let provision_limits = limits.provision.clone();
            let spawned = std::thread::Builder::new()
                .name("ai6-theater-resources".to_owned())
                .spawn(move || {
                    let sampled =
                        provision::probe_resources(&client, &capabilities, &provision_limits);
                    if let Ok(mut probe) = state.lock() {
                        probe.sampling = false;
                        probe.sampled = Some(Instant::now());
                        // A failed sample keeps the last good one; its time says how old it is.
                        if let Ok(resources) = sampled {
                            probe.resources = Some(resources);
                        }
                    }
                });
            if spawned.is_err() {
                probe.sampling = false;
            }
        }
        probe.resources.clone()
    }

    /// The last report on the Theater's requirements and ambient credentials, looking again in
    /// the background when it is due. `None` until a first look finishes, and for a Windows
    /// host that declares no requirements.
    fn known_report(
        &self,
        client: &Arc<TransportClient>,
        capabilities: &Capabilities,
        limits: &StationLimits,
    ) -> Option<ProbeReport> {
        let requires = self.theater.requires.clone();
        if requires.is_none() && capabilities.os.as_deref() == Some("windows") {
            return None;
        }
        let mut probe = self.probe.lock().ok()?;
        let due = probe
            .reported
            .is_none_or(|reported| reported.elapsed() >= limits.transports.report_every.0);
        if due && !probe.reporting {
            probe.reporting = true;
            let (client, state, capabilities) =
                (client.clone(), self.probe.clone(), capabilities.clone());
            let workdir = self.theater.workdir.clone();
            let provision_limits = limits.provision.clone();
            let spawned = std::thread::Builder::new()
                .name("ai6-theater-report".to_owned())
                .spawn(move || {
                    let report = provision::probe(
                        &client,
                        &capabilities,
                        requires.as_ref(),
                        workdir.as_deref(),
                        &provision_limits,
                    );
                    if let Ok(mut probe) = state.lock() {
                        probe.reporting = false;
                        probe.reported = Some(Instant::now());
                        probe.report = Some(report);
                    }
                });
            if spawned.is_err() {
                probe.reporting = false;
            }
        }
        probe.report.clone()
    }

    /// What the transport reported, contacting a remote host in the background until it
    /// answers.
    fn known_capabilities(
        &self,
        client: &Arc<TransportClient>,
        limits: &StationLimits,
    ) -> Result<Capabilities, String> {
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
            .is_none_or(|last| last.elapsed() >= limits.transports.reprobe_after.0);
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
    use guatiao::{Status, ToValue, Value};

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

    /// A host whose every command prints `stdout`, as the probe script would.
    struct Answers(&'static str);

    impl Transport for Answers {
        fn capabilities(&self) -> Result<Value, ProviderError> {
            Capabilities {
                run: true,
                ..Capabilities::default()
            }
            .to_value()
            .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &e.to_string()))
        }

        fn run(&self, _spec: &Value) -> Result<Value, ProviderError> {
            crate::directorates::q::transport::Output {
                code: Some(0),
                stdout: guatiao::Bytes(self.0.as_bytes().to_vec()),
                ..Default::default()
            }
            .to_value()
            .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &e.to_string()))
        }
    }

    #[test]
    fn a_probe_reports_unmet_requirements_and_ambient_credentials_without_values() {
        use crate::directorates::q::provision::Requirements;
        let theater = ssh_theater().with_requirements(
            Requirements::default()
                .with_os("linux")
                .with_harness("claude>=2.1".parse().unwrap())
                .with_tool("git".parse().unwrap()),
        );
        let transports = Transports::none().with_transport(
            theater,
            Box::new(Answers(
                "os\tLinux\nfound\tgit\tgit version 2.47.0\nmissing\tclaude\nfree_kb\t2048\n\
                 ambient\ta git credential store (~/.git-credentials)\n",
            )),
        );
        let summary = wait_for(&transports, |s| s.report.is_some());
        let report = summary.report.unwrap();
        assert_eq!(report.os.as_deref(), Some("linux"));
        assert_eq!(report.found["git"], "git version 2.47.0");
        assert_eq!(report.free_mb, Some(2));
        assert_eq!(
            report
                .unmet
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["claude>=2.1: not installed"]
        );
        assert_eq!(
            summary.weakened,
            ["ambient credential on the Theater: a git credential store (~/.git-credentials)"]
        );
        assert!(
            summary.harnesses.is_empty(),
            "a missing harness is not published"
        );
    }

    #[test]
    fn a_remote_theater_is_sampled_and_the_own_host_carries_the_stations_sample() {
        let lines = "nproc\t4\nloadavg\t0.50 0.40 0.30 1/100 7\n\
                     mem_total_kb\t2097152\nmem_available_kb\t1048576\n";
        let transports = Transports::none().with_transport(ssh_theater(), Box::new(Answers(lines)));
        let summary = wait_for(&transports, |s| s.resources.is_some());
        let resources = summary.resources.unwrap();
        assert_eq!(
            (
                resources.cpus,
                resources.load_1m,
                resources.memory_total_mb,
                resources.memory_available_mb
            ),
            (Some(4.0), Some(0.5), Some(2048), Some(1024))
        );

        // With sampling off, the full probe's sample is what the Theater shows.
        let off = Transports::none()
            .with_resources_every(Duration::ZERO)
            .with_transport(ssh_theater(), Box::new(Answers(lines)));
        let summary = wait_for(&off, |s| s.report.is_some());
        assert!(summary.resources.is_some());
        assert_eq!(summary.resources, summary.report.unwrap().resources);

        let own = Transports::build(&Theaters::default(), &Plugins::scan(&[]));
        crate::station::resources::sample();
        let local = own.summary("local", true).unwrap();
        assert!(
            local
                .resources
                .is_some_and(|resources| resources.memory_total_mb.is_some())
        );
    }

    /// Every file under `root` with its contents and permissions, to show a run wrote nothing.
    #[cfg(unix)]
    fn snapshot(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>, u32)> {
        use std::os::unix::fs::PermissionsExt;
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
                    files.push((path.clone(), std::fs::read(&path).unwrap(), mode));
                }
            }
        }
        files.sort();
        files
    }

    #[cfg(unix)]
    #[test]
    fn provisioning_twice_changes_then_finds_everything_ok_and_a_check_writes_nothing() {
        use crate::directorates::m::theaters::RunnerSettings;
        use crate::directorates::q::provision::{
            ProvisionSpec, ProvisionStep, Requirements, StepStatus, provision,
        };
        use crate::station::transports::testing::ShellTransport;

        let station = tempfile::tempdir().unwrap();
        let build = station.path().join("ai6-build");
        std::fs::write(&build, b"#!/bin/sh\necho ai6 test build\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let theater = Theater::new("box", TheaterAccess::Ssh(SshAccess::new("b.example")))
            .with_runner(RunnerSettings::default().with_command("~/opt/ai6/bin/ai6"))
            .with_requirements(
                Requirements::default()
                    .with_tool("ai6-stand-in>=1.2".parse().unwrap())
                    .with_disk_mb(1),
            )
            .with_provision(
                ProvisionSpec::default()
                    .with_runner(build.to_string_lossy())
                    .with_install(
                        "ai6-stand-in",
                        "mkdir -p bin && printf '#!/bin/sh\\necho ai6-stand-in 1.2.0\\n' \
                         > bin/ai6-stand-in && chmod 755 bin/ai6-stand-in",
                    ),
            );
        let client = TransportClient::new(Box::new(ShellTransport::new(home.path(), None)));
        let statuses = |steps: &[ProvisionStep]| {
            steps
                .iter()
                .map(|step| (step.name.clone(), step.status))
                .collect::<Vec<_>>()
        };

        // Checking an unfit Theater says what would change and writes nothing.
        let before = snapshot(home.path());
        let checked = provision(&client, &theater, true);
        assert_eq!(
            statuses(&checked),
            [
                ("disk".to_owned(), StepStatus::Ok),
                ("ai6-stand-in".to_owned(), StepStatus::WouldChange),
                ("runner".to_owned(), StepStatus::WouldChange),
            ],
            "{checked:#?}"
        );
        assert_eq!(snapshot(home.path()), before);

        let first = provision(&client, &theater, false);
        assert_eq!(
            statuses(&first),
            [
                ("disk".to_owned(), StepStatus::Ok),
                ("ai6-stand-in".to_owned(), StepStatus::Changed),
                ("runner".to_owned(), StepStatus::Changed),
            ],
            "{first:#?}"
        );
        let installed = home.path().join("opt/ai6/bin/ai6");
        assert_eq!(
            std::fs::read(&installed).unwrap(),
            std::fs::read(&build).unwrap()
        );
        assert!(!home.path().join("opt/ai6/bin/ai6.ai6-partial").exists());

        let second = provision(&client, &theater, false);
        assert!(
            second.iter().all(|step| step.status == StepStatus::Ok),
            "{second:#?}"
        );

        // A fit Theater checks all ok, and the check writes nothing.
        let before = snapshot(home.path());
        let checked = provision(&client, &theater, true);
        assert!(checked.iter().all(|step| step.status == StepStatus::Ok));
        assert_eq!(snapshot(home.path()), before);

        // A changed build is copied again; a program with no install command fails.
        std::fs::write(&build, b"#!/bin/sh\necho ai6 newer build\n").unwrap();
        let mut stricter = theater.clone();
        stricter.requires = Some(Requirements::default().with_tool("ai6-absent".parse().unwrap()));
        let steps = provision(&client, &stricter, false);
        assert_eq!(
            statuses(&steps),
            [
                ("ai6-absent".to_owned(), StepStatus::Failed),
                ("runner".to_owned(), StepStatus::Changed),
            ]
        );
        assert!(steps[0].detail.contains("provision.install.ai6-absent"));
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

/// A transport over a directory, standing in for a host that only a transport reaches: a
/// path such as `/work/a.txt` is `<root>/work/a.txt` on the Station's disk, and a command is
/// recorded rather than run.
#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use guatiao::library::ProviderError;
    use guatiao::{Bytes, FromValue, Status, ToValue, Value};

    use crate::directorates::q::transport::{
        Capabilities, DirEntry, DirListing, FileData, Output, ReadSpec, RunSpec, Transport,
        WriteSpec,
    };
    #[cfg(unix)]
    use crate::directorates::q::transport::{ForwardInfo, ForwardSpec, TransportForward};

    pub(crate) struct DirTransport {
        root: PathBuf,
        files: bool,
        runs: Arc<Mutex<Vec<RunSpec>>>,
    }

    impl DirTransport {
        pub(crate) fn new(root: &Path) -> Self {
            Self {
                root: root.to_path_buf(),
                files: true,
                runs: Arc::new(Mutex::new(Vec::new())),
            }
        }

        pub(crate) fn without_files(mut self) -> Self {
            self.files = false;
            self
        }

        /// The commands run so far.
        pub(crate) fn runs(&self) -> Arc<Mutex<Vec<RunSpec>>> {
            self.runs.clone()
        }

        pub(crate) fn capabilities_model(&self) -> Capabilities {
            Capabilities {
                run: true,
                files: self.files,
                os: Some("linux".to_owned()),
                ..Capabilities::default()
            }
        }

        fn path(&self, path: &str) -> PathBuf {
            self.root.join(path.trim_start_matches('/'))
        }
    }

    fn internal(error: impl std::fmt::Display) -> ProviderError {
        ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
    }

    fn io(error: &std::io::Error) -> ProviderError {
        let status = if error.kind() == std::io::ErrorKind::NotFound {
            Status::GUATIAO_ERR_NOT_FOUND
        } else {
            Status::GUATIAO_ERR_INTERNAL
        };
        ProviderError::new(status, &error.to_string())
    }

    impl Transport for DirTransport {
        fn capabilities(&self) -> Result<Value, ProviderError> {
            self.capabilities_model().to_value().map_err(internal)
        }

        fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
            let spec = RunSpec::from_value(spec).map_err(internal)?;
            let stdout = format!("ran {}", spec.argv.join(" "));
            self.runs.lock().unwrap().push(spec);
            Output {
                code: Some(0),
                stdout: Bytes(stdout.into_bytes()),
                ..Output::default()
            }
            .to_value()
            .map_err(internal)
        }

        fn read_file(&self, spec: &Value) -> Result<Value, ProviderError> {
            let spec = ReadSpec::from_value(spec).map_err(internal)?;
            let bytes = std::fs::read(self.path(&spec.path)).map_err(|e| io(&e))?;
            let start = usize::try_from(spec.offset)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            let end = start
                .saturating_add(usize::try_from(spec.limit).unwrap_or(usize::MAX))
                .min(bytes.len());
            FileData {
                data: Bytes(bytes[start..end].to_vec()),
                size: bytes.len() as u64,
            }
            .to_value()
            .map_err(internal)
        }

        fn write_file(&self, spec: &Value) -> Result<(), ProviderError> {
            let spec = WriteSpec::from_value(spec).map_err(internal)?;
            let path = self.path(&spec.path);
            if spec.create_dirs
                && let Some(parent) = path.parent()
            {
                std::fs::create_dir_all(parent).map_err(|e| io(&e))?;
            }
            if spec.append {
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path)
                    .and_then(|mut file| file.write_all(&spec.data.0))
                    .map_err(|e| io(&e))
            } else {
                std::fs::write(path, &spec.data.0).map_err(|e| io(&e))
            }
        }

        fn list_dir(&self, path: &str) -> Result<Value, ProviderError> {
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(self.path(path)).map_err(|e| io(&e))? {
                let entry = entry.map_err(|e| io(&e))?;
                let metadata = entry.metadata().map_err(|e| io(&e))?;
                entries.push(DirEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    kind: if metadata.is_dir() { "dir" } else { "file" }.to_owned(),
                    size: if metadata.is_dir() { 0 } else { metadata.len() },
                });
            }
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            DirListing { entries }.to_value().map_err(internal)
        }
    }

    /// A POSIX host reached through a link that a test can drop: commands run in `sh` with a
    /// home directory, files are local, a forward is a local TCP relay, and
    /// `ai6 runner call` reaches the runner serving `runner_dir` as the command would.
    #[cfg(unix)]
    pub(crate) struct ShellTransport {
        home: PathBuf,
        runner_dir: Option<PathBuf>,
        down: Arc<std::sync::atomic::AtomicBool>,
        forwards: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[cfg(unix)]
    impl ShellTransport {
        pub(crate) fn new(home: &Path, runner_dir: Option<&Path>) -> Self {
            Self {
                home: home.to_path_buf(),
                runner_dir: runner_dir.map(Path::to_path_buf),
                down: Arc::default(),
                forwards: Arc::default(),
            }
        }

        /// While set, every operation fails as a dropped link does.
        pub(crate) fn link(&self) -> Arc<std::sync::atomic::AtomicBool> {
            self.down.clone()
        }

        /// How many forwards have been opened.
        pub(crate) fn forwards(&self) -> Arc<std::sync::atomic::AtomicUsize> {
            self.forwards.clone()
        }

        fn check(&self) -> Result<(), ProviderError> {
            if self.down.load(std::sync::atomic::Ordering::Acquire) {
                return Err(internal("the link is down"));
            }
            Ok(())
        }

        fn path(&self, path: &str) -> PathBuf {
            let path = Path::new(path);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.home.join(path)
            }
        }

        fn runner_call(&self, spec: &RunSpec) -> Output {
            let mut words = spec.argv[3..].iter();
            let mut state_dir = self.runner_dir.clone().unwrap_or_default();
            let mut rest = Vec::new();
            while let Some(word) = words.next() {
                if word == "--state-dir" {
                    state_dir = PathBuf::from(words.next().cloned().unwrap_or_default());
                } else {
                    rest.push(word.clone());
                }
            }
            let body = spec
                .stdin
                .as_ref()
                .filter(|stdin| !stdin.0.is_empty())
                .map(|stdin| serde_json::from_slice::<serde_json::Value>(&stdin.0).unwrap());
            match crate::station::runner::call(&state_dir, &rest[0], &rest[1], body.as_ref()) {
                Ok((status, body)) => Output {
                    code: Some(0),
                    stdout: Bytes(
                        serde_json::json!({ "status": status, "body": body })
                            .to_string()
                            .into_bytes(),
                    ),
                    ..Output::default()
                },
                Err(error) => Output {
                    code: Some(i64::from(crate::station::runner::CALL_NOT_RUNNING)),
                    stderr: Bytes(error.into_bytes()),
                    ..Output::default()
                },
            }
        }
    }

    #[cfg(unix)]
    impl Transport for ShellTransport {
        fn capabilities(&self) -> Result<Value, ProviderError> {
            Capabilities {
                run: true,
                files: true,
                forward: true,
                os: Some(std::env::consts::OS.to_owned()),
                ..Capabilities::default()
            }
            .to_value()
            .map_err(internal)
        }

        fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
            self.check()?;
            let spec = RunSpec::from_value(spec).map_err(internal)?;
            if spec
                .argv
                .get(..3)
                .is_some_and(|words| words == ["ai6", "runner", "call"])
            {
                return self.runner_call(&spec).to_value().map_err(internal);
            }
            let line = crate::directorates::q::transport::kit::command_line(
                &spec.argv,
                spec.cwd.as_deref(),
                spec.env.as_ref(),
            )
            .map_err(internal)?;
            let mut command = std::process::Command::new("sh");
            // The host's own home, with its `bin` first on the PATH as a login shell has it.
            let path = std::env::var("PATH").unwrap_or_default();
            command
                .arg("-c")
                .arg(line)
                .current_dir(&self.home)
                .env("HOME", &self.home)
                .env("PATH", format!("{}/bin:{path}", self.home.display()))
                .env_remove("SSH_AUTH_SOCK");
            crate::directorates::q::transport::kit::run_process(
                &mut command,
                &crate::directorates::q::transport::kit::ProcessInput {
                    stdin: spec.stdin.as_ref().map(|stdin| stdin.0.as_slice()),
                    timeout_ms: spec.timeout_ms,
                    max_output: spec.max_output,
                },
                std::time::Duration::from_millis(
                    crate::directorates::q::transport::kit::DEFAULT_READER_GRACE_MS,
                ),
            )?
            .to_value()
            .map_err(internal)
        }

        fn read_file(&self, spec: &Value) -> Result<Value, ProviderError> {
            self.check()?;
            let spec = ReadSpec::from_value(spec).map_err(internal)?;
            let bytes = std::fs::read(self.path(&spec.path)).map_err(|e| io(&e))?;
            let start = usize::try_from(spec.offset)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            let end = start
                .saturating_add(usize::try_from(spec.limit).unwrap_or(usize::MAX))
                .min(bytes.len());
            FileData {
                data: Bytes(bytes[start..end].to_vec()),
                size: bytes.len() as u64,
            }
            .to_value()
            .map_err(internal)
        }

        fn write_file(&self, spec: &Value) -> Result<(), ProviderError> {
            self.check()?;
            let spec =
                crate::directorates::q::transport::WriteSpec::from_value(spec).map_err(internal)?;
            let path = self.path(&spec.path);
            if spec.create_dirs
                && let Some(parent) = path.parent()
            {
                std::fs::create_dir_all(parent).map_err(|e| io(&e))?;
            }
            std::fs::write(path, &spec.data.0).map_err(|e| io(&e))
        }

        fn forward(
            &self,
            spec: &Value,
        ) -> Result<guatiao::library::Object<dyn TransportForward>, ProviderError> {
            self.check()?;
            let spec = ForwardSpec::from_value(spec).map_err(internal)?;
            let listener =
                std::net::TcpListener::bind(("127.0.0.1", spec.remote_port)).map_err(|e| io(&e))?;
            listener.set_nonblocking(true).map_err(|e| io(&e))?;
            let port = listener.local_addr().map_err(|e| io(&e))?.port();
            let open = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let running = open.clone();
            let target = spec.target.clone();
            let listening = std::thread::spawn(move || {
                while running.load(std::sync::atomic::Ordering::Acquire) {
                    let Ok((inbound, _)) = listener.accept() else {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    };
                    let _ = inbound.set_nonblocking(false);
                    let Ok(outbound) = std::net::TcpStream::connect(&target) else {
                        continue;
                    };
                    let (mut in_read, mut out_write) =
                        (inbound.try_clone().unwrap(), outbound.try_clone().unwrap());
                    std::thread::spawn(move || std::io::copy(&mut in_read, &mut out_write));
                    let (mut out_read, mut in_write) = (outbound, inbound);
                    std::thread::spawn(move || std::io::copy(&mut out_read, &mut in_write));
                }
            });
            self.forwards
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Ok(LocalForward {
                info: ForwardInfo {
                    remote_host: "127.0.0.1".to_owned(),
                    remote_port: port,
                },
                open,
                listening: Mutex::new(Some(listening)),
            }
            .into_object())
        }
    }

    #[cfg(unix)]
    struct LocalForward {
        info: ForwardInfo,
        open: Arc<std::sync::atomic::AtomicBool>,
        listening: Mutex<Option<std::thread::JoinHandle<()>>>,
    }

    #[cfg(unix)]
    impl TransportForward for LocalForward {
        fn info(&self) -> Result<Value, ProviderError> {
            self.info.to_value().map_err(internal)
        }

        /// Returns once the port is free again, as a host's forward does.
        fn close(&self) -> Result<(), ProviderError> {
            self.open.store(false, std::sync::atomic::Ordering::Release);
            if let Some(listening) = self.listening.lock().unwrap().take() {
                let _ = listening.join();
            }
            Ok(())
        }
    }

    #[cfg(unix)]
    impl Drop for LocalForward {
        fn drop(&mut self) {
            self.open.store(false, std::sync::atomic::Ordering::Release);
        }
    }
}
