//! Loads the built provider library through the Station's plugin host, so the models cross
//! the real Guatiao boundary, and checks that the host validates its settings.

use ai6::guatiao::ToValue;
use ai6::station::plugins::{PluginBuildError, Plugins};
use ai6::{TheaterMaker, TheaterProviderConfig};

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
    TheaterProviderConfig {
        settings_toml: settings.to_owned(),
    }
    .to_value()
    .unwrap()
}

#[test]
fn the_built_library_loads_as_a_theater_provider_and_declares_what_its_theaters_do() {
    let plugins = plugins();
    assert!(
        plugins
            .theater_providers()
            .iter()
            .any(|provider| provider.id == "ephemeral-container" && provider.unavailable.is_none()),
        "{:?}",
        plugins.scan_summary()
    );
    let maker = TheaterMaker::new(
        plugins
            .theater_provider(
                "ephemeral-container",
                Some(&config("image = \"fedora:44\"\nengine = \"podman\"\n")),
            )
            .unwrap(),
    );
    let declared = maker.capabilities().unwrap();
    assert_eq!(
        (
            declared.terminal,
            declared.run,
            declared.files,
            declared.forward,
            declared.os.as_deref()
        ),
        (false, true, true, false, Some("linux"))
    );
}

#[test]
fn a_misspelled_or_missing_setting_is_refused_when_the_provider_is_built() {
    for settings in ["engine = \"podman\"\n", "image = \"i\"\nimag = \"x\"\n"] {
        let error = plugins()
            .theater_provider("ephemeral-container", Some(&config(settings)))
            .err()
            .unwrap();
        assert!(
            matches!(error, PluginBuildError::InvalidConfiguration { .. }),
            "{error}"
        );
    }
}
