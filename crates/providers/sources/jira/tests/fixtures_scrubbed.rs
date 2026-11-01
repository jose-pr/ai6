//! The recorded fixtures must hold nothing that points back at the account that made them.

use std::path::Path;

const LIVE_SECRETS: [&str; 2] = ["AI6_TEST_LIVE_JIRA_TOKEN", "AI6_TEST_LIVE_JIRA_EMAIL"];
const SCRUBBED_ACCOUNT_ID: &str = "000000:00000000-0000-0000-0000-000000000000";

/// What must not be in a fixture. The site is assembled here so this file does not name it.
fn offenses(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for needle in [
        "github_pat_",
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "ATATT",
        "Bearer ",
        "Basic ",
        "gravatar.com",
        "avatars.githubusercontent",
        "avatar-management",
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
    for token in text.split(['"', '=', '&']) {
        let account_id = token.len() == SCRUBBED_ACCOUNT_ID.len()
            && token.as_bytes().get(6) == Some(&b':')
            && token
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'-');
        if account_id && token != SCRUBBED_ACCOUNT_ID {
            found.push(token.to_owned());
        }
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

#[test]
fn no_fixture_holds_a_token_an_address_or_the_site() {
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
        ["someone", "@", "corp", ".test"].concat(),
        format!("https://{}.atlassian.net", ["boriken", "-dev"].concat()),
        ["gh", "p_", "abc123"].concat(),
        ["github", "_pat_", "abc123"].concat(),
        ["ATA", "TT3xFfGF0"].concat(),
        ["712020", ":", "12daf318-0000-4000-8000-000000000000"].concat(),
    ];
    for offense in planted {
        assert!(
            !offenses(&format!("{{\"body\": \"{offense}\"}}")).is_empty(),
            "{offense}"
        );
    }
    assert_eq!(
        offenses(&format!(
            "{{\"emailAddress\": \"reporter@example.invalid\", \"accountId\": \"{SCRUBBED_ACCOUNT_ID}\"}}"
        )),
        Vec::<String>::new()
    );
}
