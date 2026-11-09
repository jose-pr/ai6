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
            .any(|provider| provider.id == "codex" && provider.unavailable.is_none()),
        "{:?}",
        plugins.scan_summary()
    );
    plugins
        .handler("codex", None)
        .expect("every Codex handler setting has a default");
}

/// Whether the library builds  with the Station record's `[providers.codex]` table `config`.
fn builds_with(config: serde_json::Value) -> bool {
    let configs = std::collections::BTreeMap::from([("codex".to_owned(), config)]);
    plugins()
        .with_provider_configs(ai6::station::plugins::provider_configs(&configs).unwrap())
        .handler("codex", None)
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

/// An Agent's `handler_settings` may not choose the program a Station runs; the station record's
/// `[providers.codex]` may.
#[test]
fn an_agent_cannot_set_the_executable_but_the_station_record_can() {
    let plugins = plugins();
    let settings = serde_json::json!({"executable": "elsewhere"});
    let error = plugins
        .handler_with("codex", settings.as_object().unwrap())
        .err()
        .expect("an Agent's executable is refused")
        .to_string();
    assert!(
        error.contains("\"codex\"")
            && error.contains("\"executable\"")
            && error.contains("Station setting"),
        "{error}"
    );
    let station = ai6::provider_settings::settings_value(settings.as_object().unwrap()).unwrap();
    plugins
        .handler("codex", Some(&station))
        .expect("the station record may set the executable");
}
