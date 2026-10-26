// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Q provisioning: what a Theater must have, what is on it, and fitting it out.
//!
//! A Theater declares what Sessions need there ([`Requirements`], `[theater.ID.requires]`)
//! and how to install what is missing ([`ProvisionSpec`], `[theater.ID.provision]`).
//! [`probe`] looks over the Theater's transport and reports what it found, including
//! credentials already on the host that AI6 did not lease ([`ProbeReport::ambient`]).
//! [`provision`] fits the Theater out step by step, checking before it acts, so a second run
//! on a fit Theater changes nothing.
//!
//! Every call here blocks on the transport; call it from `spawn_blocking`, never on an async
//! worker. Nothing here carries a secret: credentials reach a Session as leases, never as
//! files on the Theater.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use guatiao::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::directorates::m::placement::Resources;
use crate::directorates::m::theaters::Theater;
use crate::directorates::q::transport::{
    Capabilities, Output, RunSpec, TransportClient, WriteSpec, describe,
};

/// Seconds an install command may run unless the spec says otherwise.
pub const DEFAULT_INSTALL_TIMEOUT_SECS: u64 = 600;

const PROBE_TIMEOUT_MS: u64 = 60_000;
const COMMAND_TIMEOUT_MS: u64 = 120_000;
const MAX_NAME_BYTES: usize = 64;
const MAX_VERSION_CHARS: usize = 200;

/// A program a Theater must have, and the oldest version accepted: `claude>=2.1`, or just
/// `git` for any version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequiredProgram {
    name: String,
    minimum: Option<String>,
}

impl RequiredProgram {
    /// Any version of the program `name`, which is also the command run on the Theater.
    pub fn new(name: impl Into<String>) -> Result<Self, String> {
        let name = name.into();
        let valid = !name.is_empty()
            && name.len() <= MAX_NAME_BYTES
            && !name.starts_with(['-', '.'])
            && name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')
            });
        if !valid {
            return Err(format!(
                "{name:?} is not a program name: use 1 to {MAX_NAME_BYTES} letters, digits, '.', '_', '-' or '+'"
            ));
        }
        Ok(Self {
            name,
            minimum: None,
        })
    }

    /// Accepts only `minimum` or later, compared number by number (`2.10` is after `2.9`).
    pub fn at_least(mut self, minimum: impl Into<String>) -> Result<Self, String> {
        let minimum = minimum.into();
        let numeric = minimum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.');
        if !numeric || minimum.split('.').any(str::is_empty) {
            return Err(format!(
                "{minimum:?} is not a version: use numbers separated by '.'"
            ));
        }
        self.minimum = Some(minimum);
        Ok(self)
    }

    /// The program's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The oldest version accepted, when one is required.
    pub fn minimum(&self) -> Option<&str> {
        self.minimum.as_deref()
    }

    /// Why `version`, the first line the program printed for `--version`, does not satisfy
    /// this requirement; `None` when it does.
    pub fn refuses(&self, version: &str) -> Option<String> {
        let minimum = self.minimum.as_deref()?;
        let Some(found) = version_numbers(version) else {
            return Some(format!("its version could not be read from {version:?}"));
        };
        let wanted = version_numbers(minimum).unwrap_or_default();
        let length = found.len().max(wanted.len());
        let pad = |numbers: &[u64]| {
            let mut padded = numbers.to_vec();
            padded.resize(length, 0);
            padded
        };
        (pad(&found) < pad(&wanted))
            .then(|| format!("version {} is older than {minimum}", join_numbers(&found)))
    }
}

impl fmt::Display for RequiredProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.minimum {
            Some(minimum) => write!(f, "{}>={minimum}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

impl FromStr for RequiredProgram {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.split_once(">=") {
            Some((name, minimum)) => Self::new(name.trim())?.at_least(minimum.trim()),
            None => Self::new(text.trim()),
        }
    }
}

impl Serialize for RequiredProgram {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RequiredProgram {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The numbers of the first version-shaped word in `text` (`2.1.3` in
/// `claude 2.1.3 (Claude Code)`).
fn version_numbers(text: &str) -> Option<Vec<u64>> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let word: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    word.trim_end_matches('.')
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

fn join_numbers(numbers: &[u64]) -> String {
    numbers
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// What a Theater must have before Sessions run there: `[theater.ID.requires]`.
///
/// ```toml
/// [theater.build-box.requires]
/// os = "linux"
/// harnesses = ["claude>=2.1", "codex"]
/// tools = ["git", "cargo"]
/// disk_mb = 2048
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    /// The operating system, as transports report it: `linux`, `macos`, `freebsd`, `windows`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// Agent harness CLIs, named after their AgentHandler provider (`claude`, `codex`). A
    /// Theater that lists harnesses runs only those in field mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<RequiredProgram>,
    /// Other programs Sessions use there (`git`, `cargo`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<RequiredProgram>,
    /// Free space, in MiB, in the Theater's `workdir` (the transport's default directory when
    /// it has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_mb: Option<u64>,
}

impl Requirements {
    /// Requires the operating system `os`.
    #[must_use]
    pub fn with_os(mut self, os: impl Into<String>) -> Self {
        self.os = Some(os.into());
        self
    }

    /// Requires a harness CLI.
    #[must_use]
    pub fn with_harness(mut self, harness: RequiredProgram) -> Self {
        self.harnesses.push(harness);
        self
    }

    /// Requires another program.
    #[must_use]
    pub fn with_tool(mut self, tool: RequiredProgram) -> Self {
        self.tools.push(tool);
        self
    }

    /// Requires this much free space, in MiB.
    #[must_use]
    pub fn with_disk_mb(mut self, disk_mb: u64) -> Self {
        self.disk_mb = Some(disk_mb);
        self
    }

    /// Every program required, harnesses first.
    pub fn programs(&self) -> impl Iterator<Item = &RequiredProgram> {
        self.harnesses.iter().chain(&self.tools)
    }

    /// Whether `harness` is one the Theater lists, or the Theater lists none.
    pub fn allows_harness(&self, harness: &str) -> bool {
        self.harnesses.is_empty() || self.harnesses.iter().any(|h| h.name() == harness)
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Some(os) = &self.os
            && (os.is_empty() || !os.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        {
            return Err(format!(
                "requires.os {os:?} must be a word such as \"linux\""
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for program in self.programs() {
            if !seen.insert(program.name()) {
                return Err(format!(
                    "requires names program {:?} more than once",
                    program.name()
                ));
            }
        }
        Ok(())
    }

    /// The requirements `report` shows unmet, each with why.
    pub fn check(&self, report: &ProbeReport) -> Vec<Unmet> {
        let mut unmet = Vec::new();
        if let Some(os) = &self.os
            && report.os.as_deref() != Some(os.as_str())
        {
            unmet.push(Unmet {
                name: "os".to_owned(),
                requirement: format!("os {os}"),
                reason: match &report.os {
                    Some(found) => format!("the Theater runs {found}"),
                    None => "the Theater did not report its operating system".to_owned(),
                },
            });
        }
        for program in self.programs() {
            let reason = match report.found.get(program.name()) {
                None => Some("not installed".to_owned()),
                Some(version) => program.refuses(version),
            };
            if let Some(reason) = reason {
                unmet.push(Unmet {
                    name: program.name().to_owned(),
                    requirement: program.to_string(),
                    reason,
                });
            }
        }
        if let Some(disk_mb) = self.disk_mb {
            let reason = match report.free_mb {
                Some(free) if free >= disk_mb => None,
                Some(free) => Some(format!("only {free} MiB free")),
                None => Some("free space could not be read".to_owned()),
            };
            if let Some(reason) = reason {
                unmet.push(Unmet {
                    name: "disk".to_owned(),
                    requirement: format!("{disk_mb} MiB free"),
                    reason,
                });
            }
        }
        unmet
    }
}

/// How to fit a Theater out: `[theater.ID.provision]`. It holds paths and commands only;
/// credentials are leased per Session and never installed.
///
/// ```toml
/// [theater.build-box.provision]
/// runner = "dist/linux-aarch64/ai6"
/// plugins = ["dist/linux-aarch64/libai6_agent_claude.so"]
/// install = { git = "sudo dnf install -y git", claude = "npm install -g @anthropic-ai/claude-code" }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionSpec {
    /// The `ai6` build to install as the Theater's runner (at its `runner.command`), a path on
    /// the Station. When absent, the Station's own executable, if the Theater has the
    /// Station's operating system and architecture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    /// Provider libraries the runner loads, paths on the Station, copied into the first
    /// directory of the runner's `plugin_paths`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<String>,
    /// The shell command that installs each program, by name. It runs with `sh -c` on the
    /// Theater only when the program is missing or too old.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub install: BTreeMap<String, String>,
    /// Seconds an install command may run; [`DEFAULT_INSTALL_TIMEOUT_SECS`] when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_timeout_secs: Option<u64>,
}

impl ProvisionSpec {
    /// Installs the runner from `path` on the Station.
    #[must_use]
    pub fn with_runner(mut self, path: impl Into<String>) -> Self {
        self.runner = Some(path.into());
        self
    }

    /// Copies a provider library for the runner.
    #[must_use]
    pub fn with_plugin(mut self, path: impl Into<String>) -> Self {
        self.plugins.push(path.into());
        self
    }

    /// Installs `program` with the shell command `command` when it is missing or too old.
    #[must_use]
    pub fn with_install(mut self, program: impl Into<String>, command: impl Into<String>) -> Self {
        self.install.insert(program.into(), command.into());
        self
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        let text = |value: &str| !value.trim().is_empty() && !value.contains('\0');
        if self.runner.as_deref().is_some_and(|path| !text(path))
            || self.plugins.iter().any(|path| !text(path))
        {
            return Err("provision paths must be non-empty text".to_owned());
        }
        for (program, command) in &self.install {
            RequiredProgram::new(program.clone())?;
            if !text(command) {
                return Err(format!(
                    "provision.install.{program} must be a non-empty command"
                ));
            }
        }
        if self.install_timeout_secs == Some(0) {
            return Err("provision.install_timeout_secs must be at least 1".to_owned());
        }
        Ok(())
    }
}

/// One requirement a Theater does not meet.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unmet {
    /// The program's name, or `os` or `disk`.
    pub name: String,
    /// The requirement as written: `claude>=2.1`, `os linux`, `2048 MiB free`.
    pub requirement: String,
    /// Why it is not met, in words an operator can act on.
    pub reason: String,
}

impl fmt::Display for Unmet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.requirement, self.reason)
    }
}

/// What a probe found on a Theater. It names where ambient credentials are, never their
/// values, so a Station publishes it as it is.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    /// When it was taken, as Unix seconds.
    pub checked_at: u64,
    /// The operating system: what the transport reported, else what `uname` said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// Programs found, by name, with the first line each printed for `--version`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub found: BTreeMap<String, String>,
    /// Free MiB in the working directory, when the Theater reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_mb: Option<u64>,
    /// The Theater's CPUs, load and memory, within its container's limits when it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Resources>,
    /// Requirements the Theater does not meet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmet: Vec<Unmet>,
    /// Credentials already on the Theater outside AI6's leases (a credential helper, a login,
    /// an agent socket): a weakening, not an error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambient: Vec<String>,
    /// Why the probe could not finish; the other fields are then empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProbeReport {
    /// A report that says only why the Theater could not be probed.
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            checked_at: unix_now(),
            error: Some(error.into()),
            ..Self::default()
        }
    }

    /// Whether the probe finished and found every requirement met.
    pub fn is_fit(&self) -> bool {
        self.error.is_none() && self.unmet.is_empty()
    }
}

/// Lists programs with their versions, free space and ambient credentials, one tab-separated
/// record a line. Names never values: only whether a credential file or variable exists.
const PROBE: &str = r#"ai6_dir=$1
shift
printf 'os\t%s\n' "$(uname -s 2>/dev/null)"
for ai6_name in "$@"; do
  if command -v "$ai6_name" >/dev/null 2>&1; then
    ai6_version=$("$ai6_name" --version </dev/null 2>/dev/null | head -n 1)
    printf 'found\t%s\t%s\n' "$ai6_name" "$ai6_version"
  else
    printf 'missing\t%s\n' "$ai6_name"
  fi
done
ai6_free=$(df -Pk "$ai6_dir" 2>/dev/null | awk 'NR == 2 { print $4 }')
[ -n "$ai6_free" ] && printf 'free_kb\t%s\n' "$ai6_free"
ai6_ambient() { printf 'ambient\t%s\n' "$1"; }
[ -s "$HOME/.git-credentials" ] && ai6_ambient 'a git credential store (~/.git-credentials)'
ai6_helper=$(git config --get credential.helper 2>/dev/null)
[ -n "$ai6_helper" ] && ai6_ambient "a git credential helper (${ai6_helper%% *})"
[ -n "$SSH_AUTH_SOCK" ] && ai6_ambient 'an SSH agent (SSH_AUTH_SOCK)'
for ai6_key in id_ed25519 id_ecdsa id_rsa; do
  [ -f "$HOME/.ssh/$ai6_key" ] && ai6_ambient "an SSH private key (~/.ssh/$ai6_key)"
done
[ -s "$HOME/.config/gh/hosts.yml" ] && ai6_ambient 'a GitHub CLI login (~/.config/gh/hosts.yml)'
[ -s "$HOME/.claude/.credentials.json" ] && ai6_ambient 'a Claude Code login (~/.claude/.credentials.json)'
[ -s "$HOME/.codex/auth.json" ] && ai6_ambient 'a Codex login (~/.codex/auth.json)'
for ai6_var in ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN OPENAI_API_KEY GH_TOKEN GITHUB_TOKEN; do
  eval "ai6_set=\${$ai6_var:+1}"
  [ -n "$ai6_set" ] && ai6_ambient "$ai6_var in the login environment"
done
"#;

/// Prints the CPUs, load average and memory, then a cgroup v2 container's own CPU and memory
/// limits, which a container's `/proc` does not show. macOS has no `/proc`: `sysctl` gives
/// its load and total memory, and nothing gives its available memory.
const RESOURCES: &str = r#"ai6_n=$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null)
[ -n "$ai6_n" ] && printf 'nproc\t%s\n' "$ai6_n"
if [ -r /proc/loadavg ]; then
  printf 'loadavg\t%s\n' "$(cat /proc/loadavg)"
else
  ai6_load=$(sysctl -n vm.loadavg 2>/dev/null) && printf 'loadavg\t%s\n' "$ai6_load"
fi
if [ -r /proc/meminfo ]; then
  awk '$1 == "MemTotal:" { printf "mem_total_kb\t%s\n", $2 }
       $1 == "MemAvailable:" { printf "mem_available_kb\t%s\n", $2 }' /proc/meminfo
else
  ai6_mem=$(sysctl -n hw.memsize 2>/dev/null) && printf 'mem_total_bytes\t%s\n' "$ai6_mem"
fi
for ai6_file in cpu.max memory.max memory.current; do
  [ -r "/sys/fs/cgroup/$ai6_file" ] && printf 'cgroup_%s\t%s\n' "$ai6_file" "$(cat "/sys/fs/cgroup/$ai6_file")"
done
exit 0
"#;

/// Prints the SHA-256 of a file, or nothing when it does not exist.
const DIGEST: &str = r#"[ -f "$1" ] || exit 0
{ sha256sum "$1" 2>/dev/null || shasum -a 256 "$1"; } | cut -d ' ' -f 1
"#;

/// Makes the file just written executable and moves it over the old one, which a running
/// runner may still be executing.
const PLACE: &str = r#"chmod 755 "$1" && mv -f "$1" "$2"
"#;

/// Looks over the Theater `client` reaches: the operating system, each program `requires`
/// names with its version, free space in `dir`, ambient credentials, and its CPUs, load and
/// memory as [`probe_resources`] reads them. `capabilities` are
/// what the transport reported; its `os` wins over `uname`.
///
/// Never fails: a Theater that cannot be probed answers a report with only
/// [`ProbeReport::error`] set.
pub fn probe(
    client: &TransportClient,
    capabilities: &Capabilities,
    requires: Option<&Requirements>,
    dir: Option<&str>,
) -> ProbeReport {
    let names: Vec<String> = requires
        .into_iter()
        .flat_map(Requirements::programs)
        .map(|program| program.name().to_owned())
        .collect();
    let mut report = match probe_programs(client, capabilities, &names, dir) {
        Ok(report) => report,
        Err(error) => return ProbeReport::failed(error),
    };
    if let Some(requires) = requires {
        report.unmet = requires.check(&report);
    }
    report
}

fn probe_programs(
    client: &TransportClient,
    capabilities: &Capabilities,
    names: &[String],
    dir: Option<&str>,
) -> Result<ProbeReport, String> {
    if !capabilities.run {
        return Err("its transport cannot run commands, so it cannot be probed".to_owned());
    }
    if capabilities.os.as_deref() == Some("windows") {
        return Err("probing needs a POSIX shell, and the Theater runs windows".to_owned());
    }
    let mut argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        [PROBE, RESOURCES].concat(),
        "sh".to_owned(),
        dir.unwrap_or(".").to_owned(),
    ];
    argv.extend(names.iter().cloned());
    let output = run(client, argv, PROBE_TIMEOUT_MS)?;
    if output.code != Some(0) {
        return Err(format!("the probe failed: {}", failure(&output)));
    }
    let stdout = String::from_utf8_lossy(&output.stdout.0);
    let mut report = ProbeReport {
        checked_at: unix_now(),
        os: capabilities.os.clone(),
        resources: parse_resources(&stdout),
        ..ProbeReport::default()
    };
    for line in stdout.lines() {
        let mut fields = line.splitn(3, '\t');
        match (fields.next(), fields.next(), fields.next()) {
            (Some("os"), Some(uname), _) if report.os.is_none() && !uname.is_empty() => {
                report.os = Some(os_name(uname));
            }
            (Some("found"), Some(name), version) if names.iter().any(|n| n == name) => {
                let version: String = version
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .take(MAX_VERSION_CHARS)
                    .collect();
                report.found.insert(name.to_owned(), version);
            }
            (Some("free_kb"), Some(kb), _) => {
                report.free_mb = kb.trim().parse::<u64>().ok().map(|kb| kb / 1024);
            }
            (Some("ambient"), Some(what), _) => report.ambient.push(what.to_owned()),
            _ => {}
        }
    }
    Ok(report)
}

/// Samples the CPUs, load average and memory of the Theater `client` reaches, within its
/// container's limits when it is one. Cheaper than [`probe`], so a Station runs it more often.
///
/// # Errors
///
/// The transport cannot run commands, the Theater runs Windows (it needs a POSIX shell), or
/// the command failed or printed nothing it recognised.
pub fn probe_resources(
    client: &TransportClient,
    capabilities: &Capabilities,
) -> Result<Resources, String> {
    if !capabilities.run {
        return Err("its transport cannot run commands".to_owned());
    }
    if capabilities.os.as_deref() == Some("windows") {
        return Err("sampling needs a POSIX shell, and the Theater runs windows".to_owned());
    }
    let output = run(client, script(RESOURCES, []), PROBE_TIMEOUT_MS)?;
    if output.code != Some(0) {
        return Err(format!("sampling failed: {}", failure(&output)));
    }
    parse_resources(&String::from_utf8_lossy(&output.stdout.0))
        .ok_or_else(|| "the Theater reported no CPU or memory figures".to_owned())
}

/// Reads what [`RESOURCES`] printed. A container's cgroup limits narrow the host's figures:
/// fewer CPUs under a CPU quota, less memory under a memory limit. `None` when nothing was
/// recognised.
fn parse_resources(stdout: &str) -> Option<Resources> {
    let mut nproc = None;
    let mut load_1m = None;
    let mut total_kb = None;
    let mut available_kb = None;
    let mut cpu_quota = None;
    let mut memory_limit = None;
    let mut memory_current = None;
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('\t') else {
            continue;
        };
        let value = value.trim();
        match key {
            "nproc" => nproc = value.parse::<u32>().ok().filter(|n| *n > 0),
            "loadavg" => {
                load_1m = value
                    .trim_start_matches('{')
                    .split_whitespace()
                    .next()
                    .and_then(|first| first.parse::<f64>().ok())
                    .filter(|load| load.is_finite() && *load >= 0.0);
            }
            "mem_total_kb" => total_kb = value.parse::<u64>().ok(),
            "mem_available_kb" => available_kb = value.parse::<u64>().ok(),
            "mem_total_bytes" => total_kb = value.parse::<u64>().ok().map(|bytes| bytes / 1024),
            // `max 100000` is no quota; `150000 100000` is one and a half CPUs.
            "cgroup_cpu.max" => {
                let mut fields = value.split_whitespace();
                let quota = fields.next().and_then(|quota| quota.parse::<f64>().ok());
                let period = fields.next().and_then(|period| period.parse::<f64>().ok());
                cpu_quota = quota
                    .zip(period)
                    .filter(|(_, period)| *period > 0.0)
                    .map(|(quota, period)| quota / period)
                    .filter(|cpus| cpus.is_finite() && *cpus > 0.0);
            }
            "cgroup_memory.max" => memory_limit = value.parse::<u64>().ok(),
            "cgroup_memory.current" => memory_current = value.parse::<u64>().ok(),
            _ => {}
        }
    }
    let mut memory_total_mb = total_kb.map(|kb| kb / 1024);
    let mut memory_available_mb = available_kb.map(|kb| kb / 1024);
    if let Some(limit) = memory_limit {
        let limit_mb = limit / (1024 * 1024);
        memory_total_mb = Some(memory_total_mb.map_or(limit_mb, |total| total.min(limit_mb)));
        if let Some(current) = memory_current {
            let left_mb = limit.saturating_sub(current) / (1024 * 1024);
            memory_available_mb = Some(memory_available_mb.map_or(left_mb, |a| a.min(left_mb)));
        }
    }
    let cpus = match (nproc.map(f64::from), cpu_quota) {
        (Some(cpus), Some(quota)) => Some(cpus.min(quota)),
        (cpus, quota) => cpus.or(quota),
    }
    .map(|cpus| (cpus * 100.0).round() / 100.0);
    let resources = Resources {
        cpus,
        cpu_percent: None,
        load_1m,
        memory_total_mb,
        memory_available_mb,
        sampled_at: unix_now(),
    };
    (resources.cpus.is_some() || resources.load_1m.is_some() || resources.memory_total_mb.is_some())
        .then_some(resources)
}

/// The name transports use for what `uname -s` printed.
fn os_name(uname: &str) -> String {
    match uname.trim() {
        "Darwin" => "macos".to_owned(),
        other => other.to_ascii_lowercase(),
    }
}

/// What one provisioning step did.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    /// Already as declared; nothing was done.
    Ok,
    /// It was not as declared, and now is.
    Changed,
    /// It is not as declared; a check that changes nothing left it so.
    WouldChange,
    /// It is not as declared and could not be made so.
    Failed,
}

impl StepStatus {
    /// The name printed for the step.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Changed => "changed",
            Self::WouldChange => "would-change",
            Self::Failed => "failed",
        }
    }
}

/// One provisioning step and what it did.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionStep {
    /// What the step sees to: `os`, `disk`, a program's name, `runner`, `plugin NAME`.
    pub name: String,
    /// What it did.
    pub status: StepStatus,
    /// What it found or did, in words an operator can act on.
    pub detail: String,
}

impl ProvisionStep {
    fn new(name: impl Into<String>, status: StepStatus, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
        }
    }
}

/// Fits `theater` out over `client`, in order: its operating system and free space (checked,
/// never changed), each required program (installed with its `provision.install` command
/// when missing or too old), then the runner and its provider libraries (copied from the
/// Station when their SHA-256 differs).
///
/// With `check`, it only looks: a step that would act reports
/// [`StepStatus::WouldChange`] and nothing on the Theater is written. Never fails: a
/// Theater that cannot be reached answers one failed step.
pub fn provision(client: &TransportClient, theater: &Theater, check: bool) -> Vec<ProvisionStep> {
    let capabilities = match client.probe() {
        Ok(capabilities) => capabilities,
        Err(error) => {
            return vec![ProvisionStep::new(
                "transport",
                StepStatus::Failed,
                format!("the Theater cannot be reached: {}", describe(&error)),
            )];
        }
    };
    let default_requires = Requirements::default();
    let requires = theater.requires.as_ref().unwrap_or(&default_requires);
    let default_spec = ProvisionSpec::default();
    let spec = theater.provision.as_ref().unwrap_or(&default_spec);
    let report = probe(
        client,
        &capabilities,
        Some(requires),
        theater.workdir.as_deref(),
    );
    if let Some(error) = &report.error {
        return vec![ProvisionStep::new(
            "probe",
            StepStatus::Failed,
            error.clone(),
        )];
    }
    let mut steps = Vec::new();
    let unmet = |name: &str| report.unmet.iter().find(|unmet| unmet.name == name);
    if let Some(os) = &requires.os {
        steps.push(match unmet("os") {
            Some(unmet) => ProvisionStep::new("os", StepStatus::Failed, unmet.reason.clone()),
            None => ProvisionStep::new("os", StepStatus::Ok, os.clone()),
        });
    }
    if requires.disk_mb.is_some() {
        steps.push(match unmet("disk") {
            Some(unmet) => ProvisionStep::new("disk", StepStatus::Failed, unmet.to_string()),
            None => ProvisionStep::new(
                "disk",
                StepStatus::Ok,
                format!("{} MiB free", report.free_mb.unwrap_or_default()),
            ),
        });
    }
    let timeout = Duration::from_secs(
        spec.install_timeout_secs
            .unwrap_or(DEFAULT_INSTALL_TIMEOUT_SECS),
    );
    for program in requires.tools.iter().chain(&requires.harnesses) {
        steps.push(install_step(
            client,
            &capabilities,
            program,
            &report,
            spec,
            (check, timeout, theater.workdir.as_deref()),
        ));
    }
    if let Some(runner) = &theater.runner {
        steps.extend(runner_steps(client, &capabilities, runner, spec, check));
    }
    steps
}

fn install_step(
    client: &TransportClient,
    capabilities: &Capabilities,
    program: &RequiredProgram,
    report: &ProbeReport,
    spec: &ProvisionSpec,
    (check, timeout, dir): (bool, Duration, Option<&str>),
) -> ProvisionStep {
    let name = program.name();
    let Some(unmet) = report.unmet.iter().find(|unmet| unmet.name == name) else {
        let version = report.found.get(name).cloned().unwrap_or_default();
        return ProvisionStep::new(name, StepStatus::Ok, version);
    };
    let Some(command) = spec.install.get(name) else {
        return ProvisionStep::new(
            name,
            StepStatus::Failed,
            format!("{unmet}, and provision.install.{name} says nothing about how to install it"),
        );
    };
    if check {
        return ProvisionStep::new(
            name,
            StepStatus::WouldChange,
            format!("{unmet}; would run provision.install.{name}"),
        );
    }
    let installed = run(
        client,
        vec!["sh".to_owned(), "-c".to_owned(), command.clone()],
        u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
    );
    match installed {
        Ok(output) if output.code == Some(0) => {}
        Ok(output) => {
            return ProvisionStep::new(
                name,
                StepStatus::Failed,
                format!("provision.install.{name} failed: {}", failure(&output)),
            );
        }
        Err(error) => {
            return ProvisionStep::new(
                name,
                StepStatus::Failed,
                format!("provision.install.{name} could not run: {error}"),
            );
        }
    }
    let only = Requirements::default().with_tool(program.clone());
    let after = probe(client, capabilities, Some(&only), dir);
    match (&after.error, after.unmet.first()) {
        (Some(error), _) => ProvisionStep::new(name, StepStatus::Failed, error.clone()),
        (None, Some(still)) => ProvisionStep::new(
            name,
            StepStatus::Failed,
            format!("provision.install.{name} ran, but {still}"),
        ),
        (None, None) => ProvisionStep::new(
            name,
            StepStatus::Changed,
            format!(
                "installed {}",
                after.found.get(name).cloned().unwrap_or_default()
            ),
        ),
    }
}

fn runner_steps(
    client: &TransportClient,
    capabilities: &Capabilities,
    runner: &crate::directorates::m::theaters::RunnerSettings,
    spec: &ProvisionSpec,
    check: bool,
) -> Vec<ProvisionStep> {
    let mut steps = Vec::new();
    let destination = runner.command.as_deref().and_then(theater_path);
    let source = match &spec.runner {
        Some(path) => Ok(std::path::PathBuf::from(path)),
        None if capabilities.os.as_deref() == Some(std::env::consts::OS)
            && capabilities.arch.as_deref() == Some(std::env::consts::ARCH) =>
        {
            std::env::current_exe().map_err(|error| format!("this Station's executable: {error}"))
        }
        None => Err(format!(
            "the Theater runs {}/{} and this Station {}/{}: name an ai6 build for it in provision.runner",
            capabilities.os.as_deref().unwrap_or("an unknown os"),
            capabilities.arch.as_deref().unwrap_or("an unknown arch"),
            std::env::consts::OS,
            std::env::consts::ARCH,
        )),
    };
    match (destination, source) {
        (None, _) => steps.push(ProvisionStep::new(
            "runner",
            StepStatus::Failed,
            "runner.command names no path to install ai6 at; set it to one such as \
             \"/opt/ai6/bin/ai6\"",
        )),
        (_, Err(error)) => steps.push(ProvisionStep::new("runner", StepStatus::Failed, error)),
        (Some(destination), Ok(source)) => {
            steps.push(place_file(client, "runner", &source, &destination, check))
        }
    }
    if spec.plugins.is_empty() {
        return steps;
    }
    let Some(directory) = runner
        .plugin_paths
        .first()
        .and_then(|dir| theater_path(dir))
    else {
        steps.push(ProvisionStep::new(
            "plugins",
            StepStatus::Failed,
            "runner.plugin_paths names no directory to copy provision.plugins into",
        ));
        return steps;
    };
    for plugin in &spec.plugins {
        let source = std::path::PathBuf::from(plugin);
        let file = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let destination = format!("{}/{file}", directory.trim_end_matches('/'));
        steps.push(place_file(
            client,
            &format!("plugin {file}"),
            &source,
            &destination,
            check,
        ));
    }
    steps
}

/// A path on the Theater as transports take it: absolute, or relative to the transport's
/// default directory (the login user's home over SSH), which is what `~/` means. `None` for a
/// bare command name, which only a `PATH` lookup finds.
fn theater_path(path: &str) -> Option<String> {
    if let Some(relative) = path.strip_prefix("~/") {
        return Some(relative.to_owned());
    }
    path.contains('/').then(|| path.to_owned())
}

/// Copies `source` from the Station to `destination` on the Theater unless the file there
/// already has the same SHA-256.
fn place_file(
    client: &TransportClient,
    name: &str,
    source: &std::path::Path,
    destination: &str,
    check: bool,
) -> ProvisionStep {
    let bytes = match std::fs::read(source) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ProvisionStep::new(
                name,
                StepStatus::Failed,
                format!("{} on the Station: {error}", source.display()),
            );
        }
    };
    let wanted = hex(&Sha256::digest(&bytes));
    let digest = |client: &TransportClient| {
        run(client, script(DIGEST, [destination]), COMMAND_TIMEOUT_MS).and_then(|output| {
            if output.code == Some(0) {
                Ok(String::from_utf8_lossy(&output.stdout.0).trim().to_owned())
            } else {
                Err(failure(&output))
            }
        })
    };
    let present = match digest(client) {
        Ok(present) => present,
        Err(error) => {
            return ProvisionStep::new(
                name,
                StepStatus::Failed,
                format!("{destination} could not be read: {error}"),
            );
        }
    };
    if present == wanted {
        return ProvisionStep::new(name, StepStatus::Ok, format!("{destination} is current"));
    }
    let what = if present.is_empty() {
        format!("{destination} is missing")
    } else {
        format!("{destination} differs from {}", source.display())
    };
    if check {
        return ProvisionStep::new(
            name,
            StepStatus::WouldChange,
            format!("{what}; would copy it"),
        );
    }
    let partial = format!("{destination}.ai6-partial");
    let written = client
        .write_file(&WriteSpec {
            path: partial.clone(),
            data: Bytes(bytes),
            append: false,
            create_dirs: true,
        })
        .map_err(|error| describe(&error))
        .and_then(|()| {
            run(
                client,
                script(PLACE, [partial.as_str(), destination]),
                COMMAND_TIMEOUT_MS,
            )
        })
        .and_then(|output| {
            if output.code == Some(0) {
                Ok(())
            } else {
                Err(failure(&output))
            }
        });
    if let Err(error) = written {
        return ProvisionStep::new(
            name,
            StepStatus::Failed,
            format!("{destination} could not be written: {error}"),
        );
    }
    match digest(client) {
        Ok(after) if after == wanted => ProvisionStep::new(
            name,
            StepStatus::Changed,
            format!("{what}; copied {}", source.display()),
        ),
        Ok(_) => ProvisionStep::new(
            name,
            StepStatus::Failed,
            format!("{destination} was written but its SHA-256 does not match"),
        ),
        Err(error) => ProvisionStep::new(
            name,
            StepStatus::Failed,
            format!("{destination} could not be read back: {error}"),
        ),
    }
}

fn script<'a>(body: &str, arguments: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        body.to_owned(),
        "sh".to_owned(),
    ];
    argv.extend(arguments.into_iter().map(str::to_owned));
    argv
}

fn run(client: &TransportClient, argv: Vec<String>, timeout_ms: u64) -> Result<Output, String> {
    client
        .run(&RunSpec {
            argv,
            timeout_ms,
            ..RunSpec::default()
        })
        .map_err(|error| describe(&error))
}

/// The exit status and the last line of standard error.
fn failure(output: &Output) -> String {
    let status = if output.timed_out {
        "timed out".to_owned()
    } else {
        output.code.map_or_else(
            || "no exit status".to_owned(),
            |code| format!("exit {code}"),
        )
    };
    let stderr = String::from_utf8_lossy(&output.stderr.0);
    match stderr.trim().lines().last() {
        Some(line) => format!("{status}: {line}"),
        None => status,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_programs_parse_and_compare_versions_number_by_number() {
        let claude: RequiredProgram = "claude>=2.1".parse().unwrap();
        assert_eq!(claude.name(), "claude");
        assert_eq!(claude.minimum(), Some("2.1"));
        assert_eq!(claude.to_string(), "claude>=2.1");
        assert_eq!(claude.refuses("2.1.310 (Claude Code)"), None);
        assert_eq!(claude.refuses("claude 2.10.0"), None, "2.10 is after 2.1");
        assert_eq!(
            claude.refuses("2.0.9").as_deref(),
            Some("version 2.0.9 is older than 2.1")
        );
        assert!(
            claude
                .refuses("unknown")
                .unwrap()
                .contains("could not be read")
        );
        let git: RequiredProgram = "git".parse().unwrap();
        assert_eq!(git.refuses("anything"), None);
        for bad in [
            "",
            "-x",
            "a b",
            "claude>=",
            "claude>=two",
            "claude>=1..2",
            ".hidden",
        ] {
            assert!(bad.parse::<RequiredProgram>().is_err(), "{bad:?}");
        }
        let parsed: Requirements =
            toml::from_str("harnesses = [\"claude>=2.1\"]\ntools = [\"git\"]\n").unwrap();
        assert_eq!(parsed.harnesses, [claude]);
        assert_eq!(
            toml::to_string(&parsed).unwrap(),
            "harnesses = [\"claude>=2.1\"]\ntools = [\"git\"]\n"
        );
    }

    #[test]
    fn a_report_is_checked_against_every_requirement() {
        let requires = Requirements::default()
            .with_os("linux")
            .with_harness("claude>=2.1".parse().unwrap())
            .with_tool("git".parse().unwrap())
            .with_tool("cargo".parse().unwrap())
            .with_disk_mb(100);
        let report = ProbeReport {
            os: Some("linux".to_owned()),
            found: BTreeMap::from([
                ("claude".to_owned(), "2.0.1 (Claude Code)".to_owned()),
                ("git".to_owned(), "git version 2.47.0".to_owned()),
            ]),
            free_mb: Some(50),
            ..ProbeReport::default()
        };
        let unmet: Vec<String> = requires
            .check(&report)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            unmet,
            [
                "claude>=2.1: version 2.0.1 is older than 2.1",
                "cargo: not installed",
                "100 MiB free: only 50 MiB free",
            ]
        );
        assert!(requires.allows_harness("claude") && !requires.allows_harness("codex"));
        assert!(Requirements::default().allows_harness("codex"));
        let elsewhere = ProbeReport {
            os: Some("macos".to_owned()),
            ..report
        };
        assert!(requires.check(&elsewhere)[0].reason.contains("runs macos"));
        assert!(
            Requirements::default()
                .with_tool("git".parse().unwrap())
                .with_tool("git".parse().unwrap())
                .validate()
                .is_err()
        );
    }

    #[test]
    fn theater_paths_are_absolute_or_home_relative() {
        assert_eq!(
            theater_path("/opt/ai6/bin/ai6").as_deref(),
            Some("/opt/ai6/bin/ai6")
        );
        assert_eq!(
            theater_path("~/.local/bin/ai6").as_deref(),
            Some(".local/bin/ai6")
        );
        assert_eq!(theater_path("ai6"), None);
    }

    // Outputs of `RESOURCES` captured on real hosts.
    const FEDORA: &str = "nproc\t12\nloadavg\t10.02 6.89 8.02 9/1271 2033858\n\
                          mem_total_kb\t32533308\nmem_available_kb\t27105716\n";
    // Alpine 3.22 under rootless Podman, `--memory 256m --cpus 1.5`, on the same host.
    const ALPINE_LIMITED: &str = "nproc\t12\nloadavg\t8.08 6.89 7.97 3/1226 4\n\
                                  mem_total_kb\t32533308\nmem_available_kb\t29141740\n\
                                  cgroup_cpu.max\t150000 100000\n\
                                  cgroup_memory.max\t268435456\n\
                                  cgroup_memory.current\t2322432\n";
    const ALPINE_UNLIMITED: &str = "nproc\t12\nloadavg\t7.61 6.83 7.94 4/1219 4\n\
                                    mem_total_kb\t32533308\nmem_available_kb\t29100368\n\
                                    cgroup_cpu.max\tmax 100000\ncgroup_memory.max\tmax\n\
                                    cgroup_memory.current\t1720320\n";
    // macOS 15 (x86_64): no /proc, so no available memory.
    const MACOS: &str = "nproc\t16\nloadavg\t{ 3.13 2.97 3.06 }\nmem_total_bytes\t12884901888\n";

    #[test]
    fn resources_parse_from_a_host_a_limited_container_and_macos() {
        let fedora = parse_resources(FEDORA).unwrap();
        assert_eq!(
            (fedora.cpus, fedora.load_1m, fedora.cpu_percent),
            (Some(12.0), Some(10.02), None)
        );
        assert_eq!(
            (fedora.memory_total_mb, fedora.memory_available_mb),
            (Some(31770), Some(26470))
        );

        let limited = parse_resources(ALPINE_LIMITED).unwrap();
        assert_eq!(limited.cpus, Some(1.5), "the CPU quota, not the host's 12");
        assert_eq!(limited.memory_total_mb, Some(256), "the memory limit");
        assert_eq!(
            limited.memory_available_mb,
            Some(253),
            "the limit less what the container uses"
        );

        let unlimited = parse_resources(ALPINE_UNLIMITED).unwrap();
        assert_eq!(unlimited.cpus, Some(12.0));
        assert_eq!(unlimited.memory_total_mb, Some(31770));
        assert_eq!(unlimited.memory_available_mb, Some(28418));

        let macos = parse_resources(MACOS).unwrap();
        assert_eq!(
            (macos.cpus, macos.load_1m, macos.memory_total_mb),
            (Some(16.0), Some(3.13), Some(12288))
        );
        assert_eq!(macos.memory_available_mb, None, "never guessed");

        assert_eq!(parse_resources(""), None);
        assert_eq!(parse_resources("nproc\tmany\nloadavg\tnan\n"), None);
    }

    #[test]
    fn the_full_probe_carries_resources() {
        use crate::directorates::q::transport::Transport;
        use guatiao::library::ProviderError;
        use guatiao::{Status, ToValue, Value};

        struct Prints(String);
        impl Transport for Prints {
            fn capabilities(&self) -> Result<Value, ProviderError> {
                Err(ProviderError::new(Status::GUATIAO_ERR_INTERNAL, "unused"))
            }
            fn run(&self, _spec: &Value) -> Result<Value, ProviderError> {
                Output {
                    code: Some(0),
                    stdout: Bytes(self.0.as_bytes().to_vec()),
                    ..Output::default()
                }
                .to_value()
                .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &e.to_string()))
            }
        }
        let client = TransportClient::new(Box::new(Prints(format!("os\tLinux\n{ALPINE_LIMITED}"))));
        let runs = Capabilities {
            run: true,
            ..Capabilities::default()
        };
        let report = probe(&client, &runs, None, None);
        assert_eq!(report.resources.unwrap().memory_total_mb, Some(256));
        assert_eq!(
            probe_resources(&client, &runs).unwrap().cpus,
            Some(1.5),
            "the resources-only probe reads the same lines"
        );
        let windows = Capabilities {
            os: Some("windows".to_owned()),
            ..runs
        };
        assert!(probe_resources(&client, &windows).is_err());
    }
}
