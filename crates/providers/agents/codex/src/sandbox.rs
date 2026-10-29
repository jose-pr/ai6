//! The sandbox an assistant's scripts run in: a Codex permission profile
//! (learn.chatgpt.com/docs/permissions) that denies the whole filesystem but the platform's
//! own paths, lets the working directory be read and only the mode's directories be written,
//! and gives scripts no network.
//!
//! Measured with Codex 0.164.0 against a loopback stand-in for the model: under this profile a
//! script wrote only the mode's directories and read nothing outside the working directory, on
//! Windows (MXC) and on Linux (bubblewrap). With the profile's network on, app-server threads
//! started no proxy, so no host list was enforced and scripts reached the host's loopback
//! services: scripts therefore never get a network here. A legacy `workspace-write` sandbox
//! made the whole working directory and `%TEMP%` writable and every file readable.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use ai6::directorates::section_00::agent::AssistantMode;
use ai6::directorates::section_00::assistant::SCRIPT_ENVIRONMENT;
use ai6::directorates::section_00::programs::{
    canonical, find_program, path_text, program_dirs, station_paths, variable,
};
use serde_json::Value;

use crate::tools::CONFIG_ENVIRONMENT;

/// The permission profile an assistant's scripts run under.
pub(crate) const PROFILE: &str = "ai6_assistant";
/// The profile the availability probe runs under: [`PROFILE`] with nothing to write.
const PROBE_PROFILE: &str = "ai6_assistant_probe";

/// The features a scripts thread keeps besides the tool check's own: `exec`'s JavaScript host
/// (without it `exec` fails closed) and the command tools `exec` reaches.
pub(crate) const SCRIPT_FEATURES: [&str; 3] = ["code_mode_host", "shell_tool", "unified_exec"];

/// What the probe prints when Codex ran it inside its sandbox.
const PROBE_MARKER: &str = "AI6_CODEX_SANDBOX_OK";
/// How long the probe may run, and how long its answer stands.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// The sandbox a scripts thread's commands run in: [`PROFILE`] with these filesystem rules
/// ([`rules`]), keeping only [`SCRIPT_ENVIRONMENT`] in a command's environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Profile {
    rules: Vec<(String, &'static str)>,
}

impl Profile {
    /// The profile of `mode`'s scripts, run from `cwd` by `executable` with `environment`.
    pub(crate) fn new(
        executable: &str,
        cwd: &str,
        environment: &BTreeMap<String, String>,
        mode: &AssistantMode,
    ) -> Self {
        Self {
            rules: rules(
                executable,
                Path::new(cwd),
                environment,
                &mode.interpreters,
                &mode.files,
            ),
        }
    }

    /// The `-c` overrides that run the thread's commands in this profile: the profile, chosen
    /// as the default; only [`SCRIPT_ENVIRONMENT`] kept; and on Windows the MXC sandbox, the one
    /// implementation that denies reads (`unelevated` refuses a profile that restricts reads
    /// and `elevated` needs the whole filesystem readable, Codex 0.164.0).
    pub(crate) fn overrides(&self) -> Vec<String> {
        let mut argv = windows_sandbox();
        argv.extend([
            "-c".to_owned(),
            format!(
                "permissions.{PROFILE}={{filesystem={}}}",
                table(&self.rules)
            ),
            "-c".to_owned(),
            format!("default_permissions=\"{PROFILE}\""),
            "-c".to_owned(),
            format!(
                "shell_environment_policy.include_only={}",
                serde_json::to_string(&SCRIPT_ENVIRONMENT).unwrap_or_else(|_| "[]".to_owned())
            ),
        ]);
        argv
    }

    /// Checks an app server's effective configuration (the result of `config/read`) against
    /// [`Self::overrides`]: `default_permissions` is [`PROFILE`], the profile holds exactly
    /// these filesystem rules and nothing else, and the environment policy is only
    /// `include_only`. Codex merges a user's `[permissions.<name>]` table into the one passed
    /// with `-c` and applies `shell_environment_policy.set` to the variables kept (measured with
    /// Codex 0.164.0: a user's `network.enabled = true` let scripts reach loopback and the
    /// internet). `Err` names what differs.
    pub(crate) fn check_config(&self, result: &Value) -> Result<(), String> {
        let config = result.get("config").unwrap_or(&Value::Null);
        let mut found = Vec::new();
        match config.get("default_permissions") {
            Some(Value::String(name)) if name == PROFILE => {}
            other => found.push(format!("default_permissions is {}", shown(other))),
        }
        match config
            .pointer(&format!("/permissions/{PROFILE}"))
            .and_then(Value::as_object)
        {
            None => found.push(format!("permissions.{PROFILE} is missing")),
            Some(profile) => {
                for (key, value) in profile {
                    if key == "filesystem" {
                        found.extend(self.filesystem_changes(value));
                    } else if !value.is_null() {
                        found.push(format!("permissions.{PROFILE}.{key} is {value}"));
                    }
                }
            }
        }
        match config
            .get("shell_environment_policy")
            .and_then(Value::as_object)
        {
            None => found.push("shell_environment_policy is missing".to_owned()),
            Some(policy) => {
                for (key, value) in policy {
                    let expected = if key == "include_only" {
                        serde_json::json!(SCRIPT_ENVIRONMENT)
                    } else {
                        Value::Null
                    };
                    if *value != expected {
                        found.push(format!("shell_environment_policy.{key} is {value}"));
                    }
                }
            }
        }
        changed(found)
    }

    /// The entries of the effective `filesystem` table that differ from these rules.
    fn filesystem_changes(&self, table: &Value) -> Vec<String> {
        let Some(table) = table.as_object() else {
            return vec![format!("permissions.{PROFILE}.filesystem is {table}")];
        };
        let expected: BTreeMap<&str, &str> = self
            .rules
            .iter()
            .map(|(key, access)| (key.as_str(), *access))
            .collect();
        let mut found: Vec<String> = table
            .iter()
            .filter(|(key, value)| {
                !value.is_null() && value.as_str() != expected.get(key.as_str()).copied()
            })
            .map(|(key, value)| format!("permissions.{PROFILE}.filesystem has {key:?} = {value}"))
            .collect();
        found.extend(
            expected
                .keys()
                .filter(|key| table.get(**key).is_none_or(Value::is_null))
                .map(|key| format!("permissions.{PROFILE}.filesystem lacks {key:?}")),
        );
        found
    }

    /// Checks the thread `thread/start` or `thread/resume` answered with: it runs in
    /// [`PROFILE`] (`activePermissionProfile`) and its sandbox has no network.
    pub(crate) fn check_thread(&self, result: &Value) -> Result<(), String> {
        let mut found = Vec::new();
        match result.pointer("/activePermissionProfile/id") {
            Some(Value::String(name)) if name == PROFILE => {}
            other => found.push(format!(
                "the thread's permission profile is {}",
                shown(other)
            )),
        }
        if result.pointer("/sandbox/networkAccess") == Some(&Value::Bool(true)) {
            found.push("the thread's sandbox has network access".to_owned());
        }
        changed(found)
    }
}

/// `value` as JSON, or `missing`.
fn shown(value: Option<&Value>) -> String {
    value.map_or_else(|| "missing".to_owned(), Value::to_string)
}

/// `Err` naming `found`, unless it is empty.
fn changed(found: Vec<String>) -> Result<(), String> {
    if found.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Codex's configuration changes the sandbox Moneypenny's scripts run in ({}): remove that \
         from Codex's config.toml or its managed configuration, or run her without interpreters",
        found.join("; ")
    ))
}

/// `-c windows.sandbox="mxc"` on Windows, nothing elsewhere.
fn windows_sandbox() -> Vec<String> {
    if cfg!(windows) {
        vec!["-c".to_owned(), "windows.sandbox=\"mxc\"".to_owned()]
    } else {
        Vec::new()
    }
}

/// An inline TOML table of `rules`, each key a quoted path or special token: a JSON string is
/// a valid TOML basic string.
fn table(rules: &[(String, &str)]) -> String {
    let entries = rules
        .iter()
        .map(|(key, access)| {
            format!(
                "{}=\"{access}\"",
                serde_json::to_string(key).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    format!("{{{}}}", entries.join(","))
}

/// `dir`, a `/`-separated relative path, with the platform's separator.
fn native(dir: &str) -> PathBuf {
    dir.split('/').collect()
}

/// The filesystem rules of a profile whose commands run in `cwd`: everything denied but
/// `:minimal` and `cwd`, the install directories of the codex executable and `interpreters`
/// ([`read_dirs`]) readable, `files` (relative to `cwd`) writable, and the Station's own paths
/// ([`station_paths`]) denied again.
///
/// `:minimal` re-opens all of `/etc`, `/usr` (with `/usr/local/etc` and `/usr/etc`), `/dev`,
/// `/proc` and `/run` on Linux, and `C:\Windows`, `C:\Program Files` and `C:\ProgramData` on
/// Windows, and a deny inside a readable directory wins over it (measured with Codex 0.164.0:
/// `codex sandbox -P`, bubblewrap and MXC).
fn rules(
    executable: &str,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    interpreters: &[String],
    files: &[String],
) -> Vec<(String, &'static str)> {
    let open = read_dirs(executable, cwd, environment, interpreters);
    let mut rules = vec![
        (":root".to_owned(), "deny"),
        (":minimal".to_owned(), "read"),
        (path_text(cwd), "read"),
    ];
    rules.extend(open.iter().map(|dir| (path_text(dir), "read")));
    rules.extend(
        files
            .iter()
            .map(|dir| (path_text(&cwd.join(native(dir))), "write")),
    );
    rules.extend(
        station_paths(cwd, &open)
            .iter()
            .map(|path| (path_text(path), "deny")),
    );
    // A key given twice makes the inline table invalid TOML.
    let mut seen = std::collections::HashSet::new();
    rules.retain(|(key, _)| seen.insert(key.clone()));
    rules
}

/// The install directories scripts also read ([`program_dirs`]): the codex executable's
/// (bubblewrap runs it again inside the sandbox on Linux) and each interpreter's.
fn read_dirs(
    executable: &str,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    interpreters: &[String],
) -> Vec<PathBuf> {
    let programs: Vec<&str> = std::iter::once(executable)
        .chain(interpreters.iter().map(String::as_str))
        .collect();
    program_dirs(&programs, cwd, environment)
}

/// Why Codex cannot run an assistant's scripts in its sandbox here, `None` when it can: the
/// answer of [`probe`] with the process runner, kept for [`PROBE_LIFETIME`] per executable and
/// configuration environment.
pub(crate) fn unavailable(
    executable: &str,
    environment: &BTreeMap<String, String>,
) -> Option<String> {
    type Key = (String, Vec<(String, Option<String>)>);
    type Answers = Mutex<HashMap<Key, (Instant, Option<String>)>>;
    static ANSWERS: OnceLock<Answers> = OnceLock::new();
    let key = (
        executable.to_owned(),
        CONFIG_ENVIRONMENT
            .iter()
            .map(|name| ((*name).to_owned(), variable(environment, name)))
            .collect(),
    );
    let answers = ANSWERS.get_or_init(Mutex::default);
    let cached = answers.lock().ok().and_then(|answers| {
        answers
            .get(&key)
            .filter(|(at, _)| at.elapsed() < PROBE_LIFETIME)
            .map(|(_, answer)| answer.clone())
    });
    if let Some(answer) = cached {
        return answer;
    }
    let answer = probe(executable, environment, &run);
    if let Ok(mut answers) = answers.lock() {
        answers.retain(|_, (at, _)| at.elapsed() < PROBE_LIFETIME);
        answers.insert(key, (Instant::now(), answer.clone()));
    }
    answer
}

/// Runs one command and answers whether it exited successfully, with what it printed.
pub(crate) type Runner<'a> =
    dyn Fn(&[String], &Path, &BTreeMap<String, String>) -> std::io::Result<(bool, String)> + 'a;

/// Asks Codex to run a command in its sandbox under [`PROBE_PROFILE`] (`codex sandbox -P`, no
/// model involved), from a fresh directory it may read; `None` when the command ran there.
pub(crate) fn probe(
    executable: &str,
    environment: &BTreeMap<String, String>,
    runner: &Runner<'_>,
) -> Option<String> {
    let dir = match tempfile::Builder::new()
        .prefix("ai6-codex-probe-")
        .tempdir()
    {
        Ok(dir) => dir,
        Err(error) => {
            return Some(format!(
                "the sandbox probe's directory was not made: {error}"
            ));
        }
    };
    probe_in(executable, environment, runner, dir.path())
}

fn probe_in(
    executable: &str,
    environment: &BTreeMap<String, String>,
    runner: &Runner<'_>,
    dir: &Path,
) -> Option<String> {
    let rules = rules(executable, &canonical(dir), environment, &[], &[]);
    let mut argv = vec![executable.to_owned()];
    argv.extend(windows_sandbox());
    argv.extend([
        "-c".to_owned(),
        format!(
            "permissions.{PROBE_PROFILE}={{filesystem={}}}",
            table(&rules)
        ),
        "sandbox".to_owned(),
        "-P".to_owned(),
        PROBE_PROFILE.to_owned(),
        "--".to_owned(),
    ]);
    if cfg!(windows) {
        argv.extend(["cmd.exe", "/d", "/c", "echo", PROBE_MARKER].map(str::to_owned));
    } else {
        argv.extend(["/bin/sh", "-c", &format!("echo {PROBE_MARKER}")].map(str::to_owned));
    }
    let (succeeded, output) = match runner(&argv, dir, environment) {
        Ok(answer) => answer,
        Err(error) => return Some(format!("`{executable} sandbox` did not run: {error}")),
    };
    if succeeded && output.contains(PROBE_MARKER) {
        return None;
    }
    let last = output
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("no output");
    let last: String = last.chars().take(300).collect();
    Some(format!(
        "Codex's sandbox did not run a command here{}: {last}",
        if cfg!(windows) {
            " (windows.sandbox = \"mxc\")"
        } else {
            ""
        }
    ))
}

/// Runs `argv` in `cwd` with exactly `environment`, killed after [`PROBE_TIMEOUT`].
fn run(
    argv: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
) -> std::io::Result<(bool, String)> {
    run_for(argv, cwd, environment, PROBE_TIMEOUT)
}

/// [`run`] with `timeout`. The command and everything it starts form one process tree (its own
/// process group on Unix, a kill-on-close Job Object on Windows), killed whole once it exits or
/// times out, so nothing it left behind keeps running or holds its output open.
fn run_for(
    argv: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    timeout: Duration,
) -> std::io::Result<(bool, String)> {
    let program = find_program(&argv[0], environment).unwrap_or_else(|| PathBuf::from(&argv[0]));
    let mut command = std::process::Command::new(program);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, tree) = tree::spawn(&mut command)?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // Unreaped (timed out), the program's children may still be found by their parent.
    let _ = tree.kill(status.is_none());
    let output = child.wait_with_output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    match status {
        Some(status) => Ok((status.success(), text)),
        None => Ok((false, format!("{text}\ndid not finish in {timeout:?}"))),
    }
}

/// Starting the probe as the root of a process tree.
#[cfg(unix)]
mod tree {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    use crate::process_tree::ProcessTree;

    /// Spawns `command` leading a process group of its own.
    pub(super) fn spawn(command: &mut Command) -> std::io::Result<(Child, ProcessTree)> {
        command.process_group(0);
        let mut child = command.spawn()?;
        match ProcessTree::of(Some(child.id())) {
            Ok(tree) => Ok((child, tree)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }
}

/// Starting the probe as the root of a process tree.
#[cfg(windows)]
mod tree {
    #![allow(unsafe_code)]

    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    use crate::process_tree::ProcessTree;

    /// Spawns `command` suspended, puts it in a new Job Object and only then lets it run, so
    /// nothing it starts is outside the job.
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, ProcessTree)> {
        let tree = ProcessTree::new()?;
        command.creation_flags(CREATE_SUSPENDED);
        let mut child = command.spawn()?;
        // SAFETY: both handles are open: the job's while `tree` lives, the process's while
        // `child` does.
        let joined =
            unsafe { AssignProcessToJobObject(tree.handle(), child.as_raw_handle() as HANDLE) };
        let started = if joined == 0 {
            Err(io::Error::last_os_error())
        } else {
            resume(child.id())
        };
        match started {
            Ok(()) => Ok((child, tree)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }

    /// Resumes the threads of the suspended process `pid`: its main thread, the only one it has.
    fn resume(pid: u32) -> io::Result<()> {
        // SAFETY: a thread snapshot takes no pointer; the handle is checked and closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed = 0;
        // SAFETY: `entry` is a THREADENTRY32 with its size set, and `snapshot` is open.
        let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: OpenThread takes plain values; a null handle is checked.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if !thread.is_null() {
                    // SAFETY: `thread` was opened above with the right to resume it, and is
                    // closed once.
                    if unsafe { ResumeThread(thread) } != u32::MAX {
                        resumed += 1;
                    }
                    // SAFETY: as above.
                    unsafe { CloseHandle(thread) };
                }
            }
            // SAFETY: as for Thread32First.
            more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        // SAFETY: the snapshot handle is open and closed once.
        unsafe { CloseHandle(snapshot) };
        if resumed == 0 {
            return Err(io::Error::other(
                "the probe's main thread could not be resumed",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai6::directorates::section_00::programs::{STATION_CONFIG_ROOTS, denied_station_paths};

    fn parsed(argv: &[String], key: &str) -> toml::Value {
        let pair = argv
            .windows(2)
            .find(|pair| pair[0] == "-c" && pair[1].starts_with(&format!("{key}=")))
            .unwrap_or_else(|| panic!("no {key} in {argv:?}"));
        let table: toml::Table = toml::from_str(&pair[1]).unwrap();
        let mut value = toml::Value::Table(table);
        for part in key.split('.') {
            value = value[part].clone();
        }
        value
    }

    #[test]
    fn the_profile_denies_all_but_the_platform_reads_the_working_directory_and_writes_its_dirs() {
        let root = tempfile_dir();
        let cwd = root.join("conversation");
        std::fs::create_dir_all(cwd.join("personal")).unwrap();
        let mut mode = AssistantMode::moneypenny();
        mode.files = vec!["personal".to_owned(), "notes/scratch".to_owned()];
        mode.interpreters = vec!["python3".to_owned()];
        let argv = Profile::new(
            "codex-not-on-path",
            &cwd.to_string_lossy(),
            &BTreeMap::from([("PATH".to_owned(), String::new())]),
            &mode,
        )
        .overrides();
        let filesystem = parsed(&argv, "permissions.ai6_assistant")["filesystem"].clone();
        let filesystem = filesystem.as_table().unwrap();
        let cwd_text = cwd.to_string_lossy().into_owned();
        assert_eq!(filesystem[":root"].as_str(), Some("deny"));
        assert_eq!(filesystem[":minimal"].as_str(), Some("read"));
        assert_eq!(filesystem[&cwd_text].as_str(), Some("read"));
        assert_eq!(
            filesystem[&cwd.join("personal").to_string_lossy().into_owned()].as_str(),
            Some("write")
        );
        assert_eq!(
            filesystem[&cwd
                .join("notes")
                .join("scratch")
                .to_string_lossy()
                .into_owned()]
                .as_str(),
            Some("write")
        );
        let denied_again: std::collections::BTreeSet<String> = filesystem
            .iter()
            .filter(|(key, access)| *key != ":root" && access.as_str() == Some("deny"))
            .map(|(key, _)| key.clone())
            .collect();
        let station: std::collections::BTreeSet<String> = station_paths(&cwd, &[])
            .iter()
            .map(|path| path_text(path))
            .collect();
        assert_eq!(
            denied_again, station,
            "the Station's own paths are denied again"
        );
        assert_eq!(
            filesystem.len(),
            5 + station.len(),
            "nothing else: {filesystem:?}"
        );
        assert_eq!(
            parsed(&argv, "default_permissions").as_str(),
            Some("ai6_assistant")
        );
        let kept = parsed(&argv, "shell_environment_policy.include_only");
        assert_eq!(
            kept.as_array().unwrap().len(),
            SCRIPT_ENVIRONMENT.len(),
            "{kept:?}"
        );
        assert!(
            !argv.iter().any(|argument| argument.contains("network")),
            "scripts get no network: {argv:?}"
        );
        assert_eq!(
            argv.iter()
                .any(|argument| argument == "windows.sandbox=\"mxc\""),
            cfg!(windows)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn tempfile_dir() -> PathBuf {
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ai6-codex-sandbox-test-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        canonical(&dir)
    }

    #[test]
    fn an_interpreter_is_readable_where_it_is_unless_that_holds_a_protected_directory() {
        let root = tempfile_dir();
        let tools = root.join("tools").join("python");
        let home = root.join("home");
        let codex_home = home.join(".codex");
        let cwd = root.join("desk").join("conversation");
        for dir in [&tools, &codex_home, &cwd] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let name = if cfg!(windows) {
            "python3.exe"
        } else {
            "python3"
        };
        for dir in [&tools, &root, &codex_home] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let path = |dirs: &[&PathBuf]| std::env::join_paths(dirs).unwrap();
        let environment = |dirs: &[&PathBuf]| {
            BTreeMap::from([
                ("PATH".to_owned(), path(dirs).to_string_lossy().into_owned()),
                ("HOME".to_owned(), home.to_string_lossy().into_owned()),
                (
                    "CODEX_HOME".to_owned(),
                    codex_home.to_string_lossy().into_owned(),
                ),
            ])
        };
        let interpreters = ["python3".to_owned()];
        assert_eq!(
            read_dirs("codex", &cwd, &environment(&[&tools]), &interpreters),
            std::slice::from_ref(&tools)
        );
        assert!(
            read_dirs("codex", &cwd, &environment(&[&root]), &interpreters).is_empty(),
            "a directory holding the home and working directories stays unreadable"
        );
        assert!(
            read_dirs("codex", &cwd, &environment(&[&codex_home]), &interpreters).is_empty(),
            "so does one inside a credential directory"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Codex 0.164.0's `config/read` result (`origins` left out) and `thread/start` result
    /// (`thread` left out) on Linux for a scripts thread under [`fixture_profile`], with the
    /// user's `config.toml` empty, adding `[permissions.ai6_assistant]` entries (`"/" = "write"`,
    /// `network.enabled = true`), and setting `shell_environment_policy.set`; paths and ports
    /// replaced.
    const CONFIG_READ: &str =
        include_str!("../tests/fixtures/codex-0.164.0/config-read-scripts.json");
    const CONFIG_READ_MERGED: &str =
        include_str!("../tests/fixtures/codex-0.164.0/config-read-merged-profile.json");
    const CONFIG_READ_ENV_SET: &str =
        include_str!("../tests/fixtures/codex-0.164.0/config-read-env-set.json");
    const THREAD_START: &str =
        include_str!("../tests/fixtures/codex-0.164.0/thread-start-scripts.json");
    const THREAD_START_MERGED: &str =
        include_str!("../tests/fixtures/codex-0.164.0/thread-start-merged-profile.json");

    /// The profile the fixtures were recorded with.
    fn fixture_profile() -> Profile {
        let conversation = "/tmp/ai6-desk/conversation";
        Profile {
            rules: vec![
                (":root".to_owned(), "deny"),
                (":minimal".to_owned(), "read"),
                (conversation.to_owned(), "read"),
                (format!("{conversation}/personal"), "write"),
                (format!("{conversation}/scratch"), "write"),
                ("/opt/codex".to_owned(), "read"),
                ("/usr".to_owned(), "read"),
            ],
        }
    }

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_users_configuration_merged_into_the_scripts_profile_is_refused() {
        let profile = fixture_profile();
        assert_eq!(profile.check_config(&json(CONFIG_READ)), Ok(()));
        assert_eq!(profile.check_thread(&json(THREAD_START)), Ok(()));

        let merged = profile.check_config(&json(CONFIG_READ_MERGED)).unwrap_err();
        assert!(
            merged.contains("filesystem has \"/\" = \"write\"")
                && merged.contains("permissions.ai6_assistant.network is"),
            "{merged}"
        );
        let thread = profile
            .check_thread(&json(THREAD_START_MERGED))
            .unwrap_err();
        assert!(thread.contains("network access"), "{thread}");
        let set = profile
            .check_config(&json(CONFIG_READ_ENV_SET))
            .unwrap_err();
        assert!(set.contains("shell_environment_policy.set is"), "{set}");

        let mut other = json(CONFIG_READ);
        other["config"]["default_permissions"] = Value::from("wide");
        other["config"]["permissions"]["ai6_assistant"]["filesystem"]
            .as_object_mut()
            .unwrap()
            .remove("/usr");
        let found = profile.check_config(&other).unwrap_err();
        assert!(
            found.contains("default_permissions is \"wide\"") && found.contains("lacks \"/usr\""),
            "{found}"
        );
        let mut unprofiled = json(THREAD_START);
        unprofiled["activePermissionProfile"] = Value::Null;
        assert!(profile.check_thread(&unprofiled).is_err());
        assert!(profile.check_config(&Value::Null).is_err());
    }

    #[test]
    fn the_stations_own_paths_are_denied_inside_what_the_profile_opens() {
        let cwd = Path::new(if cfg!(windows) {
            r"C:\Temp\desk\conversation"
        } else {
            "/tmp/desk/conversation"
        });
        let program = std::env::current_exe().unwrap();
        let program_dir = canonical(program.parent().unwrap());
        let filesystem = parsed(
            &Profile::new(
                "codex-not-on-path",
                &cwd.to_string_lossy(),
                &BTreeMap::from([("PATH".to_owned(), String::new())]),
                &AssistantMode::moneypenny(),
            )
            .overrides(),
            "permissions.ai6_assistant",
        )["filesystem"]
            .clone();
        let denied =
            |path: &Path| filesystem.get(path_text(path)).and_then(|v| v.as_str()) == Some("deny");
        assert!(denied(&program_dir), "{filesystem:?}");
        if !cfg!(windows) {
            for root in STATION_CONFIG_ROOTS {
                assert!(
                    denied(&canonical(Path::new(root))),
                    "{root}: {filesystem:?}"
                );
            }
        }

        let root = tempfile_dir();
        let (state, home, install) = (root.join("state"), root.join("home"), root.join("opt"));
        let conversation = state.join("desk").join("conversation");
        let bin = install.join("bin");
        for dir in [&conversation, &home, &bin] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let station = bin.join("ai6");
        std::fs::write(&station, "").unwrap();
        let unix_roots = if cfg!(windows) {
            0
        } else {
            STATION_CONFIG_ROOTS.len()
        };
        let paths = denied_station_paths(
            &root.join("elsewhere"),
            &[],
            Some(state.clone()),
            Some(home.clone()),
            Some(station.clone()),
        );
        assert_eq!(
            paths[unix_roots..],
            [state.clone(), home.clone(), bin.clone()]
        );
        let paths = denied_station_paths(
            &conversation,
            std::slice::from_ref(&install),
            Some(state),
            Some(home.clone()),
            Some(station.clone()),
        );
        assert_eq!(
            paths[unix_roots..],
            [home, station],
            "a deny holding the working directory is left out, and inside an interpreter's \
             install directory only the program itself is denied"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_probe_that_times_out_is_killed_with_everything_it_started() {
        // The shell's child inherits its output: killing the shell alone would leave the
        // output open, and the probe waiting, until the child exits.
        let argv: Vec<String> = if cfg!(windows) {
            vec!["cmd.exe", "/d", "/c", "ping -n 30 127.0.0.1 & echo done"]
        } else {
            vec!["/bin/sh", "-c", "sleep 30; echo done"]
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        let environment: BTreeMap<String, String> = std::env::vars().collect();
        let started = Instant::now();
        let (succeeded, output) = run_for(
            &argv,
            &std::env::temp_dir(),
            &environment,
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(!succeeded && output.contains("did not finish"), "{output}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the probe's child outlived it: {:?}",
            started.elapsed()
        );
        assert!(!output.contains("done"), "{output}");
    }

    #[test]
    fn the_probe_answers_available_only_when_codex_ran_the_command_in_its_sandbox() {
        let seen = Mutex::new(Vec::new());
        let ran = |argv: &[String], cwd: &Path, _: &BTreeMap<String, String>| {
            seen.lock()
                .unwrap()
                .push((argv.to_vec(), cwd.to_path_buf()));
            Ok((true, format!("{PROBE_MARKER}\r\n")))
        };
        assert_eq!(probe("codex", &BTreeMap::new(), &ran), None);
        let (argv, cwd) = seen.lock().unwrap()[0].clone();
        let at = argv.iter().position(|word| word == "sandbox").unwrap();
        assert_eq!(argv[at..at + 4], ["sandbox", "-P", PROBE_PROFILE, "--"]);
        let filesystem = parsed(&argv, "permissions.ai6_assistant_probe")["filesystem"].clone();
        assert_eq!(filesystem[":root"].as_str(), Some("deny"));
        // The directory is gone by now, so its canonical path (`/private/var/…` for macOS's
        // `/var/…`) cannot be asked for again: its name identifies it.
        let name = cwd.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            filesystem
                .as_table()
                .unwrap()
                .iter()
                .any(|(path, access)| { path.ends_with(&name) && access.as_str() == Some("read") }),
            "the probe runs from a directory it may read: {filesystem:?}"
        );
        assert!(!cwd.exists(), "and removes it");

        let failed = |_: &[String], _: &Path, _: &BTreeMap<String, String>| {
            Ok((
                false,
                "windows sandbox failed: MXC is unavailable\n".to_owned(),
            ))
        };
        let reason = probe("codex", &BTreeMap::new(), &failed).unwrap();
        assert!(reason.contains("MXC is unavailable"), "{reason}");
        let silent =
            |_: &[String], _: &Path, _: &BTreeMap<String, String>| Ok((true, String::new()));
        assert!(
            probe("codex", &BTreeMap::new(), &silent).is_some(),
            "success without the marker is not a sandbox that ran it"
        );
        let missing = |_: &[String], _: &Path, _: &BTreeMap<String, String>| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        };
        assert!(
            probe("codex", &BTreeMap::new(), &missing)
                .unwrap()
                .contains("did not run")
        );
    }
}
