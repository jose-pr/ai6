//! The `ssh` transport against a real OpenSSH server.
//!
//! On Linux the test starts a private `sshd` (no root, loopback, a throwaway host key). On
//! Windows it starts the same inside a WSL distribution (`AI6_TEST_WSL_DISTRO`, default
//! `FedoraLinux-44`) and reaches it through WSL's localhost relay. Where there is no
//! `sshd` to start, the test says why it skipped and passes.

use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::guatiao::{Bytes, FromValue, ToValue};
use ai6::{
    ExitInfo, ForwardSpec, ReadSpec, RunSpec, Spec, Transport, TransportClient, TransportConfig,
    WriteSpec,
};
use ai6_transport_ssh::Ssh;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, PrivateKey};

/// Where distributions put the server and its SFTP subsystem (Fedora and macOS, Debian).
const SSHD_PATHS: &str = "/usr/sbin/sshd /usr/local/sbin/sshd";
const SFTP_PATHS: &str = "/usr/libexec/openssh/sftp-server /usr/lib/openssh/sftp-server";

/// A command that reads a bash script from its standard input where the `sshd` lives.
fn on_host() -> Command {
    if cfg!(windows) {
        let distro =
            std::env::var("AI6_TEST_WSL_DISTRO").unwrap_or_else(|_| "FedoraLinux-44".to_owned());
        let mut command = Command::new("wsl.exe");
        command.args(["-d", &distro, "-e", "bash", "-s"]);
        command
    } else {
        let mut command = Command::new("bash");
        command.arg("-s");
        command
    }
}

/// Runs `script` on the host and answers its standard output.
fn output_of(script: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = on_host()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start the host shell: {error}"))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .map_err(|error| format!("could not send the script: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("the host shell failed: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout).replace('\0', "");
    if output.status.success() {
        Ok(text.trim().to_owned())
    } else {
        Err(format!(
            "exit {:?}: {}{}",
            output.status.code(),
            text.trim(),
            String::from_utf8_lossy(&output.stderr).replace('\0', "")
        ))
    }
}

struct Sshd {
    dir: String,
    port: u16,
    user: String,
    pin: String,
    key_dir: tempfile::TempDir,
}

impl Sshd {
    /// Starts a private `sshd`, or says why it cannot.
    fn start() -> Result<Self, String> {
        let found = output_of(&format!(
            "for f in {SSHD_PATHS}; do [ -x $f ] && echo $f && break; done; \
             for f in {SFTP_PATHS}; do [ -x $f ] && echo $f && break; done; \
             command -v ssh-keygen"
        ))
        .map_err(|why| format!("no OpenSSH server to start ({why})"))?;
        let [sshd, sftp_server, _] = found.lines().collect::<Vec<_>>()[..] else {
            return Err(format!(
                "no OpenSSH server to start (found only: {found:?})"
            ));
        };
        let (sshd, sftp_server) = (sshd.to_owned(), sftp_server.to_owned());

        let key_dir = tempfile::tempdir().unwrap();
        let client = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        client
            .write_openssh_file(key_dir.path().join("client_key"), LineEnding::LF)
            .unwrap();
        let authorized = client.public_key().to_openssh().unwrap();
        assert!(!authorized.contains('\''));

        let user = output_of("id -un")?;
        let dir = format!(
            "/tmp/ai6-sshd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                % 1_000_000
        );
        let mut last = String::new();
        for attempt in 0..8u16 {
            let port = 20_000
                + u16::try_from(
                    (std::process::id() as u64 * 7919 + u64::from(attempt) * 104_729) % 30_000,
                )
                .unwrap();
            if TcpListener::bind(("127.0.0.1", port)).is_err() {
                continue;
            }
            let script = format!(
                r#"set -e
rm -rf {dir}; mkdir -p {dir}/scratch
ssh-keygen -q -t ed25519 -N '' -f {dir}/host_key
printf '%s\n' '{authorized}' > {dir}/authorized_keys
chmod 600 {dir}/authorized_keys {dir}/host_key
cat > {dir}/sshd_config <<CONFIG
Port {port}
ListenAddress 127.0.0.1
HostKey {dir}/host_key
AuthorizedKeysFile {dir}/authorized_keys
PidFile {dir}/pid
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
StrictModes no
AllowTcpForwarding yes
Subsystem sftp {sftp_server}
LogLevel ERROR
CONFIG
setsid nohup {sshd} -D -f {dir}/sshd_config -E {dir}/log > /dev/null 2>&1 < /dev/null &
sleep 1
test -s {dir}/pid
echo started
"#
            );
            output_of(&script)?;
            // The server is ready when the port answers.
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    let pin = output_of(&format!(
                        "awk '{{print $2}}' {dir}/host_key.pub | base64 -d | sha256sum | cut -d' ' -f1"
                    ))?;
                    return Ok(Self {
                        dir,
                        port,
                        user,
                        pin: format!("sha256:{pin}"),
                        key_dir,
                    });
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            last = output_of(&format!(
                "cat {dir}/log 2>&1; pkill -f '{dir}/sshd_config' || true"
            ))
            .unwrap_or_default();
        }
        let _ = output_of(&format!("rm -r -f {dir}"));
        Err(format!("sshd did not come up: {last}"))
    }

    fn identity(&self) -> String {
        self.key_dir
            .path()
            .join("client_key")
            .to_string_lossy()
            .into_owned()
    }

    fn settings(&self, pin: Option<&str>, extra: &str) -> String {
        let pin = pin.map_or(String::new(), |pin| format!("host_key = \"{pin}\"\n"));
        format!(
            "host = \"127.0.0.1\"\nport = {}\nuser = \"{}\"\nidentity = {:?}\n{pin}{extra}",
            self.port,
            self.user,
            self.identity(),
        )
    }

    fn client(&self, pin: Option<&str>, extra: &str) -> TransportClient {
        TransportClient::new(Box::new(self.ssh(pin, extra)))
    }

    fn ssh(&self, pin: Option<&str>, extra: &str) -> Ssh {
        Ssh::try_from(TransportConfig {
            settings_toml: self.settings(pin, extra),
        })
        .unwrap()
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = output_of(&format!(
            "kill $(cat {dir}/pid) 2>/dev/null; sleep 0.2; rm -rf {dir}",
            dir = self.dir
        ));
    }
}

fn run(client: &TransportClient, script: &str) -> ai6::Output {
    client
        .run(&RunSpec {
            argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            ..RunSpec::default()
        })
        .unwrap()
}

fn text(bytes: &Bytes) -> String {
    String::from_utf8_lossy(&bytes.0).into_owned()
}

#[test]
fn the_ssh_transport_works_against_a_real_openssh_server() {
    let server = match Sshd::start() {
        Ok(server) => server,
        Err(why) => {
            eprintln!("SKIPPED: {why}");
            return;
        }
    };
    let pinned = server.client(Some(&server.pin), "");
    let scratch = format!("{}/scratch", server.dir);

    // The pin is the SHA-256 of the key blob, as `sha256sum` computes it.
    let out = run(&pinned, "echo ready");
    assert_eq!(
        (out.code, text(&out.stdout)),
        (Some(0), "ready\n".to_owned())
    );

    // The contract every transport passes.
    contract::check(
        Box::new(server.ssh(Some(&server.pin), "")),
        &Host {
            shell: &["sh", "-c"],
            expand: |name| format!("${name}"),
            scratch: &scratch,
        },
    );

    // A host presenting another key than the pinned one is refused, and so is a missing pin.
    let wrong = format!("sha256:{}", "ab".repeat(32));
    let error = server
        .client(Some(&wrong), "")
        .run(&RunSpec {
            argv: vec!["true".to_owned()],
            ..RunSpec::default()
        })
        .expect_err("a wrong pin must be refused");
    let message = ai6::describe(&error);
    assert!(
        message.contains("not the pinned") && message.contains(&server.pin),
        "{message}"
    );
    let error = server
        .client(None, "")
        .run(&RunSpec {
            argv: vec!["true".to_owned()],
            ..RunSpec::default()
        })
        .expect_err("first contact without a pin must be refused");
    let message = ai6::describe(&error);
    assert!(
        message.contains("no host key is pinned") && message.contains(&server.pin),
        "{message}"
    );

    // Trust on first use is allowed, and shows the key it trusted.
    let tofu = server.client(None, "trust_on_first_use = true\n");
    assert_eq!(run(&tofu, "true").code, Some(0));
    let notes = tofu.capabilities().unwrap().notes;
    assert!(
        notes.iter().any(|note| note.contains(&server.pin)),
        "{notes:?}"
    );

    // What the host is.
    let probed = pinned.probe().unwrap();
    let uname = output_of("uname -m").unwrap();
    assert_eq!(probed.os.as_deref(), Some("linux"));
    assert_eq!(
        probed.arch.as_deref(),
        Some(if uname == "arm64" {
            "aarch64"
        } else {
            uname.as_str()
        })
    );
    assert!(probed.files && probed.run && probed.terminal && probed.forward);

    // Its CPUs, load and memory, as a Station samples a Theater for placement.
    let resources = ai6::probe_resources(&pinned, &probed, &Default::default())
        .expect("the host can be sampled");
    assert!(
        resources.cpus.is_some_and(|cpus| cpus >= 1.0) && resources.load_1m.is_some(),
        "{resources:?}"
    );
    let total = resources.memory_total_mb.expect("MemTotal");
    let available = resources.memory_available_mb.expect("MemAvailable");
    assert!(total > 0 && available <= total, "{resources:?}");

    // Arguments reach the program as written, however the shell would read them.
    let out = pinned
        .run(&RunSpec {
            argv: [
                "sh",
                "-c",
                "printf '%s|' \"$@\"",
                "x",
                "a b",
                "it's",
                "$HOME",
                "*",
            ]
            .map(str::to_owned)
            .to_vec(),
            cwd: Some(scratch.clone()),
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(text(&out.stdout), "a b|it's|$HOME|*|");
    let out = pinned
        .run(&RunSpec {
            argv: ["sh", "-c", "pwd; echo \"$AI6_PROBE\""]
                .map(str::to_owned)
                .to_vec(),
            cwd: Some(scratch.clone()),
            env: Some([("AI6_PROBE".to_owned(), "x y'z".to_owned())].into()),
            ..RunSpec::default()
        })
        .unwrap();
    assert!(
        text(&out.stdout).ends_with(&format!("{scratch}\nx y'z\n")),
        "{}",
        text(&out.stdout)
    );
    let out = run(&pinned, "cat; echo; exit 7");
    assert_eq!(out.code, Some(7));

    // A real pty has the size it was asked for, and a program that never exits is killed.
    let ssh = server.ssh(Some(&server.pin), "");
    let spec = Spec {
        argv: ["sh", "-c", "stty size; sleep 60"]
            .map(str::to_owned)
            .to_vec(),
        cwd: None,
        env: None,
        rows: 31,
        cols: 97,
    };
    let session = ssh.open_terminal(&spec.to_value().unwrap()).unwrap();
    let mut seen = String::new();
    let mut buffer = [0u8; 256];
    let deadline = Instant::now() + Duration::from_secs(10);
    while !seen.contains("31 97") && Instant::now() < deadline {
        let count = session.read(&mut buffer, 200).unwrap();
        if count > 0 {
            seen.push_str(&String::from_utf8_lossy(&buffer[..count as usize]));
        }
    }
    assert!(seen.contains("31 97"), "{seen:?}");
    session.kill().unwrap();
    assert!(
        !ExitInfo::from_value(&session.exit_status(2000).unwrap())
            .unwrap()
            .running
    );
    wait_until("the killed terminal's program is gone", || {
        run(
            &pinned,
            "pgrep -f '[s]leep 60' >/dev/null && echo alive || echo gone",
        )
        .stdout
        .0
        .starts_with(b"gone")
    });

    // A timeout kills the remote program, and the connection stays usable.
    let started = Instant::now();
    let out = pinned
        .run(&RunSpec {
            argv: vec!["sleep".to_owned(), "45".to_owned()],
            timeout_ms: 400,
            ..RunSpec::default()
        })
        .unwrap();
    assert!(out.timed_out && started.elapsed() < Duration::from_secs(10));
    wait_until("the timed-out program is gone", || {
        run(
            &pinned,
            "pgrep -f '[s]leep 45' >/dev/null && echo alive || echo gone",
        )
        .stdout
        .0
        .starts_with(b"gone")
    });

    // SFTP moves more than one packet, and the bytes come back as they went.
    let big: Vec<u8> = (0..3 * 1024 * 1024u32).map(|n| (n % 251) as u8).collect();
    let path = format!("{scratch}/big.bin");
    pinned
        .write_file(&WriteSpec {
            path: path.clone(),
            data: Bytes(big.clone()),
            append: false,
            create_dirs: false,
        })
        .unwrap();
    let remote_sum = run(&pinned, &format!("sha256sum {path} | cut -d' ' -f1"));
    assert_eq!(text(&remote_sum.stdout).trim().len(), 64);
    let mut back = Vec::new();
    loop {
        let chunk = pinned
            .read_file(&ReadSpec {
                path: path.clone(),
                offset: back.len() as u64,
                limit: 1024 * 1024,
            })
            .unwrap();
        assert_eq!(chunk.size, big.len() as u64);
        if chunk.data.0.is_empty() {
            break;
        }
        back.extend_from_slice(&chunk.data.0);
    }
    assert!(back == big, "3 MiB did not survive the round trip");
    pinned
        .write_file(&WriteSpec {
            path: format!("{scratch}/deep/er/file.txt"),
            data: Bytes(b"x".to_vec()),
            append: false,
            create_dirs: true,
        })
        .unwrap();
    assert_eq!(
        pinned
            .list_dir(&format!("{scratch}/deep/er"))
            .unwrap()
            .entries[0]
            .name,
        "file.txt"
    );

    // A reverse forward: the host reaches a service on the Station through the tunnel.
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = service.local_addr().unwrap().to_string();
    let echo = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream, _) = service.accept().unwrap();
        let mut buffer = [0u8; 16];
        let count = stream.read(&mut buffer).unwrap();
        stream.write_all(&buffer[..count]).unwrap();
    });
    let forward = pinned
        .forward(&ForwardSpec {
            remote_host: "127.0.0.1".to_owned(),
            remote_port: 0,
            target,
        })
        .unwrap();
    let port = forward.info().remote_port;
    let out = run(
        &pinned,
        // `/dev/tcp` is a bash feature; the remote login shell may be dash.
        &format!("bash -c 'exec 3<>/dev/tcp/127.0.0.1/{port}; printf tunnel >&3; head -c 6 <&3'"),
    );
    assert_eq!(text(&out.stdout), "tunnel", "{}", text(&out.stderr));
    echo.join().unwrap();
    forward.close().unwrap();
    let out = run(
        &pinned,
        &format!(
            "bash -c '(exec 3<>/dev/tcp/127.0.0.1/{port}) 2>/dev/null' && echo open || echo closed"
        ),
    );
    assert_eq!(text(&out.stdout), "closed\n");
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}
