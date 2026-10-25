//! The `adb` transport against a real Android device or emulator.
//!
//! It runs only when `AI6_TEST_ADB_SERIAL` names the device (and `AI6_TEST_ADB` the adb
//! executable, if it is not `adb`): the test never picks a device on its own, because a
//! machine with a developer's phone attached must not be touched by a test run.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::guatiao::Bytes;
use ai6::{ForwardSpec, ReadSpec, RunSpec, TransportClient, TransportConfig, WriteSpec};
use ai6_transport_adb::Adb;

fn device() -> Option<Adb> {
    let Ok(serial) = std::env::var("AI6_TEST_ADB_SERIAL") else {
        eprintln!("SKIPPED: AI6_TEST_ADB_SERIAL does not name a device");
        return None;
    };
    let executable = std::env::var("AI6_TEST_ADB").unwrap_or_else(|_| "adb".to_owned());
    let settings = format!("serial = {serial:?}\nexecutable = {executable:?}\n");
    Some(
        Adb::try_from(TransportConfig {
            settings_toml: settings,
        })
        .unwrap(),
    )
}

fn run(client: &TransportClient, script: &str) -> ai6::Output {
    client
        .run(&RunSpec {
            argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            ..RunSpec::default()
        })
        .unwrap()
}

#[test]
fn the_adb_transport_works_on_a_real_android_device() {
    let Some(adb) = device() else { return };
    let client = TransportClient::new(Box::new(adb));
    let probed = client.probe().expect("the device answers");
    assert_eq!(probed.os.as_deref(), Some("android"));
    assert!(probed.arch.is_some());

    let scratch = format!("/data/local/tmp/ai6-adb-{}", std::process::id());
    assert_eq!(
        run(&client, &format!("mkdir -p {scratch}/contract")).code,
        Some(0)
    );

    // The contract every transport passes, on the device's own shell.
    let transport = Adb::try_from(TransportConfig {
        settings_toml: format!(
            "serial = {:?}\nexecutable = {:?}\n",
            std::env::var("AI6_TEST_ADB_SERIAL").unwrap(),
            std::env::var("AI6_TEST_ADB").unwrap_or_else(|_| "adb".to_owned())
        ),
    })
    .unwrap();
    contract::check(
        Box::new(transport),
        &Host {
            shell: &["sh", "-c"],
            expand: |name| format!("${name}"),
            scratch: &format!("{scratch}/contract"),
        },
    );

    // Arguments reach the program as written, however the device shell would read them.
    let output = client
        .run(&RunSpec {
            argv: ["printf", "%s|", "a b", "it's", "$HOME", "*"]
                .map(str::to_owned)
                .to_vec(),
            cwd: Some(scratch.clone()),
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout.0),
        "a b|it's|$HOME|*|"
    );
    let output = run(&client, "echo oops 1>&2; exit 9");
    assert_eq!(output.code, Some(9));
    assert_eq!(String::from_utf8_lossy(&output.stderr.0).trim(), "oops");

    // Bytes survive both ways, 300 KiB of them.
    let big: Vec<u8> = (0..300 * 1024u32).map(|n| (n % 251) as u8).collect();
    let path = format!("{scratch}/big.bin");
    client
        .write_file(&WriteSpec {
            path: path.clone(),
            data: Bytes(big.clone()),
            append: false,
            create_dirs: false,
        })
        .unwrap();
    let read = client
        .read_file(&ReadSpec {
            path,
            offset: 0,
            limit: 1024 * 1024,
        })
        .unwrap();
    assert_eq!(read.size, big.len() as u64);
    assert!(read.data.0 == big, "300 KiB did not survive the round trip");

    // A reverse forward: the device reaches a service on the Station's loopback.
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = service.local_addr().unwrap().to_string();
    let echo = std::thread::spawn(move || {
        let (mut stream, _) = service.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut buffer = [0u8; 16];
        let count = stream.read(&mut buffer).unwrap();
        stream.write_all(&buffer[..count]).unwrap();
    });
    let forward = client
        .forward(&ForwardSpec {
            remote_host: "127.0.0.1".to_owned(),
            remote_port: 0,
            target,
        })
        .unwrap();
    let port = forward.info().remote_port;
    let out = run(
        &client,
        &format!("(echo tunnel; sleep 2) | nc -w 3 127.0.0.1 {port}"),
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout.0).trim(),
        "tunnel",
        "{}",
        String::from_utf8_lossy(&out.stderr.0)
    );
    echo.join().unwrap();
    forward.close().unwrap();

    run(&client, &format!("rm -r -f {scratch}"));
}
