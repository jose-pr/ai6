//! Loads the built provider library through the Station's plugin host, so the models cross
//! the real Guatiao boundary, and checks that the host validates its settings.

use ai6::guatiao::ToValue;
use ai6::station::plugins::{PluginBuildError, Plugins};
use ai6::{Capabilities, TransportConfig};

/// The cdylib is built into the directory that holds this test binary.
fn plugins() -> Plugins {
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    Plugins::scan(&[deps])
}

fn config(settings: &str) -> ai6::guatiao::Value {
    TransportConfig {
        settings_toml: settings.to_owned(),
    }
    .to_value()
    .unwrap()
}

#[test]
fn the_built_library_loads_as_a_transport_and_declares_what_it_does() {
    let plugins = plugins();
    assert!(
        plugins
            .transports()
            .iter()
            .any(|provider| provider.id == "qemu" && provider.unavailable.is_none()),
        "{:?}",
        plugins.scan_summary()
    );
    let transport = plugins
        .transport("qemu", Some(&config("address = \"127.0.0.1:4555\"\n")))
        .unwrap();
    let declared = ai6::guatiao::FromValue::from_value(&transport.capabilities().unwrap());
    let declared: Capabilities = declared.unwrap();
    assert_eq!(
        (
            declared.terminal,
            declared.run,
            declared.files,
            declared.forward
        ),
        (false, true, true, false)
    );
}

#[test]
fn a_misspelled_or_missing_setting_is_refused_when_the_transport_is_built() {
    let error = plugins()
        .transport(
            "qemu",
            Some(&config("address = \"127.0.0.1:4555\"\nadress = 1\n")),
        )
        .err()
        .unwrap();
    assert!(
        matches!(error, PluginBuildError::InvalidConfiguration { .. }),
        "{error}"
    );
}
