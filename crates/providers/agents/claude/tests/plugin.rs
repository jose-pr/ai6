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
fn the_built_library_is_built_with_its_default_settings() {
    let plugins = plugins();
    assert!(
        plugins
            .handlers()
            .iter()
            .any(|provider| provider.id == "claude" && provider.unavailable.is_none()),
        "{:?}",
        plugins.scan_summary()
    );
    plugins
        .handler("claude", None)
        .expect("every Claude handler setting has a default");
}

/// Whether the library builds  with the Station record's `[providers.claude]` table `config`.
fn builds_with(config: serde_json::Value) -> bool {
    let configs = std::collections::BTreeMap::from([("claude".to_owned(), config)]);
    plugins()
        .with_provider_configs(ai6::station::plugins::provider_configs(&configs).unwrap())
        .handler("claude", None)
        .is_ok()
}

#[test]
fn the_built_library_reads_its_limits_from_its_configuration() {
    assert!(builds_with(
        serde_json::json!({ "tail_lines": 3, "exit_wait_ms": 0, "csi_bytes": 128 })
    ));
    assert!(
        !builds_with(serde_json::json!({ "tail_lines": "many" })),
        "a limit that is not a number is refused"
    );
    assert!(
        !builds_with(serde_json::json!({ "no_such_limit": 1 })),
        "an unknown key is refused"
    );
}
