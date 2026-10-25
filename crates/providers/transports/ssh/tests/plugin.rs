//! Loads the built provider library through the Station's plugin host, so the models cross
//! the real Guatiao boundary, and runs the contract suite against what comes back.

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::station::plugins::{PluginBuildError, Plugins};

mod support;
use support::{FakeSsh, shell_available};

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
    if !shell_available() {
        eprintln!("SKIPPED: no `sh` on PATH for the in-process SSH server to run commands");
        return;
    }
    let plugins = plugins();
    assert!(
        plugins
            .transports()
            .iter()
            .any(|provider| provider.id == "ssh" && provider.unavailable.is_none()),
        "{:?}",
        plugins.scan_summary()
    );
    let server = FakeSsh::start();
    let transport = plugins
        .transport("ssh", Some(&server.config(server.settings(""))))
        .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    contract::check(
        transport,
        &Host {
            shell: &["sh", "-c"],
            expand: |name| format!("${name}"),
            scratch: &scratch.path().to_string_lossy(),
        },
    );
}

#[test]
fn a_misspelled_setting_is_refused_when_the_transport_is_built() {
    let server = FakeSsh::start();
    let config = server.config(server.settings("trust_on_first_use_ = true\n"));
    let error = plugins().transport("ssh", Some(&config)).err().unwrap();
    assert!(
        matches!(error, PluginBuildError::InvalidConfiguration { .. }),
        "{error}"
    );
    assert!(
        matches!(
            plugins().transport("ssh", None).err().unwrap(),
            PluginBuildError::ConfigurationRequired { .. }
        ),
        "the ssh transport has no usable default"
    );
}
