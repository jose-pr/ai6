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

/// The Agent's `handler_settings` as the roster's table.
fn agent_settings(toml_text: &str) -> serde_json::Map<String, serde_json::Value> {
    toml::from_str(toml_text).unwrap()
}

#[test]
fn the_built_library_is_built_with_its_default_settings() {
    let plugins = plugins();
    assert!(
        plugins
            .handlers()
            .iter()
            .any(|provider| provider.id == "antigravity" && provider.unavailable.is_none()),
        "{:?}",
        plugins.scan_summary()
    );
    plugins
        .handler("antigravity", None)
        .expect("every setting has a default");
}

#[test]
fn an_agents_settings_reach_the_built_library_and_its_weakenings_come_back() {
    let plugins = plugins();
    let built = plugins
        .handler_with(
            "antigravity",
            &agent_settings("allowed_tools = [\"search_web\"]\n"),
        )
        .expect("the handler reads its settings");
    assert_eq!(
        built.weakenings().unwrap(),
        ["allowed_tools adds search_web to what its field Runs may call"]
    );
    assert!(
        plugins
            .handler_with("antigravity", &agent_settings("no_such_key = 1\n"))
            .is_err(),
        "an unknown key is refused"
    );
}

/// Whether the library builds with the Station record's `[providers.antigravity]` table.
fn builds_with(config: serde_json::Value) -> bool {
    let configs = std::collections::BTreeMap::from([("antigravity".to_owned(), config)]);
    plugins()
        .with_provider_configs(ai6::station::plugins::provider_configs(&configs).unwrap())
        .handler("antigravity", None)
        .is_ok()
}

#[test]
fn the_built_library_reads_its_limits_from_its_configuration() {
    assert!(builds_with(
        serde_json::json!({ "tail_lines": 3, "gate_timeout_secs": 5, "mcp_timeout_secs": -1 })
    ));
    assert!(!builds_with(serde_json::json!({ "tail_lines": "many" })));
    assert!(!builds_with(serde_json::json!({ "no_such_limit": 1 })));
}

/// An Agent's `handler_settings` may not choose the program a Station runs; the station record's
/// `[providers.antigravity]` may.
#[test]
fn an_agent_cannot_set_the_executable_but_the_station_record_can() {
    let plugins = plugins();
    let settings = serde_json::json!({"executable": "elsewhere"});
    let error = plugins
        .handler_with("antigravity", settings.as_object().unwrap())
        .err()
        .expect("an Agent's executable is refused")
        .to_string();
    assert!(
        error.contains("\"antigravity\"")
            && error.contains("\"executable\"")
            && error.contains("Station setting"),
        "{error}"
    );
    let station = ai6::provider_settings::settings_value(settings.as_object().unwrap()).unwrap();
    plugins
        .handler("antigravity", Some(&station))
        .expect("the station record may set the executable");
}
