//! The `container` transport against a real Docker or Podman engine.
//!
//! It runs only when `AI6_TEST_CONTAINER` names a running container (and `AI6_TEST_ENGINE`
//! the engine, if it is not `docker`): the test never picks a container on its own.

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::guatiao::Bytes;
use ai6::{ReadSpec, RunSpec, TransportClient, TransportConfig, WriteSpec};
use ai6_transport_container::Container;

fn container() -> Option<Container> {
    let Ok(name) = std::env::var("AI6_TEST_CONTAINER") else {
        eprintln!("SKIPPED: AI6_TEST_CONTAINER does not name a running container");
        return None;
    };
    let engine = std::env::var("AI6_TEST_ENGINE").unwrap_or_else(|_| "docker".to_owned());
    Some(
        Container::try_from(TransportConfig {
            settings_toml: format!("container = {name:?}\nengine = {engine:?}\n"),
        })
        .unwrap(),
    )
}

#[test]
fn the_container_transport_works_on_a_real_engine() {
    let Some(first) = container() else { return };
    let client = TransportClient::new(Box::new(first));
    let probed = client.probe().expect("the container is running");
    assert!(probed.os.is_some() && probed.arch.is_some() && probed.files);

    let scratch = format!("/tmp/ai6-container-{}", std::process::id());
    let make = client
        .run(&RunSpec {
            argv: ["mkdir", "-p", &format!("{scratch}/contract")]
                .map(str::to_owned)
                .to_vec(),
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(make.code, Some(0));

    contract::check(
        Box::new(container().unwrap()),
        &Host {
            shell: &["sh", "-c"],
            expand: |name| format!("${name}"),
            scratch: &format!("{scratch}/contract"),
        },
    );

    // Arguments reach the program as written; the engine takes no shell in between.
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

    // Bytes survive both ways.
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

    let _ = client.run(&RunSpec {
        argv: ["rm", "-r", "-f", &scratch].map(str::to_owned).to_vec(),
        ..RunSpec::default()
    });
}
