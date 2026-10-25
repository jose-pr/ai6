//! Loads the built provider library through the Station's plugin host, so the models cross
//! the real Guatiao boundary, and runs one poll and one report against a loopback server.

use std::sync::Once;

use ai6::guatiao::ToValue;
use ai6::station::plugins::{PluginBuildError, Plugins};
use ai6::{CxReport, SourceConfig, SourcePoll};

mod support;
use support::{canned, serve};

const TOKEN_VAR: &str = "AI6_TEST_GITHUB_PLUGIN_TOKEN";
static INIT: Once = Once::new();

fn config(settings: &str) -> ai6::guatiao::Value {
    INIT.call_once(|| {
        // SAFETY: runs once, before this test binary reads the environment.
        unsafe { std::env::set_var(TOKEN_VAR, "plugin-token") };
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
fn the_built_library_loads_as_a_source_and_serves_a_poll_and_a_report() {
    let plugins = plugins();
    assert!(
        plugins
            .sources()
            .iter()
            .any(|provider| provider.id == "github"),
        "{:?}",
        plugins.scan_summary()
    );

    let fake = serve(|_| {
        vec![
            canned(200, include_str!("fixtures/issues_page2.json")),
            canned(201, "{}"),
        ]
    });
    let source = plugins
        .source(
            "github",
            Some(&config(&format!(
                "repo = \"o/r\"\ntoken_env = \"{TOKEN_VAR}\"\napi_url = \"{}\"",
                fake.base
            ))),
        )
        .unwrap();

    let batch = source.poll(SourcePoll { cursor: None }).unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].id, "o/r#14");
    assert_eq!(batch.cursor.as_deref(), Some("2026-10-04T11:15:00Z"));

    source
        .report(CxReport {
            item_id: "o/r#14".to_owned(),
            mission_id: "m".to_owned(),
            outcome: "done".to_owned(),
            body: "The requested work is finished.".to_owned(),
            branch: None,
        })
        .unwrap();
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests[1].target, "/repos/o/r/issues/14/comments");
}

#[test]
fn the_host_refuses_a_misspelled_setting_before_the_provider_runs() {
    let error = plugins()
        .source("github", Some(&config("repo = \"o/r\"\nlabel = \"x\"")))
        .err()
        .unwrap();
    assert!(
        matches!(error, PluginBuildError::InvalidConfiguration { .. }),
        "{error}"
    );
}
