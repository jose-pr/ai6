//! Loads the built provider library through the Station's plugin host, so the models cross
//! the real Guatiao boundary, and runs one poll against a loopback server.

use std::sync::Once;

use ai6::guatiao::ToValue;
use ai6::station::plugins::{PluginBuildError, Plugins};
use ai6::{SourceConfig, SourcePoll};

mod support;
use support::{canned, serve};

static INIT: Once = Once::new();

fn config(settings: &str) -> ai6::guatiao::Value {
    INIT.call_once(|| {
        // SAFETY: runs once, before this test binary reads the environment.
        unsafe { std::env::set_var("AI6_TEST_JIRA_PLUGIN_TOKEN", "plugin-token") };
    });
    SourceConfig {
        settings_toml: settings.to_owned(),
    }
    .to_value()
    .unwrap()
}

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
fn the_built_library_loads_as_a_source_and_serves_a_poll() {
    let plugins = plugins();
    for providers in [plugins.sources(), plugins.trackers()] {
        assert!(
            providers.iter().any(|provider| provider.id == "jira"),
            "{:?}",
            plugins.scan_summary()
        );
    }
    let fake = serve(|_| vec![canned(200, include_str!("fixtures/cloud_page2.json"))]);
    let source = plugins
        .source(
            "jira",
            Some(&config(&format!(
                "site = \"{}\"\nproject = \"GUA\"\nauth = \"bearer\"\n\
                 token_env = \"AI6_TEST_JIRA_PLUGIN_TOKEN\"",
                fake.base
            ))),
        )
        .unwrap();
    let batch = source.poll(SourcePoll { cursor: None }).unwrap();
    assert_eq!(batch.items[0].id, "GUA-14");
}

#[test]
fn the_host_refuses_a_misspelled_setting_before_the_provider_runs() {
    let error = plugins()
        .source(
            "jira",
            Some(&config(
                "site = \"https://x\"\nproject = \"G\"\nlabel = \"x\"",
            )),
        )
        .err()
        .unwrap();
    assert!(
        matches!(error, PluginBuildError::InvalidConfiguration { .. }),
        "{error}"
    );
}
