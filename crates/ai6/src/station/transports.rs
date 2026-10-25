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
    #[cfg_attr(
        not(feature = "cables-mcp"),
        allow(dead_code, reason = "only the Theater tools use it")
    )]
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
            command.arg("-c").arg(line).current_dir(&self.home);
            crate::directorates::q::transport::kit::run_process(
                &mut command,
                &crate::directorates::q::transport::kit::ProcessInput {
                    stdin: spec.stdin.as_ref().map(|stdin| stdin.0.as_slice()),
                    timeout_ms: spec.timeout_ms,
                    max_output: spec.max_output,
                },
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
