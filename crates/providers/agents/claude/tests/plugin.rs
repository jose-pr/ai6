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

/// The Agent's `handler_settings` as the roster's table.
fn agent_settings(toml_text: &str) -> serde_json::Map<String, serde_json::Value> {
    toml::from_str(toml_text).unwrap()
}

#[test]
fn an_agents_settings_reach_the_built_library_and_its_weakenings_come_back() {
    let plugins = plugins();
    let built = plugins
        .handler_with(
            "claude",
            &agent_settings("allowed_tools = [\"WebFetch\"]\ndenied_tools = [\"Bash\"]\n"),
        )
        .expect("the handler reads its settings");
    assert_eq!(
        built.weakenings().unwrap(),
        ["allowed_tools adds WebFetch to what its field Runs may call"]
    );
    assert!(
        plugins
            .handler_with("claude", &agent_settings("no_such_key = 1\n"))
            .is_err(),
        "an unknown key is refused"
    );
    assert!(
        plugins
            .handler("claude", None)
            .unwrap()
            .weakenings()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_agents_settings_go_beside_the_stations_table_for_the_handler() {
    let configs = std::collections::BTreeMap::from([(
        "claude".to_owned(),
        serde_json::json!({ "tail_lines": "many" }),
    )]);
    let refused = plugins()
        .with_provider_configs(ai6::station::plugins::provider_configs(&configs).unwrap())
        .handler_with("claude", &agent_settings("denied_tools = [\"Bash\"]\n"));
    assert!(
        refused.is_err(),
        "the table is still read when an Agent has settings"
    );
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
/// `[providers.claude]` may.
#[test]
fn an_agent_cannot_set_the_executable_but_the_station_record_can() {
    let plugins = plugins();
    let settings = serde_json::json!({"executable": "elsewhere"});
    let error = plugins
        .handler_with("claude", settings.as_object().unwrap())
        .err()
        .expect("an Agent's executable is refused")
        .to_string();
    assert!(
        error.contains("\"claude\"")
            && error.contains("\"executable\"")
            && error.contains("Station setting"),
        "{error}"
    );
    let station = ai6::provider_settings::settings_value(settings.as_object().unwrap()).unwrap();
    plugins
        .handler("claude", Some(&station))
        .expect("the station record may set the executable");
    let tools = serde_json::json!({"allowed_tools": ["WebFetch"], "denied_tools": ["Bash(rm *)"]});
    plugins
        .handler_with("claude", tools.as_object().unwrap())
        .expect("an Agent may set the tool lists");
}
