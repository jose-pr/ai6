//! Loads the built handler library through the Station's plugin host.

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
fn the_built_library_loads_and_builds_without_settings() {
    let plugins = plugins();
    assert!(
        plugins
            .handlers()
            .iter()
            .any(|provider| provider.id == "acp" && provider.unavailable.is_none()),
        "{:?}",
        plugins.scan_summary()
    );
    plugins
        .handler("acp", None)
        .expect("the acp handler builds without settings");
}
