//! The `ephemeral-container` provider against a real Docker or Podman engine: a container is
//! made, fitted out by Q over the `container` transport, used, listed and removed.
//!
//! It runs only when `AI6_TEST_THEATER_IMAGE` names an image already present (and
//! `AI6_TEST_ENGINE` the engine, if it is not `docker`): the test never pulls an image.

use std::process::Command;

use ai6::directorates::q::provision::{self, ProvisionSpec, Requirements, StepStatus};
use ai6::station::plugins::Plugins;
use ai6::{
    RunSpec, Theater, TheaterAccess, TheaterMaker, TheaterProviderConfig, TheaterRequest,
    TransportAccess,
};
use ai6_theater_container::{ContainerTheaters, container_name};

#[test]
fn a_container_theater_is_made_fitted_out_used_and_gone_afterwards() {
    let Ok(image) = std::env::var("AI6_TEST_THEATER_IMAGE") else {
        eprintln!("SKIPPED: AI6_TEST_THEATER_IMAGE does not name an image");
        return;
    };
    let engine = std::env::var("AI6_TEST_ENGINE").unwrap_or_else(|_| "docker".to_owned());
    let maker = TheaterMaker::new(Box::new(
        ContainerTheaters::try_from(TheaterProviderConfig {
            settings_toml: format!(
                "image = {image:?}\nengine = {engine:?}\nrun_args = [\"--network\", \"none\"]\n"
            ),
        })
        .unwrap(),
    ));
    let template = format!("ai6-test-{}", std::process::id());
    let name = format!("{template}-m1");
    let made = maker
        .create(&TheaterRequest {
            name: name.clone(),
            template: template.clone(),
            mission: "m1".to_owned(),
        })
        .expect("the container is made");
    let listed = maker.list(&template).unwrap();
    assert_eq!(listed, std::slice::from_ref(&made));

    // Q fits it out over the transport the provider named, loaded as a Station loads it
    // (the `container` transport library is built beside this test as a dev-dependency).
    let mut access = TransportAccess::new(made.transport.clone());
    access.config = match toml::from_str::<toml::Table>(&made.settings_toml) {
        Ok(table) => table
            .into_iter()
            .map(|(key, value)| {
                (
                    key,
                    serde_json::Value::String(value.as_str().unwrap().to_owned()),
                )
            })
            .collect(),
        Err(error) => panic!("{error}"),
    };
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let client = Plugins::scan(&[deps])
        .theater_transport(&TheaterAccess::Transport(access.clone()))
        .expect("the container transport library is beside the test");
    let theater = Theater::new(name.clone(), TheaterAccess::Transport(access))
        .with_requirements(Requirements::default().with_tool("ai6-stand-in".parse().unwrap()))
        .with_provision(ProvisionSpec::default().with_install(
            "ai6-stand-in",
            "printf '#!/bin/sh\\necho ai6-stand-in 1.0\\n' > /usr/local/bin/ai6-stand-in \
             && chmod 755 /usr/local/bin/ai6-stand-in",
        ));
    let steps = provision::provision(&client, &theater, false);
    assert_eq!(steps[0].status, StepStatus::Changed, "{steps:#?}");
    let again = provision::provision(&client, &theater, false);
    assert_eq!(again[0].status, StepStatus::Ok, "{again:#?}");
    let ran = client
        .run(&RunSpec {
            argv: vec!["ai6-stand-in".to_owned()],
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout.0).trim(),
        "ai6-stand-in 1.0"
    );

    maker.destroy(&name).unwrap();
    maker
        .destroy(&name)
        .expect("destroying it again is not an error");
    assert!(maker.list(&template).unwrap().is_empty());
    let gone = Command::new(&engine)
        .args(["container", "inspect", &container_name(&name)])
        .output()
        .unwrap();
    assert!(!gone.status.success(), "the container is gone");
}
