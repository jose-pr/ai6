// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The transport contract, as Guatiao kinds: how a Station reaches a Theater.
//!
//! A [`Transport`] is one way into one host: the Station's own machine (`local`), an SSH
//! server, a container, a device. It is built from a Theater's access settings and
//! declares what it can do in [`Capabilities`]; an operation it cannot do answers
//! `GUATIAO_ERR_UNSUPPORTED`, so a caller checks the capability first. Slot order is the
//! ABI: required slots come first and a new slot is appended with a default body.
//!
//! The Terminal kind stays what a transport opens: [`Transport::open_terminal`] hands back
//! the same [`TerminalSession`] a Terminal provider does.
//!
//! Provider calls block; call them from `spawn_blocking`, never on an async worker.

use std::collections::BTreeMap;

use guatiao::library::{Object, ProviderError};
use guatiao::{Bytes, FromValue, Schema, Status, ToValue, Value};
use serde::{Deserialize, Serialize};

use crate::directorates::q::terminal::TerminalSession;

/// Output a [`Transport::run`] keeps per stream unless the spec asks for another limit.
pub const DEFAULT_MAX_OUTPUT: u64 = 1024 * 1024;

/// Bytes a [`Transport::read_file`] returns unless the spec asks for another limit.
pub const DEFAULT_READ_LIMIT: u64 = 1024 * 1024;

/// What a transport can do on the host it reaches.
///
/// `os` and `arch` are what the host reports (`linux`, `windows`, `macos`; `x86_64`,
/// `aarch64`); a transport that has not looked yet leaves them empty.
#[derive(
    ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq,
)]
#[map(crate = ::guatiao)]
pub struct Capabilities {
    /// [`Transport::open_terminal`] works.
    #[map(default)]
    #[serde(default)]
    pub terminal: bool,
    /// [`Transport::run`] works.
    #[map(default)]
    #[serde(default)]
    pub run: bool,
    /// [`Transport::read_file`], [`Transport::write_file`] and [`Transport::list_dir`] work.
    #[map(default)]
    #[serde(default)]
    pub files: bool,
    /// [`Transport::forward`] works.
    #[map(default)]
    #[serde(default)]
    pub forward: bool,
    /// The host's operating system, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// The host's CPU architecture, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// Facts an operator should see, such as the host key trusted on first use.
    #[map(default)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Settings a Theater passes to the transport that reaches it. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct TransportConfig {
    /// The Theater's access settings, serialized as TOML. The provider parses and
    /// validates the text itself and refuses unknown keys.
    pub settings_toml: String,
}

/// A command to run to completion. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct RunSpec {
    /// The program and its arguments, one element each; never re-split by a shell.
    pub argv: Vec<String>,
    /// Working directory on the host; absent uses the transport's default.
    pub cwd: Option<String>,
    /// Environment variables added to the host's own.
    pub env: Option<BTreeMap<String, String>>,
    /// Bytes written to the program's standard input before it is closed.
    pub stdin: Option<Bytes>,
    /// Milliseconds before the program is killed; `0` waits for it.
    #[map(default)]
    pub timeout_ms: u64,
    /// Bytes of each output stream kept; the rest is dropped and reported as truncated.
    #[map(default = DEFAULT_MAX_OUTPUT)]
    pub max_output: u64,
}

impl Default for RunSpec {
    /// An empty command that keeps [`DEFAULT_MAX_OUTPUT`] bytes of each stream.
    fn default() -> Self {
        Self {
            argv: Vec::new(),
            cwd: None,
            env: None,
            stdin: None,
            timeout_ms: 0,
            max_output: DEFAULT_MAX_OUTPUT,
        }
    }
}

/// What a finished (or timed-out) command left behind. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Default)]
#[map(crate = ::guatiao)]
pub struct Output {
    /// The exit code; absent when a signal ended the program or it was killed.
    pub code: Option<i64>,
    /// The signal number that ended the program, when the host reports one.
    pub signal: Option<i64>,
    /// Standard output, up to the spec's limit.
    pub stdout: Bytes,
    /// Standard error, up to the spec's limit.
    pub stderr: Bytes,
    /// The timeout expired and the program was killed.
    #[map(default)]
    pub timed_out: bool,
    /// An output stream was longer than the limit.
    #[map(default)]
    pub truncated: bool,
}

/// A read of part of a file. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct ReadSpec {
    /// Path on the host; a relative path is resolved against the transport's default
    /// directory.
    pub path: String,
    /// First byte to read.
    #[map(default)]
    pub offset: u64,
    /// Most bytes to return.
    #[map(default = DEFAULT_READ_LIMIT)]
    pub limit: u64,
}

/// Part of a file. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Default)]
#[map(crate = ::guatiao)]
pub struct FileData {
    /// The bytes read.
    pub data: Bytes,
    /// The whole file's size in bytes.
    pub size: u64,
}

/// A write of bytes to a file. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct WriteSpec {
    /// Path on the host; a relative path is resolved against the transport's default
    /// directory.
    pub path: String,
    /// The bytes to write.
    pub data: Bytes,
    /// Add to the end instead of replacing the file.
    #[map(default)]
    pub append: bool,
    /// Create missing parent directories.
    #[map(default)]
    pub create_dirs: bool,
}

/// One entry of a directory. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct DirEntry {
    /// The entry's name, without its directory.
    pub name: String,
    /// `file`, `dir`, `link` or `other`.
    pub kind: String,
    /// Size in bytes; `0` for a directory.
    pub size: u64,
}

/// The entries of a directory, sorted by name. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Default)]
#[map(crate = ::guatiao)]
pub struct DirListing {
    /// The entries.
    pub entries: Vec<DirEntry>,
}

/// A reverse forward: a port on the host that reaches an address on the Station. Crosses as
/// a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct ForwardSpec {
    /// The address the host listens on.
    #[map(default = default_forward_host())]
    pub remote_host: String,
    /// The port the host listens on; `0` lets the host choose.
    #[map(default)]
    pub remote_port: u16,
    /// Where connections to the host's port are sent on the Station, as `host:port`.
    pub target: String,
}

fn default_forward_host() -> String {
    "127.0.0.1".to_owned()
}

/// Where an open forward listens. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct ForwardInfo {
    /// The address the host listens on.
    pub remote_host: String,
    /// The port the host listens on.
    pub remote_port: u16,
}

/// One way into one host.
#[guatiao::kind(name = "ai6_transport")]
pub trait Transport: Send + Sync {
    /// Answers the [`Capabilities`] as a value, without contacting the host.
    fn capabilities(&self) -> Result<Value, ProviderError>;

    /// Contacts the host and answers its [`Capabilities`] with `os` and `arch` filled in
    /// when it can tell. Defaults to [`Transport::capabilities`].
    fn probe(&self) -> Result<Value, ProviderError> {
        self.capabilities()
    }

    /// Starts `spec` (a Terminal [`Spec`](crate::directorates::q::terminal::Spec) as a
    /// value) on a terminal on the host.
    fn open_terminal(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let _ = spec;
        Err(unsupported("a terminal"))
    }

    /// Runs `spec` (a [`RunSpec`] as a value) to completion and answers an [`Output`].
    fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
        let _ = spec;
        Err(unsupported("running a command"))
    }

    /// Reads `spec` (a [`ReadSpec`] as a value) and answers a [`FileData`].
    fn read_file(&self, spec: &Value) -> Result<Value, ProviderError> {
        let _ = spec;
        Err(unsupported("reading a file"))
    }

    /// Writes `spec` (a [`WriteSpec`] as a value).
    fn write_file(&self, spec: &Value) -> Result<(), ProviderError> {
        let _ = spec;
        Err(unsupported("writing a file"))
    }

    /// Lists the directory `path` and answers a [`DirListing`].
    fn list_dir(&self, path: &str) -> Result<Value, ProviderError> {
        let _ = path;
        Err(unsupported("listing a directory"))
    }

    /// Opens `spec` (a [`ForwardSpec`] as a value); the forward lasts until the handle is
    /// closed or dropped.
    fn forward(&self, spec: &Value) -> Result<Object<dyn TransportForward>, ProviderError> {
        let _ = spec;
        Err(unsupported("a forward"))
    }
}

/// One open reverse forward. Dropping the handle closes it.
#[guatiao::kind(object, name = "ai6_transport_forward")]
pub trait TransportForward: Send + Sync {
    /// Answers the [`ForwardInfo`] as a value.
    fn info(&self) -> Result<Value, ProviderError>;
    /// Stops listening. Idempotent.
    fn close(&self) -> Result<(), ProviderError>;
}

/// The refusal for an operation a transport does not do.
pub fn unsupported(what: &str) -> ProviderError {
    ProviderError::new(
        Status::GUATIAO_ERR_UNSUPPORTED,
        &format!("this transport does not support {what}"),
    )
}

/// What a provider said went wrong, in words an operator can read.
pub fn describe(error: &ProviderError) -> String {
    match error.message() {
        "" => error.to_string(),
        message => message.to_owned(),
    }
}

/// Whether `error` is the refusal for an operation the transport does not do.
pub fn is_unsupported(error: &ProviderError) -> bool {
    error.status == Status::GUATIAO_ERR_UNSUPPORTED
}

/// A transport with its models spelled as Rust types.
pub struct TransportClient {
    transport: Box<dyn Transport>,
}

impl TransportClient {
    /// Wraps a built transport.
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self { transport }
    }

    /// What the transport can do, without contacting the host.
    pub fn capabilities(&self) -> Result<Capabilities, ProviderError> {
        decode(&self.transport.capabilities()?)
    }

    /// What the transport can do, after contacting the host.
    pub fn probe(&self) -> Result<Capabilities, ProviderError> {
        decode(&self.transport.probe()?)
    }

    /// Starts a program on a terminal on the host.
    pub fn open_terminal(
        &self,
        spec: &crate::directorates::q::terminal::Spec,
    ) -> Result<Object<dyn TerminalSession>, ProviderError> {
        self.transport.open_terminal(&encode(spec)?)
    }

    /// Runs a command to completion.
    pub fn run(&self, spec: &RunSpec) -> Result<Output, ProviderError> {
        decode(&self.transport.run(&encode(spec)?)?)
    }

    /// Reads part of a file.
    pub fn read_file(&self, spec: &ReadSpec) -> Result<FileData, ProviderError> {
        decode(&self.transport.read_file(&encode(spec)?)?)
    }

    /// Writes bytes to a file.
    pub fn write_file(&self, spec: &WriteSpec) -> Result<(), ProviderError> {
        self.transport.write_file(&encode(spec)?)
    }

    /// Lists a directory.
    pub fn list_dir(&self, path: &str) -> Result<DirListing, ProviderError> {
        decode(&self.transport.list_dir(path)?)
    }

    /// Opens a reverse forward.
    pub fn forward(&self, spec: &ForwardSpec) -> Result<Forward, ProviderError> {
        let handle = self.transport.forward(&encode(spec)?)?;
        let info = decode(&handle.info()?)?;
        Ok(Forward { handle, info })
    }
}

/// An open reverse forward, closed when dropped.
pub struct Forward {
    handle: Object<dyn TransportForward>,
    info: ForwardInfo,
}

impl Forward {
    /// Where the host listens.
    pub fn info(&self) -> &ForwardInfo {
        &self.info
    }

    /// Stops listening. Idempotent.
    pub fn close(&self) -> Result<(), ProviderError> {
        self.handle.close()
    }
}

fn encode(model: &impl ToValue) -> Result<Value, ProviderError> {
    model
        .to_value()
        .map_err(|error| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &error.to_string()))
}

fn decode<T: FromValue>(value: &Value) -> Result<T, ProviderError> {
    T::from_value(value)
        .map_err(|error| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &error.to_string()))
}

/// The contract suite every transport passes, written once beside the kind.
pub mod contract {
    use super::*;

    /// How the suite drives a host: which shell it has and where it may write.
    pub struct Host<'a> {
        /// The argv prefix that runs one shell command string (`["sh", "-c"]`).
        pub shell: &'a [&'a str],
        /// The shell syntax that expands the environment variable `name`.
        pub expand: fn(name: &str) -> String,
        /// An existing, empty directory the suite may fill.
        pub scratch: &'a str,
    }

    impl Host<'_> {
        fn argv(&self, command: &str) -> Vec<String> {
            self.shell
                .iter()
                .map(|part| (*part).to_owned())
                .chain([command.to_owned()])
                .collect()
        }

        fn join(&self, name: &str) -> String {
            format!("{}/{name}", self.scratch.trim_end_matches(['/', '\\']))
        }
    }

    /// Checks `transport` against its own [`Capabilities`]: what it claims works, and what
    /// it does not claim answers `GUATIAO_ERR_UNSUPPORTED`. Panics with the failed check.
    pub fn check(transport: Box<dyn Transport>, host: &Host<'_>) {
        let client = TransportClient::new(transport);
        let capabilities = client.capabilities().expect("capabilities");
        let probed = client.probe().expect("probe");
        assert_eq!(
            (probed.terminal, probed.run, probed.files, probed.forward),
            (
                capabilities.terminal,
                capabilities.run,
                capabilities.files,
                capabilities.forward
            ),
            "probing must not change what the transport claims"
        );
        if capabilities.run {
            check_run(&client, host);
        } else {
            let refused = client.run(&RunSpec {
                argv: host.argv("exit 0"),
                ..RunSpec::default()
            });
            assert!(refused.is_err_and(|e| is_unsupported(&e)), "run refusal");
        }
        if capabilities.files {
            check_files(&client, host);
        } else {
            let refused = client.list_dir(host.scratch);
            assert!(refused.is_err_and(|e| is_unsupported(&e)), "files refusal");
        }
        if !capabilities.forward {
            let refused = client.forward(&ForwardSpec {
                remote_host: "127.0.0.1".to_owned(),
                remote_port: 0,
                target: "127.0.0.1:1".to_owned(),
            });
            assert!(
                refused.is_err_and(|e| is_unsupported(&e)),
                "forward refusal"
            );
        }
        if !capabilities.terminal {
            let refused = client.open_terminal(&crate::directorates::q::terminal::Spec {
                argv: host.argv("exit 0"),
                cwd: None,
                env: None,
                rows: 24,
                cols: 80,
            });
            assert!(
                refused.is_err_and(|e| is_unsupported(&e)),
                "terminal refusal"
            );
        }
    }

    fn check_run(client: &TransportClient, host: &Host<'_>) {
        let run = |command: &str, edit: &dyn Fn(&mut RunSpec)| {
            let mut spec = RunSpec {
                argv: host.argv(command),
                ..RunSpec::default()
            };
            edit(&mut spec);
            client.run(&spec).expect("run")
        };

        let out = run("echo contract-out", &|_| {});
        assert_eq!(out.code, Some(0));
        assert!(String::from_utf8_lossy(&out.stdout.0).contains("contract-out"));
        assert!(!out.timed_out && !out.truncated);

        let out = run("echo contract-err 1>&2 && exit 3", &|_| {});
        assert_eq!(out.code, Some(3), "exit code");
        assert!(String::from_utf8_lossy(&out.stderr.0).contains("contract-err"));
        assert!(!String::from_utf8_lossy(&out.stdout.0).contains("contract-err"));

        let command = format!("echo {}", (host.expand)("AI6_CONTRACT"));
        let out = run(&command, &|spec| {
            spec.env = Some(BTreeMap::from([(
                "AI6_CONTRACT".to_owned(),
                "from-env".to_owned(),
            )]));
        });
        assert!(String::from_utf8_lossy(&out.stdout.0).contains("from-env"));

        let out = run("echo here", &|spec| {
            spec.cwd = Some(host.scratch.to_owned())
        });
        assert_eq!(out.code, Some(0), "cwd is honoured");

        let out = run("echo 0123456789", &|spec| spec.max_output = 4);
        assert!(out.truncated && out.stdout.0.len() == 4, "output limit");

        let out = run("exit 0", &|_| {});
        assert_eq!(out.code, Some(0));

        let missing = client.run(&RunSpec {
            argv: vec!["ai6-no-such-program".to_owned()],
            ..RunSpec::default()
        });
        let ran_and_failed = missing
            .as_ref()
            .is_ok_and(|out| out.code.is_some_and(|code| code != 0));
        assert!(
            missing.is_err() || ran_and_failed,
            "a missing program is an error or a non-zero exit"
        );
    }

    fn check_files(client: &TransportClient, host: &Host<'_>) {
        let path = host.join("contract.txt");
        let write = |data: &[u8], append: bool| {
            client
                .write_file(&WriteSpec {
                    path: path.clone(),
                    data: Bytes(data.to_vec()),
                    append,
                    create_dirs: false,
                })
                .expect("write")
        };
        write(b"hello ", false);
        write(b"world", true);
        let read = client
            .read_file(&ReadSpec {
                path: path.clone(),
                offset: 0,
                limit: DEFAULT_READ_LIMIT,
            })
            .expect("read");
        assert_eq!(read.data.0, b"hello world");
        assert_eq!(read.size, 11);

        let part = client
            .read_file(&ReadSpec {
                path: path.clone(),
                offset: 6,
                limit: 3,
            })
            .expect("partial read");
        assert_eq!(part.data.0, b"wor");
        assert_eq!(part.size, 11, "size is the whole file's");

        write(b"replaced", false);
        let read = client
            .read_file(&ReadSpec {
                path: path.clone(),
                offset: 0,
                limit: DEFAULT_READ_LIMIT,
            })
            .expect("read again");
        assert_eq!(read.data.0, b"replaced");

        let nested = host.join("a/b/c.bin");
        let binary: Vec<u8> = (0..=255).collect();
        let refused = client.write_file(&WriteSpec {
            path: nested.clone(),
            data: Bytes(binary.clone()),
            append: false,
            create_dirs: false,
        });
        assert!(refused.is_err(), "missing parent without create_dirs");
        client
            .write_file(&WriteSpec {
                path: nested.clone(),
                data: Bytes(binary.clone()),
                append: false,
                create_dirs: true,
            })
            .expect("write with parents");
        let read = client
            .read_file(&ReadSpec {
                path: nested,
                offset: 0,
                limit: DEFAULT_READ_LIMIT,
            })
            .expect("read binary");
        assert_eq!(read.data.0, binary, "bytes survive unchanged");

        let listing = client.list_dir(host.scratch).expect("list");
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind.as_str()))
            .collect();
        assert_eq!(names, [("a", "dir"), ("contract.txt", "file")]);
        assert_eq!(listing.entries[1].size, 8);

        let missing = client.read_file(&ReadSpec {
            path: host.join("absent.txt"),
            offset: 0,
            limit: 1,
        });
        assert!(
            missing.is_err_and(|e| e.status == Status::GUATIAO_ERR_NOT_FOUND),
            "a missing file is GUATIAO_ERR_NOT_FOUND"
        );
        assert!(client.list_dir(&host.join("absent")).is_err());
    }
}

#[cfg(test)]
mod tests {
    use guatiao::{Map, TryAsMut};

    use super::*;

    // A changed required signature affects every built transport.
    #[test]
    fn floor_hashes_are_pinned() {
        assert_eq!(TransportVtable::FLOOR_HASH, 3711047304);
        assert_eq!(TransportForwardVtable::FLOOR_HASH, 3937278426);
    }

    fn without(model: &impl ToValue, keys: &[&str]) -> Value {
        let mut value = model.to_value().unwrap();
        let map: &mut Map = value.try_as_mut().unwrap();
        for key in keys {
            map.remove(key);
        }
        value
    }

    // A model grows by optional or defaulted fields only: these are the keys a built
    // provider cannot do without.
    #[test]
    fn models_read_with_only_their_required_keys() {
        let run = RunSpec::default();
        let read = RunSpec::from_value(&without(
            &run,
            &["cwd", "env", "stdin", "timeout_ms", "max_output"],
        ))
        .unwrap();
        assert_eq!(read.max_output, DEFAULT_MAX_OUTPUT);
        assert_eq!(read.timeout_ms, 0);

        let spec = ReadSpec {
            path: "f".to_owned(),
            offset: 0,
            limit: 0,
        };
        let read = ReadSpec::from_value(&without(&spec, &["offset", "limit"])).unwrap();
        assert_eq!((read.offset, read.limit), (0, DEFAULT_READ_LIMIT));

        let spec = WriteSpec {
            path: "f".to_owned(),
            data: Bytes(vec![1]),
            append: true,
            create_dirs: true,
        };
        let read = WriteSpec::from_value(&without(&spec, &["append", "create_dirs"])).unwrap();
        assert!(!read.append && !read.create_dirs);

        let spec = ForwardSpec {
            remote_host: String::new(),
            remote_port: 9,
            target: "127.0.0.1:1".to_owned(),
        };
        let read =
            ForwardSpec::from_value(&without(&spec, &["remote_host", "remote_port"])).unwrap();
        assert_eq!(
            (read.remote_host.as_str(), read.remote_port),
            ("127.0.0.1", 0)
        );

        let read = Capabilities::from_value(&without(
            &Capabilities::default(),
            &["terminal", "run", "files", "forward"],
        ))
        .unwrap();
        assert_eq!(read, Capabilities::default());
    }

    #[test]
    fn specs_round_trip_through_a_value() {
        let spec = RunSpec {
            argv: vec!["ls".to_owned()],
            cwd: Some("/work".to_owned()),
            env: Some(BTreeMap::from([("A".to_owned(), "1".to_owned())])),
            stdin: Some(Bytes(vec![0, 255])),
            timeout_ms: 5,
            max_output: 10,
        };
        assert_eq!(
            RunSpec::from_value(&spec.to_value().unwrap()).unwrap(),
            spec
        );
        let output = Output {
            code: Some(3),
            signal: None,
            stdout: Bytes(vec![0, 1, 2]),
            stderr: Bytes(Vec::new()),
            timed_out: false,
            truncated: true,
        };
        assert_eq!(
            Output::from_value(&output.to_value().unwrap()).unwrap(),
            output
        );
    }

    struct Bare;

    impl Transport for Bare {
        fn capabilities(&self) -> Result<Value, ProviderError> {
            Capabilities::default().to_value().map_err(|error| {
                ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
            })
        }
    }

    #[test]
    fn a_transport_that_implements_nothing_else_refuses_everything_else() {
        let client = TransportClient::new(Box::new(Bare));
        assert_eq!(client.capabilities().unwrap(), Capabilities::default());
        assert_eq!(client.probe().unwrap(), Capabilities::default());
        assert!(
            client
                .run(&RunSpec::default())
                .is_err_and(|e| is_unsupported(&e))
        );
        assert!(client.list_dir(".").is_err_and(|e| is_unsupported(&e)));
    }

    #[test]
    fn the_contract_suite_accepts_a_transport_that_claims_nothing() {
        contract::check(
            Box::new(Bare),
            &contract::Host {
                shell: &["sh", "-c"],
                expand: |name| format!("${name}"),
                scratch: ".",
            },
        );
    }
}
