use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ai6::directorates::q::terminal::{ExitInfo, Spec, Terminal, TerminalSession};
use ai6::guatiao::{FromValue, ToValue, library::Object};
use ai6_terminal_pipe::{Pipe, PipeConfig, StderrMode};

const INPUT: usize = 70_000;

fn spec(argv: Vec<String>) -> Spec {
    Spec {
        argv,
        cwd: None,
        env: None,
        rows: 24,
        cols: 80,
    }
}

fn start(argv: Vec<String>) -> Object<dyn TerminalSession> {
    Pipe::default()
        .spawn(&spec(argv).to_value().unwrap())
        .unwrap()
}

/// This test binary, running only the test `name`, with `vars` added to its environment.
fn child_spec(name: &str, vars: &[(&str, String)]) -> Spec {
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    for (key, value) in vars {
        env.insert((*key).to_owned(), value.clone());
    }
    Spec {
        env: Some(env),
        ..spec(vec![
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            "--exact".to_owned(),
            name.to_owned(),
            "--nocapture".to_owned(),
        ])
    }
}

fn start_child(pipe: Pipe, name: &str, vars: &[(&str, String)]) -> Object<dyn TerminalSession> {
    pipe.spawn(&child_spec(name, vars).to_value().unwrap())
        .unwrap()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn read_until(session: &Object<dyn TerminalSession>, marker: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut output = Vec::new();
    let mut buffer = vec![0; 65536];
    while !contains(&output, marker) {
        let count = session.read(&mut buffer, 100).unwrap();
        if count > 0 {
            output.extend_from_slice(&buffer[..count as usize]);
        }
        assert!(
            Instant::now() < deadline,
            "the output did not contain the marker: {}",
            String::from_utf8_lossy(&output[..output.len().min(400)])
        );
    }
    output
}

/// Reads until the session reports the end of its output; fails if that takes 10 seconds.
fn read_to_end(session: &Object<dyn TerminalSession>) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    let mut buffer = vec![0; 65536];
    loop {
        let count = session.read(&mut buffer, 100).unwrap();
        if count == 0 {
            return output;
        }
        if count > 0 {
            output.extend_from_slice(&buffer[..count as usize]);
        }
        assert!(
            Instant::now() < deadline,
            "the output never ended: {}",
            String::from_utf8_lossy(&output[..output.len().min(400)])
        );
    }
}

fn exit_now(session: &Object<dyn TerminalSession>) -> ExitInfo {
    ExitInfo::from_value(&session.exit_status(0).unwrap()).unwrap()
}

fn wait_for_exit(session: &Object<dyn TerminalSession>) -> ExitInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = ExitInfo::from_value(&session.exit_status(100).unwrap()).unwrap();
        if !status.running {
            return status;
        }
        assert!(Instant::now() < deadline, "the child did not exit");
    }
}

/// What [`pipe_child`] writes after the line it reads: a long line of its own, then a line
/// with a character outside ASCII.
fn payload() -> Vec<u8> {
    format!("{}\n\u{e9}\u{4e2d} done\n", "x".repeat(INPUT)).into_bytes()
}

/// The child of [`a_long_line_arrives_whole_and_the_output_is_exact`]: reads one line, then
/// reports its length and whether its standard streams are terminals, writes [`payload`] and
/// a line on standard error.
#[test]
fn pipe_child() {
    if std::env::var_os("AI6_PIPE_CHILD").is_none() {
        return;
    }
    let mut input = String::new();
    std::io::stdin().lock().read_line(&mut input).unwrap();
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "{{\"read\":{},\"stdin_tty\":{},\"stdout_tty\":{}}}",
        input.trim_end_matches('\n').len(),
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal()
    )
    .unwrap();
    out.write_all(&payload()).unwrap();
    out.flush().unwrap();
    drop(out);
    eprintln!("AI6_PIPE_STDERR");
}

/// An input line far longer than a terminal's line limit reaches the program whole, the
/// program sees no terminal, and its output arrives as written: no CR before a line feed and
/// no control sequence. Standard error's last line follows the output.
#[test]
fn a_long_line_arrives_whole_and_the_output_is_exact() {
    let session = start_child(
        Pipe::default(),
        "pipe_child",
        &[("AI6_PIPE_CHILD", "1".to_owned())],
    );
    session
        .write(format!("{}\n", "i".repeat(INPUT)).as_bytes())
        .unwrap();
    let output = read_to_end(&session);
    let report = format!("{{\"read\":{INPUT},\"stdin_tty\":false,\"stdout_tty\":false}}\n");
    assert!(
        contains(&output, report.as_bytes()),
        "the child read its line whole with no terminal: {}",
        String::from_utf8_lossy(&output[..output.len().min(400)])
    );
    assert!(contains(&output, &payload()), "the payload arrives exact");
    assert!(!output.contains(&b'\r'), "no CR was inserted");
    assert!(!output.contains(&0x1b), "no control sequence was added");
    assert!(
        output.ends_with(b"AI6_PIPE_STDERR\n"),
        "stderr's tail ends the output"
    );
    assert_eq!(exit_now(&session).code, Some(0));
}

/// The child of [`stderr_keeps_out_of_the_stream_and_only_its_tail_follows`]: a line on
/// standard output, many on standard error, then another on standard output.
#[test]
fn pipe_noisy_child() {
    if std::env::var_os("AI6_PIPE_NOISY").is_none() {
        return;
    }
    println!("AI6_OUT_FIRST");
    std::io::stdout().flush().unwrap();
    for n in 0..2000 {
        eprintln!("AI6_ERR_{n:04}");
    }
    println!("AI6_OUT_SECOND");
    std::io::stdout().flush().unwrap();
}

/// By default standard error never breaks into the program's output; only its last lines, up to
/// `stderr_tail_bytes`, follow the output once it has ended. Merged, it lands in between.
#[test]
fn stderr_keeps_out_of_the_stream_and_only_its_tail_follows() {
    let noisy = [("AI6_PIPE_NOISY", "1".to_owned())];
    let output = read_to_end(&start_child(Pipe::default(), "pipe_noisy_child", &noisy));
    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains("AI6_OUT_FIRST\nAI6_OUT_SECOND\n"),
        "{}",
        &text[..text.len().min(200)]
    );
    // The test harness's own report ends standard output; the tail comes after it.
    let errors = &text[text.find("AI6_ERR_").unwrap()..];
    assert!(text.find("test result").unwrap() < text.find("AI6_ERR_").unwrap());
    assert!(
        errors.len() <= PipeConfig::default().stderr_tail_bytes as usize,
        "{}",
        errors.len()
    );
    assert!(errors.ends_with("AI6_ERR_1999\n"), "the last line is kept");
    assert!(!errors.contains("AI6_ERR_0000"), "the first is not");
    assert!(
        errors
            .lines()
            .all(|line| line.len() == "AI6_ERR_0000".len()),
        "only whole lines"
    );

    let merged = Pipe::new(PipeConfig {
        stderr: StderrMode::Merge,
        ..PipeConfig::default()
    });
    let output = read_to_end(&start_child(merged, "pipe_noisy_child", &noisy));
    let text = String::from_utf8_lossy(&output);
    let second = text.find("AI6_OUT_SECOND").unwrap();
    assert!(
        text[..second].contains("AI6_ERR_0000"),
        "merged, it is in between"
    );
}

/// With `stderr = "drop"` the program's standard error is discarded.
#[test]
fn dropped_stderr_is_not_in_the_output() {
    let pipe = Pipe::new(PipeConfig {
        stderr: StderrMode::Drop,
        ..PipeConfig::default()
    });
    let session = start_child(pipe, "pipe_child", &[("AI6_PIPE_CHILD", "1".to_owned())]);
    session.write(b"short\n").unwrap();
    let output = read_to_end(&session);
    assert!(contains(&output, &payload()));
    assert!(!contains(&output, b"AI6_PIPE_STDERR"));
}

/// A configuration with no settings is the default: only standard error's tail kept.
#[test]
fn an_empty_configuration_keeps_only_the_stderr_tail() {
    let empty = BTreeMap::<String, String>::new().to_value().unwrap();
    let config = PipeConfig::from_value(&empty).unwrap();
    assert_eq!(config.stderr, StderrMode::Tail);
    assert_eq!(config, PipeConfig::default());
    assert_eq!(PipeConfig::default().stderr, StderrMode::Tail);
    for (name, mode) in [
        ("tail", StderrMode::Tail),
        ("merge", StderrMode::Merge),
        ("drop", StderrMode::Drop),
    ] {
        let value = BTreeMap::from([("stderr".to_owned(), name.to_owned())])
            .to_value()
            .unwrap();
        assert_eq!(PipeConfig::from_value(&value).unwrap().stderr, mode);
    }
}

/// The child of [`a_child_that_exits_early_ends_its_output`]: writes a line and exits with
/// code 3 before reading any input.
#[test]
fn pipe_exit_child() {
    if std::env::var_os("AI6_PIPE_EXIT").is_none() {
        return;
    }
    println!("AI6_PIPE_FATAL: the harness gave up");
    std::io::stdout().flush().unwrap();
    std::process::exit(3);
}

#[test]
fn a_child_that_exits_early_ends_its_output() {
    let session = start_child(
        Pipe::default(),
        "pipe_exit_child",
        &[("AI6_PIPE_EXIT", "1".to_owned())],
    );
    let output = read_to_end(&session);
    assert!(
        contains(&output, b"AI6_PIPE_FATAL"),
        "the last line is read before the end: {}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(exit_now(&session).code, Some(3));
    let mut byte = [0];
    assert_eq!(session.read(&mut byte, 100).unwrap(), 0, "the end is final");
}

#[test]
fn a_shell_command_runs_and_reports_its_exit() {
    #[cfg(windows)]
    let argv = vec![
        "cmd.exe".to_owned(),
        "/D".to_owned(),
        "/C".to_owned(),
        "echo AI6_PIPE_READY& exit 5".to_owned(),
    ];
    #[cfg(unix)]
    let argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "printf AI6_PIPE_READY; exit 5".to_owned(),
    ];
    let session = start(argv);
    session.resize(40, 120).unwrap();
    let output = read_to_end(&session);
    assert!(contains(&output, b"AI6_PIPE_READY"));
    let status = exit_now(&session);
    assert_eq!((status.code, status.signal), (Some(5), None));
}

/// A grandchild: appends a byte to the file `AI6_PIPE_BEAT` names every 20 ms, or with
/// `AI6_PIPE_SLEEP` only sleeps, for at most 30 s.
#[test]
fn pipe_grandchild() {
    let beat = std::env::var_os("AI6_PIPE_BEAT").map(PathBuf::from);
    if beat.is_none() && std::env::var_os("AI6_PIPE_SLEEP").is_none() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(beat) = &beat {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(beat)
                .unwrap();
            file.write_all(b".").unwrap();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A child that starts [`pipe_grandchild`] with its own environment, which inherits the
/// child's output, then says so; with `AI6_PIPE_PARENT=exit` it exits with code 4, otherwise
/// it waits for the grandchild. With `AI6_PIPE_PARENT=detach` the grandchild leaves the child's
/// session and process group (`setsid`), or on Windows starts detached in a process group of
/// its own.
#[test]
fn pipe_parent() {
    let Some(mode) = std::env::var_os("AI6_PIPE_PARENT") else {
        return;
    };
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "pipe_grandchild", "--nocapture"]);
    command.env_remove("AI6_PIPE_PARENT");
    if mode == "detach" {
        detach(&mut command);
    }
    let mut grandchild = command.spawn().unwrap();
    println!("AI6_PIPE_SPAWNED");
    std::io::stdout().flush().unwrap();
    if mode == "exit" {
        std::process::exit(4);
    }
    let _ = grandchild.wait();
}

#[cfg(unix)]
fn detach(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe and touches no memory of the parent.
    unsafe {
        command.pre_exec(|| match libc::setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
}

#[cfg(windows)]
fn detach(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

/// `kill` ends the program's grandchildren too, and returns once the program has exited.
#[test]
fn kill_ends_a_grandchild() {
    kill_ends_a_grandchild_started("sleep");
}

/// `kill` also ends a grandchild that left the program's session and process group.
#[test]
fn kill_ends_a_grandchild_that_left_the_process_group() {
    kill_ends_a_grandchild_started("detach");
}

fn kill_ends_a_grandchild_started(mode: &str) {
    let dir = tempfile_dir(&format!("kill-{mode}"));
    let beat = dir.join("beat");
    let session = start_child(
        Pipe::default(),
        "pipe_parent",
        &[
            ("AI6_PIPE_PARENT", mode.to_owned()),
            ("AI6_PIPE_BEAT", beat.to_string_lossy().into_owned()),
        ],
    );
    read_until(&session, b"AI6_PIPE_SPAWNED");
    let deadline = Instant::now() + Duration::from_secs(20);
    while size(&beat) < 3 {
        assert!(Instant::now() < deadline, "the grandchild never beat");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(exit_now(&session).running);
    session.kill().unwrap();
    assert!(
        !exit_now(&session).running,
        "kill returned before the child exited"
    );
    session.kill().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let after_kill = size(&beat);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(size(&beat), after_kill, "the grandchild still runs");
    read_to_end(&session);
    let _ = std::fs::remove_dir_all(dir);
}

/// A process the child left running keeps the output pipe open after the child has exited;
/// the output still ends.
#[test]
fn a_child_that_exits_leaving_a_process_behind_ends_its_output() {
    let session = start_child(
        Pipe::default(),
        "pipe_parent",
        &[
            ("AI6_PIPE_PARENT", "exit".to_owned()),
            ("AI6_PIPE_SLEEP", "1".to_owned()),
        ],
    );
    let output = read_to_end(&session);
    assert!(contains(&output, b"AI6_PIPE_SPAWNED"));
    assert_eq!(wait_for_exit(&session).code, Some(4));
}

/// What the file whose handle [`the_child_inherits_nothing_but_its_pipes`] leaks holds.
const LEAK_MARKER: &str = "AI6_PIPE_LEAKED_HANDLE";

/// The child of [`the_child_inherits_nothing_but_its_pipes`]: reports whether the descriptor or
/// handle number in `AI6_PIPE_LEAK` reads the leaked file, and whether it leads a session of
/// its own (always `true` on Windows, which has none).
#[test]
fn pipe_leak_child() {
    let Ok(raw) = std::env::var("AI6_PIPE_LEAK") else {
        return;
    };
    let leaked = read_inherited(raw.parse().unwrap()).is_some_and(|text| text == LEAK_MARKER);
    println!("{{\"leaked\":{leaked},\"leader\":{}}}", session_leader());
}

#[cfg(unix)]
fn read_inherited(fd: i32) -> Option<String> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    // SAFETY: the file is never closed here, so a descriptor the process does not own is left
    // alone; reading one that is not open fails.
    let mut file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
    let mut text = String::new();
    file.read_to_string(&mut text).ok().map(|_| text)
}

#[cfg(windows)]
fn read_inherited(handle: usize) -> Option<String> {
    use std::io::Read;
    use std::os::windows::io::FromRawHandle;
    // SAFETY: as on Unix, the handle is never closed here.
    let mut file =
        std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(handle as _) });
    let mut text = String::new();
    file.read_to_string(&mut text).ok().map(|_| text)
}

#[cfg(unix)]
fn session_leader() -> bool {
    // SAFETY: getsid and getpid take and return plain integers.
    unsafe { libc::getsid(0) == libc::getpid() }
}

#[cfg(windows)]
fn session_leader() -> bool {
    true
}

/// Opens `path` with a descriptor or handle every child the test process starts would inherit,
/// and returns it with its number.
#[cfg(unix)]
fn inheritable(path: &Path) -> (std::fs::File, String) {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(path).unwrap();
    // SAFETY: fcntl on a descriptor this function owns.
    assert_eq!(
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) },
        0
    );
    let fd = file.as_raw_fd().to_string();
    (file, fd)
}

#[cfg(windows)]
fn inheritable(path: &Path) -> (std::fs::File, String) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    let file = std::fs::File::open(path).unwrap();
    // SAFETY: SetHandleInformation on a handle this function owns.
    let set = unsafe {
        SetHandleInformation(
            file.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    };
    assert_ne!(set, 0);
    let handle = (file.as_raw_handle() as usize).to_string();
    (file, handle)
}

/// The program inherits only its standard streams, not a descriptor or handle the Station left
/// inheritable, and on Unix leads a session of its own.
#[test]
fn the_child_inherits_nothing_but_its_pipes() {
    let dir = tempfile_dir("leak");
    let path = dir.join("marker");
    std::fs::write(&path, LEAK_MARKER).unwrap();
    let (file, raw) = inheritable(&path);
    let session = start_child(
        Pipe::default(),
        "pipe_leak_child",
        &[("AI6_PIPE_LEAK", raw)],
    );
    let output = read_to_end(&session);
    drop(file);
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("\"leaked\":false"), "{text}");
    assert!(text.contains("\"leader\":true"), "{text}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The child of [`a_batch_file_gets_its_arguments_exactly`]: prints the arguments it was given
/// after its own, as JSON.
#[test]
fn pipe_args_child() {
    if std::env::var_os("AI6_PIPE_ARGS").is_none() {
        return;
    }
    let args: Vec<String> = std::env::args()
        .skip_while(|arg| arg != "--nocapture")
        .skip(1)
        .collect();
    println!("ARGS {}", serde_json_like(&args));
}

/// `args` as a JSON array of strings, escaping only what the test's arguments hold.
fn serde_json_like(args: &[String]) -> String {
    let quoted: Vec<String> = args
        .iter()
        .map(|arg| format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", quoted.join(","))
}

/// A `.cmd` file, as npm installs `codex.cmd`, gets each argument as given: `cmd.exe` neither
/// expands a variable in one nor ends the command at a `&`.
#[cfg(windows)]
#[test]
fn a_batch_file_gets_its_arguments_exactly() {
    let dir = tempfile_dir("batch");
    let script = dir.join("relay.cmd");
    std::fs::write(
        &script,
        format!(
            "@\"{}\" --exact pipe_args_child --nocapture %*\r\n",
            std::env::current_exe().unwrap().display()
        ),
    )
    .unwrap();
    let args = [
        "plain",
        "two words",
        r#"k="v w""#,
        "%PATH%",
        "a&b|c^d",
        r"C:\dir\",
        "",
    ];
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    env.insert("AI6_PIPE_ARGS".to_owned(), "1".to_owned());
    let mut argv = vec![script.to_string_lossy().into_owned()];
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    let session = Pipe::default()
        .spawn(
            &Spec {
                env: Some(env),
                ..spec(argv)
            }
            .to_value()
            .unwrap(),
        )
        .unwrap();
    let output = String::from_utf8_lossy(&read_to_end(&session)).into_owned();
    let expected: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    assert!(
        output.contains(&format!("ARGS {}", serde_json_like(&expected))),
        "{output}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A Station reads this to give the provider to harnesses that speak JSON.
#[test]
fn the_provider_says_it_gives_no_terminal() {
    assert!(!Pipe::default().tty());
}

#[test]
fn an_empty_argv_or_a_missing_program_is_refused() {
    assert!(
        Pipe::default()
            .spawn(&spec(Vec::new()).to_value().unwrap())
            .is_err()
    );
    assert!(
        Pipe::default()
            .spawn(
                &spec(vec!["ai6-no-such-program".to_owned()])
                    .to_value()
                    .unwrap()
            )
            .is_err()
    );
}

/// A fresh directory under the target directory, which the test removes.
fn tempfile_dir(name: &str) -> PathBuf {
    let dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("pipe-scratch")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A smaller `stderr_tail_bytes` keeps less of standard error; a read of no bytes is refused.
#[test]
fn the_configured_tail_and_chunk_are_observed() {
    let noisy = [("AI6_PIPE_NOISY", "1".to_owned())];
    let small = Pipe::new(PipeConfig {
        stderr_tail_bytes: 128,
        read_chunk_bytes: 7,
        ..PipeConfig::default()
    });
    let output = read_to_end(&start_child(small, "pipe_noisy_child", &noisy));
    let text = String::from_utf8_lossy(&output);
    let errors = &text[text.find("AI6_ERR_").unwrap()..];
    assert!(errors.len() <= 128, "{}", errors.len());
    assert!(errors.ends_with("AI6_ERR_1999\n"), "the last line is kept");

    assert!(
        Pipe::try_from(PipeConfig {
            read_chunk_bytes: 0,
            ..PipeConfig::default()
        })
        .is_err()
    );
}
