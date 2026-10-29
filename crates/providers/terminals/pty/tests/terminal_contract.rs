use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use ai6::directorates::q::terminal::{
    ExitInfo, JsonLines, STREAM_COLS, Spec, StreamLine, Terminal, TerminalSession,
};
use ai6::guatiao::{FromValue, ToValue, library::Object};
use ai6_terminal_pty::Pty;

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
    Pty.spawn(&spec(argv).to_value().unwrap()).unwrap()
}

fn read_until(session: &Object<dyn TerminalSession>, marker: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut buffer = [0; 1024];
    while !output.windows(marker.len()).any(|window| window == marker) {
        let count = session.read(&mut buffer, 100).unwrap();
        if count > 0 {
            output.extend_from_slice(&buffer[..count as usize]);
        }
        assert!(
            Instant::now() < deadline,
            "PTY output did not contain marker: {}",
            String::from_utf8_lossy(&output)
        );
    }
    output
}

fn wait_for_exit(session: &Object<dyn TerminalSession>) -> ExitInfo {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = session.exit_status(0).unwrap();
        let status = ExitInfo::from_value(&value).unwrap();
        if !status.running {
            return status;
        }
        assert!(Instant::now() < deadline, "PTY child did not exit");
        let mut byte = [0];
        let _ = session.read(&mut byte, 20).unwrap();
    }
}

#[test]
fn a_pty_child_emits_output_and_reports_exit() {
    #[cfg(windows)]
    let argv = vec![
        "cmd.exe".to_owned(),
        "/D".to_owned(),
        "/C".to_owned(),
        "echo AI6_PTY_READY".to_owned(),
    ];
    #[cfg(unix)]
    let argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "printf AI6_PTY_READY".to_owned(),
    ];

    let session = start(argv);
    let output = read_until(&session, b"AI6_PTY_READY");
    assert!(output.windows(13).any(|window| window == b"AI6_PTY_READY"));
    let status = wait_for_exit(&session);
    assert_eq!(status.code, Some(0));
    assert_eq!(status.signal, None);
}

/// The child of [`a_child_waiting_for_its_input_answers_without_help`]: this test binary run
/// again with `AI6_PTY_CHILD` set. It waits for one line, then writes one JSON line per
/// length in `AI6_PTY_LINES`, each holding the length of what it read.
#[test]
fn pty_child() {
    let Ok(lengths) = std::env::var("AI6_PTY_LINES") else {
        return;
    };
    let mut input = String::new();
    std::io::stdin().lock().read_line(&mut input).unwrap();
    let read = input.trim_end().len();
    let mut out = std::io::stdout().lock();
    for (n, length) in lengths.split(',').enumerate() {
        let text = format!(
            "{}  {}",
            "x".repeat(length.parse().unwrap()),
            "\u{e9}\u{4e2d}"
        );
        writeln!(out, "{{\"n\":{n},\"read\":{read},\"text\":\"{text}\"}}").unwrap();
    }
    out.flush().unwrap();
}

/// A program that reads its input before writing anything, as Claude Code and Codex do,
/// answers with no help from the caller: nobody here answers a cursor position query, which
/// ConPTY sends at start and waits on. It reads an input line far longer than a Unix
/// terminal's canonical-mode limit whole, and its JSON lines arrive whole through
/// [`JsonLines`] on the widest terminal, including one longer than that.
#[test]
fn a_child_waiting_for_its_input_answers_without_help() {
    const INPUT: usize = 70_000;
    let lengths = [300, 70_000, 1_000];
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    env.insert(
        "AI6_PTY_LINES".to_owned(),
        lengths.map(|n| n.to_string()).join(","),
    );
    let argv = [
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        "--exact".to_owned(),
        "pty_child".to_owned(),
        "--nocapture".to_owned(),
    ];
    let session = Pty
        .spawn(
            &Spec {
                argv: argv.to_vec(),
                cwd: None,
                env: Some(env),
                rows: 24,
                cols: STREAM_COLS,
            }
            .to_value()
            .unwrap(),
        )
        .unwrap();
    session
        .write(format!("{}\n", "i".repeat(INPUT)).as_bytes())
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut lines = JsonLines::new(&Default::default());
    let mut raw = Vec::new();
    let mut values = Vec::new();
    let mut buffer = vec![0; 65536];
    while values.len() < lengths.len() {
        assert!(
            Instant::now() < deadline,
            "the child never answered; it wrote {:?}",
            String::from_utf8_lossy(&raw[..raw.len().min(400)])
        );
        let count = session.read(&mut buffer, 100).unwrap();
        if count > 0 {
            raw.extend_from_slice(&buffer[..count as usize]);
            lines.push(&buffer[..count as usize]);
        }
        while let Some(line) = lines.next_line() {
            if let StreamLine::Json { value, .. } = line {
                values.push(value);
            }
        }
    }
    for (n, (value, length)) in values.iter().zip(lengths).enumerate() {
        assert_eq!(value.get("n").and_then(|n| n.as_u64()), Some(n as u64));
        assert_eq!(
            value.get("read").and_then(|n| n.as_u64()),
            Some(INPUT as u64)
        );
        let text = format!("{}  {}", "x".repeat(length), "\u{e9}\u{4e2d}");
        assert_eq!(value.get("text").and_then(|t| t.as_str()), Some(&*text));
    }
    assert!(
        !raw.windows(4).any(|window| window == b"\x1b[6n"),
        "the query was answered and removed"
    );
    assert_eq!(wait_for_exit(&session).code, Some(0));
}

/// Input written to the program does not come back in its output.
#[cfg(unix)]
#[test]
fn written_input_is_not_echoed() {
    let session = start(vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "IFS= read -r line; printf 'AI6_READ_%s.' \"${#line}\"".to_owned(),
    ]);
    session.write(b"AI6_SECRET_INPUT\n").unwrap();
    let output = read_until(&session, b"AI6_READ_16.");
    assert!(
        !output.windows(10).any(|window| window == b"AI6_SECRET"),
        "the input was echoed: {}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(wait_for_exit(&session).code, Some(0));
}

/// A ^C written to the terminal interrupts its program, which is how the Claude Code handler
/// interrupts a turn.
#[cfg(unix)]
#[test]
fn a_written_ctrl_c_interrupts_the_program() {
    let session = start(vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "exec sleep 30".to_owned(),
    ]);
    session.write(&[3]).unwrap();
    let status = wait_for_exit(&session);
    assert!(status.signal.is_some(), "it exited with {status:?}");
}

/// Reads until the session reports the end of its output; fails if that takes 10 seconds.
fn read_to_end(session: &Object<dyn TerminalSession>) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    let mut buffer = [0; 1024];
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
            "the output never ended after the child exited: {}",
            String::from_utf8_lossy(&output)
        );
    }
}

/// The child of [`a_child_that_exits_early_ends_its_output`]: a harness that writes an error
/// line and exits with code 3 before reading any input.
#[test]
fn pty_exit_child() {
    if std::env::var_os("AI6_PTY_EXIT").is_none() {
        return;
    }
    println!("AI6_PTY_FATAL: the harness gave up");
    std::io::stdout().flush().unwrap();
    std::process::exit(3);
}

/// A harness that exits on its own ends the output once what it wrote has been read, although
/// ConPTY keeps the output open until the pseudo-console is closed.
#[test]
fn a_child_that_exits_early_ends_its_output() {
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    env.insert("AI6_PTY_EXIT".to_owned(), "1".to_owned());
    let argv = vec![
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        "--exact".to_owned(),
        "pty_exit_child".to_owned(),
        "--nocapture".to_owned(),
    ];
    let session = Pty
        .spawn(
            &Spec {
                env: Some(env),
                cols: STREAM_COLS,
                ..spec(argv)
            }
            .to_value()
            .unwrap(),
        )
        .unwrap();
    let output = read_to_end(&session);
    assert!(
        output.windows(13).any(|window| window == b"AI6_PTY_FATAL"),
        "the last line is read before the end: {}",
        String::from_utf8_lossy(&output)
    );
    let status = ExitInfo::from_value(&session.exit_status(0).unwrap()).unwrap();
    assert_eq!(status.code, Some(3));
    let mut byte = [0];
    assert_eq!(session.read(&mut byte, 100).unwrap(), 0, "the end is final");
}

/// A process the child left running keeps a Unix terminal open after the child has exited,
/// as ConPTY does on Windows; the output still ends.
#[cfg(unix)]
#[test]
fn a_child_that_exits_leaving_a_process_behind_ends_its_output() {
    let session = start(vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "sleep 30 & printf AI6_PTY_LEFT; exit 4".to_owned(),
    ]);
    let output = read_to_end(&session);
    assert!(output.windows(12).any(|window| window == b"AI6_PTY_LEFT"));
    let status = ExitInfo::from_value(&session.exit_status(0).unwrap()).unwrap();
    assert_eq!(status.code, Some(4));
}

/// `kill` returns once the child has exited, not when it was only told to.
#[test]
fn kill_returns_once_the_child_has_exited() {
    #[cfg(windows)]
    let argv = vec![
        "ping.exe".to_owned(),
        "-n".to_owned(),
        "30".to_owned(),
        "127.0.0.1".to_owned(),
    ];
    #[cfg(unix)]
    let argv = vec!["sh".to_owned(), "-c".to_owned(), "exec sleep 30".to_owned()];
    let session = start(argv);
    let mut byte = [0];
    let _ = session.read(&mut byte, 500).unwrap();
    session.kill().unwrap();
    let status = ExitInfo::from_value(&session.exit_status(0).unwrap()).unwrap();
    assert!(!status.running, "kill returned before the child exited");
}

#[test]
fn kill_ends_a_running_pty_process() {
    #[cfg(windows)]
    let argv = vec![
        "ping.exe".to_owned(),
        "-t".to_owned(),
        "127.0.0.1".to_owned(),
    ];
    #[cfg(unix)]
    let argv = vec!["sh".to_owned(), "-c".to_owned(), "exec sleep 30".to_owned()];

    let session = start(argv);
    session.resize(40, 120).unwrap();
    let value = session.exit_status(0).unwrap();
    assert!(ExitInfo::from_value(&value).unwrap().running);
    session.kill().unwrap();
    assert!(!wait_for_exit(&session).running);
}

/// A grandchild: appends a byte to the file `AI6_PTY_BEAT` names every 20 ms, for at most 30 s.
#[test]
fn pty_grandchild() {
    let Some(beat) = std::env::var_os("AI6_PTY_BEAT") else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&beat)
            .unwrap();
        file.write_all(b".").unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A child that starts [`pty_grandchild`] outside its own session and process group
/// (`setsid`), or on Windows detached in a process group of its own, says so, and waits for it.
#[test]
fn pty_detacher() {
    if std::env::var_os("AI6_PTY_DETACH").is_none() {
        return;
    }
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "pty_grandchild", "--nocapture"]);
    command.env_remove("AI6_PTY_DETACH");
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut command);
    let mut grandchild = command.spawn().unwrap();
    println!("AI6_PTY_SPAWNED");
    std::io::stdout().flush().unwrap();
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

/// `kill` ends a grandchild that left the program's session and process group.
#[test]
fn kill_ends_a_grandchild_that_left_the_process_group() {
    let dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("pty-scratch")
        .join(format!("detach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let beat = dir.join("beat");
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    env.insert("AI6_PTY_DETACH".to_owned(), "1".to_owned());
    env.insert(
        "AI6_PTY_BEAT".to_owned(),
        beat.to_string_lossy().into_owned(),
    );
    let argv = vec![
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        "--exact".to_owned(),
        "pty_detacher".to_owned(),
        "--nocapture".to_owned(),
    ];
    let session = Pty
        .spawn(
            &Spec {
                env: Some(env),
                ..spec(argv)
            }
            .to_value()
            .unwrap(),
        )
        .unwrap();
    read_until(&session, b"AI6_PTY_SPAWNED");
    let size = || std::fs::metadata(&beat).map_or(0, |meta| meta.len());
    let deadline = Instant::now() + Duration::from_secs(20);
    while size() < 3 {
        assert!(Instant::now() < deadline, "the grandchild never beat");
        std::thread::sleep(Duration::from_millis(20));
    }
    session.kill().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let after_kill = size();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(size(), after_kill, "the grandchild still runs");
    let _ = std::fs::remove_dir_all(dir);
}
