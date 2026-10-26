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
/// ConPTY sends at start and waits on. Its JSON lines arrive whole through [`JsonLines`] on
/// the widest terminal, including one longer than that.
#[test]
fn a_child_waiting_for_its_input_answers_without_help() {
    const INPUT: usize = 4_000;
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
    // A Unix pty in canonical mode cuts an input line at 4095 bytes (Linux N_TTY).
    session
        .write(format!("{}\n", "i".repeat(INPUT)).as_bytes())
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut lines = JsonLines::new();
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
