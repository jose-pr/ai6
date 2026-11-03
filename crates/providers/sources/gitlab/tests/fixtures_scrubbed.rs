//! The recorded fixtures must hold nothing that points back at the account that made them.

use std::path::Path;

const LIVE_SECRETS: [&str; 2] = ["AI6_TEST_LIVE_GITLAB_TOKEN", "AI6_TEST_LIVE_GITLAB_PROJECT"];
/// The user IDs fixtures keep: people stand in as 1, 2 and 3.
const FIXTURE_USER_IDS: [u64; 3] = [1, 2, 3];
/// The project's group stands in as 100, the project as 1000.
const FIXTURE_NAMESPACE_IDS: [u64; 2] = [100, 1000];

/// What must not be in a fixture. The site is assembled here so this file does not name it.
fn offenses(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for needle in [
        "glpat-",
        "gloas-",
        "gldt-",
        "glrt-",
        "glptt-",
        "glsoat-",
        "glft-",
        "glimt-",
        "glagent-",
        "PRIVATE-TOKEN",
        "Bearer ",
        "Basic ",
        "gravatar.com",
    ] {
        if text.contains(needle) {
            found.push(needle.to_owned());
        }
    }
    let site = ["boriken", "-dev"].concat();
    if text.to_lowercase().contains(&site) {
        found.push(site);
    }
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || "@.-_+".contains(c))) {
        let address = word
            .split_once('@')
            .is_some_and(|(user, host)| !user.is_empty() && host.contains('.'));
        if address && !word.ends_with("@example.invalid") {
            found.push(word.to_owned());
        }
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        people(&value, &mut found);
    }
    // A value given to a live run must not have reached a fixture.
    for name in LIVE_SECRETS {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
            && text.contains(&value)
        {
            found.push(name.to_owned());
        }
    }
    found
}

/// A person's or a group's numeric ID is an account: only the fixtures' own stand-ins may appear.
fn people(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Array(items) => items.iter().for_each(|item| people(item, found)),
        serde_json::Value::Object(map) => {
            let id = map.get("id").and_then(serde_json::Value::as_u64);
            if map.contains_key("username")
                && let Some(id) = id
                && !FIXTURE_USER_IDS.contains(&id)
            {
                found.push(format!("user id {id}"));
            }
            if map.contains_key("full_path")
                && let Some(id) = id
                && !FIXTURE_NAMESPACE_IDS.contains(&id)
            {
                found.push(format!("namespace id {id}"));
            }
            map.values().for_each(|item| people(item, found));
        }
        _ => {}
    }
}

#[test]
fn no_fixture_holds_a_token_an_address_an_account_or_the_site() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut scanned = 0;
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(offenses(&text), Vec::<String>::new(), "{}", path.display());
        scanned += 1;
    }
    assert!(scanned > 0, "no fixtures in {}", directory.display());
}

#[test]
fn the_scan_finds_each_kind_of_offense() {
    let planted = [
        format!("\"{}\"", ["someone", "@", "corp", ".test"].concat()),
        format!("\"https://gitlab.com/{}/r\"", ["boriken", "-dev"].concat()),
        format!("\"{}\"", ["gl", "pat-", "abc123"].concat()),
        "{\"author\": {\"id\": 1234567, \"username\": \"someone\"}}".to_owned(),
        "{\"namespace\": {\"id\": 7654321, \"full_path\": \"o\"}}".to_owned(),
        "\"https://secure.gravatar.com/avatar/1\"".to_owned(),
    ];
    for offense in planted {
        assert!(
            !offenses(&format!("{{\"body\": {offense}}}")).is_empty(),
            "{offense}"
        );
    }
    assert_eq!(
        offenses(
            "{\"author\": {\"id\": 1, \"username\": \"reporter\", \"public_email\": \"reporter@example.invalid\"}}"
        ),
        Vec::<String>::new()
    );
}
