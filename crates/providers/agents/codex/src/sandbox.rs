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
    canonical, find_program, path_text, program_dirs, starts_with, variable,
};

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

/// The `-c` overrides that run a scripts thread's commands in [`PROFILE`]: the profile, chosen
/// as the default; only [`SCRIPT_ENVIRONMENT`] kept in a command's environment; and on Windows
/// the MXC sandbox, the one implementation that denies reads (`unelevated` refuses a profile
/// that restricts reads and `elevated` needs the whole filesystem readable, Codex 0.164.0).
pub(crate) fn overrides(
    executable: &str,
    cwd: &str,
    environment: &BTreeMap<String, String>,
    mode: &AssistantMode,
) -> Vec<String> {
    let rules = rules(
        executable,
        Path::new(cwd),
        environment,
        &mode.interpreters,
        &mode.files,
    );
    let mut argv = windows_sandbox();
    argv.extend([
        "-c".to_owned(),
        format!("permissions.{PROFILE}={{filesystem={}}}", table(&rules)),
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

/// Where a Station on Unix may keep its configuration and secrets inside what `:minimal` or an
/// interpreter's install directory (`/usr`) re-opens.
const STATION_CONFIG_ROOTS: [&str; 3] = ["/etc/ai6", "/usr/local/etc", "/usr/etc"];

/// The Station's own paths a profile denies again: [`STATION_CONFIG_ROOTS`] (not on Windows),
/// AI6's state directory, `AI6_HOME`, and the directory of the program hosting this handler.
fn station_paths(cwd: &Path, open: &[PathBuf]) -> Vec<PathBuf> {
    denied_station_paths(
        cwd,
        open,
        ai6::station::safehouse::state_dir().ok(),
        std::env::var_os("AI6_HOME").map(PathBuf::from),
        std::env::current_exe().ok(),
    )
}

/// [`station_paths`] from the given state directory, `AI6_HOME` and program. The program's
/// directory is denied unless it holds or lies in an `open` directory, which keeps it readable,
/// and then only the program itself is. A path that is or holds `cwd` or an `open` directory is
/// left out: its deny would take those with it.
fn denied_station_paths(
    cwd: &Path,
    open: &[PathBuf],
    state: Option<PathBuf>,
    home: Option<PathBuf>,
    program: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = if cfg!(windows) {
        Vec::new()
    } else {
        STATION_CONFIG_ROOTS.iter().map(PathBuf::from).collect()
    };
    candidates.extend(state);
    candidates.extend(home.filter(|home| !home.as_os_str().is_empty()));
    if let Some(program) = program.map(|program| canonical(&program)) {
        let overlaps = |dir: &Path| {
            open.iter()
                .any(|open| starts_with(dir, open) || starts_with(open, dir))
        };
        match program.parent() {
            Some(dir) if !overlaps(dir) => candidates.push(dir.to_path_buf()),
            _ => candidates.push(program),
        }
    }
    let cwd = canonical(cwd);
    let mut paths: Vec<PathBuf> = Vec::new();
    for path in candidates.iter().map(|path| canonical(path)) {
        if starts_with(&cwd, &path)
            || open.iter().any(|dir| starts_with(dir, &path))
            || paths.contains(&path)
        {
            continue;
        }
        paths.push(path);
    }
    paths
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
        let argv = overrides(
            "codex-not-on-path",
            &cwd.to_string_lossy(),
            &BTreeMap::from([("PATH".to_owned(), String::new())]),
            &mode,
        );
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
            &overrides(
                "codex-not-on-path",
                &cwd.to_string_lossy(),
                &BTreeMap::from([("PATH".to_owned(), String::new())]),
                &AssistantMode::moneypenny(),
            ),
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
        assert_eq!(
            filesystem[&path_text(&canonical(&cwd))].as_str(),
            Some("read"),
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
