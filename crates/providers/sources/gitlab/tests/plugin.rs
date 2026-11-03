//! Loads the built provider library through the Station's plugin host, so the models cross
//! the real Guatiao boundary, and runs one poll and one post against a loopback server.

use std::sync::Once;

use ai6::guatiao::ToValue;
use ai6::station::plugins::{PluginBuildError, Plugins};
use ai6::{SourceConfig, SourcePoll, TrackerPost};

mod support;
use support::{replay, serve};

const TOKEN_VAR: &str = "AI6_TEST_GITLAB_PLUGIN_TOKEN";
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
fn the_built_library_loads_as_a_source_and_a_tracker_and_serves_a_poll_and_a_post() {
    let plugins = plugins();
    for providers in [plugins.sources(), plugins.trackers()] {
        assert!(
            providers.iter().any(|provider| provider.id == "gitlab"),
            "{:?}",
            plugins.scan_summary()
        );
    }

    let fake = serve(|_| {
        vec![
            replay(include_str!("fixtures/issues_page2.json"), &[]),
            replay(include_str!("fixtures/note_created.json"), &[]),
        ]
    });
    let settings = format!(
        "project = \"o/r\"\ntoken_env = \"{TOKEN_VAR}\"\napi_url = \"{}/api/v4\"",
        fake.base
    );
    let source = plugins.source("gitlab", Some(&config(&settings))).unwrap();

    let batch = source.poll(SourcePoll { cursor: None }).unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].id, "o/r#2");
    assert_eq!(batch.items[0].code_tag.as_deref(), Some("gl"));
    assert_eq!(batch.cursor.as_deref(), Some("2026-11-03T15:25:13.002Z"));

    let tracker = plugins.tracker("gitlab", Some(&config(&settings))).unwrap();
    let posted = tracker
        .post(TrackerPost {
            item: "o/r#2".to_owned(),
            body: "The requested work is finished.".to_owned(),
            edit: None,
            marker: None,
        })
        .unwrap();
    assert_eq!(posted.id, "4151757723");
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests[1].target, "/api/v4/projects/o%2Fr/issues/2/notes");
}

#[test]
fn the_host_refuses_a_misspelled_setting_before_the_provider_runs() {
    let error = plugins()
        .source("gitlab", Some(&config("project = \"o/r\"\nlabel = \"x\"")))
        .err()
        .unwrap();
    assert!(
        matches!(error, PluginBuildError::InvalidConfiguration { .. }),
        "{error}"
    );
}
