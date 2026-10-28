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
use ai6::directorates::section_00::assistant::{SCRIPT_ENVIRONMENT, credential_dir_var};

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
    let cwd = Path::new(cwd);
    let mut rules = vec![
        (":root".to_owned(), "deny"),
        (":minimal".to_owned(), "read"),
        (path_text(cwd), "read"),
    ];
    rules.extend(
        read_dirs(executable, cwd, environment, &mode.interpreters)
            .into_iter()
            .map(|dir| (path_text(&dir), "read")),
    );
    rules.extend(
        mode.files
            .iter()
            .map(|dir| (path_text(&cwd.join(native(dir))), "write")),
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

/// `path` as text, without Windows' verbatim `\\?\` prefix, which Codex does not expect.
fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(&text)
        .to_owned()
}

/// The directories scripts also read: where the codex executable is (bubblewrap runs it again
/// inside the sandbox on Linux) and where each interpreter is, as found on the harness's
/// `PATH` with links followed. A directory that is or holds the home directory, the working
/// directory or its parent (other conversations), or a credential directory, or lies inside a
/// credential directory, is left out: an interpreter there is unreachable.
fn read_dirs(
    executable: &str,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    interpreters: &[String],
) -> Vec<PathBuf> {
    let credentials: Vec<PathBuf> = environment
        .iter()
        .filter(|(name, _)| credential_dir_var(name))
        .map(|(_, path)| canonical(Path::new(path)))
        .collect();
    let mut kept: Vec<PathBuf> = ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(|name| variable(environment, name).or_else(|| std::env::var(name).ok()))
        .map(|home| canonical(Path::new(&home)))
        .collect();
    kept.push(canonical(cwd));
    kept.extend(cwd.parent().map(canonical));
    kept.extend(credentials.iter().cloned());
    let mut dirs = Vec::new();
    for program in std::iter::once(executable).chain(interpreters.iter().map(String::as_str)) {
        let Some(dir) = resolve(program, environment)
            .and_then(|found| canonical(&found).parent().map(Path::to_path_buf))
        else {
            continue;
        };
        let holds = |inner: &Path| starts_with(inner, &dir);
        if kept.iter().any(|path| holds(path))
            || credentials.iter().any(|path| starts_with(&dir, path))
            || dirs.contains(&dir)
        {
            continue;
        }
        dirs.push(dir);
    }
    dirs
}

/// `name`'s value in `environment`, case ignored as Windows ignores it.
fn variable(environment: &BTreeMap<String, String>, name: &str) -> Option<String> {
    environment
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// `path` with links followed, or as given when it cannot be resolved.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|resolved| PathBuf::from(path_text(&resolved)))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Whether `path` is `base` or lies under it, case ignored on Windows.
fn starts_with(path: &Path, base: &Path) -> bool {
    if cfg!(windows) {
        let lower = |path: &Path| PathBuf::from(path_text(path).to_lowercase());
        lower(path).starts_with(lower(base))
    } else {
        path.starts_with(base)
    }
}

/// Where `program` is: itself when it names a path, else the first match on the `PATH` of
/// `environment` (the process's own when it has none), with Windows' executable extensions.
fn resolve(program: &str, environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    let given = Path::new(program);
    if given.components().count() > 1 || given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    let path = variable(environment, "PATH").or_else(|| std::env::var("PATH").ok())?;
    let extensions: Vec<String> = if cfg!(windows) && given.extension().is_none() {
        variable(environment, "PATHEXT")
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        extensions
            .iter()
            .map(|ext| dir.join(format!("{program}{ext}")))
            .find(|candidate| candidate.is_file())
    })
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
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ai6-codex-probe-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return Some(format!(
            "the sandbox probe's directory was not made: {error}"
        ));
    }
    let answer = probe_in(executable, environment, runner, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    answer
}

fn probe_in(
    executable: &str,
    environment: &BTreeMap<String, String>,
    runner: &Runner<'_>,
    dir: &Path,
) -> Option<String> {
    let mut rules = vec![
        (":root".to_owned(), "deny"),
        (":minimal".to_owned(), "read"),
        (path_text(&canonical(dir)), "read"),
    ];
    rules.extend(
        read_dirs(executable, dir, environment, &[])
            .into_iter()
            .map(|dir| (path_text(&dir), "read")),
    );
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
    let program = resolve(&argv[0], environment).unwrap_or_else(|| PathBuf::from(&argv[0]));
    let mut child = std::process::Command::new(program)
        .args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = child.wait_with_output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    match status {
        Some(status) => Ok((status.success(), text)),
        None => Ok((
            false,
            format!("{text}\ndid not finish in {PROBE_TIMEOUT:?}"),
        )),
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
        assert_eq!(filesystem.len(), 5, "nothing else: {filesystem:?}");
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
