//! Loads the built provider library through the Station's plugin host, so the models cross
//! the real Guatiao boundary, and runs the contract suite against what comes back.

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::station::plugins::Plugins;

/// The cdylib is built into the directory that holds this test binary.
fn plugins() -> Plugins {
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    Plugins::scan(&[deps])
}

#[test]
fn the_built_library_loads_as_a_transport_and_passes_the_contract_suite() {
    let plugins = plugins();
    assert!(
        plugins
            .transports()
            .iter()
            .any(|provider| provider.id == "local" && provider.unavailable.is_none()),
        "{:?}",
        plugins.scan_summary()
    );
    let transport = plugins.transport("local", None).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let (shell, expand): (&[&str], fn(&str) -> String) = if cfg!(windows) {
        (&["cmd", "/C"], |name| format!("%{name}%"))
    } else {
        (&["sh", "-c"], |name| format!("${name}"))
    };
    contract::check(
        transport,
        &Host {
            shell,
            expand,
            scratch: &scratch.path().to_string_lossy(),
        },
    );
}
