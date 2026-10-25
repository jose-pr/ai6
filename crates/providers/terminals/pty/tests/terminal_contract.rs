use std::time::{Duration, Instant};

use ai6::directorates::q::terminal::{ExitInfo, Spec, Terminal, TerminalSession};
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
            if output.windows(4).any(|window| window == b"\x1b[6n") {
                session.write(b"\x1b[24;1R").unwrap();
            }
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
