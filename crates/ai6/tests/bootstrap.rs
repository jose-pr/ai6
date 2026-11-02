//! `ai6.cfg`: format by extension and first line, search order, unknown keys and weakenings.

use std::path::{Path, PathBuf};
use std::process::Command;

use ai6::bootstrap::{Bootstrap, SearchPath, SecretRef, read_ca_file};

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn read(text: &str) -> Result<Bootstrap, String> {
    let dir = tempfile::tempdir().unwrap();
    Bootstrap::read(&write(dir.path(), "ai6.cfg", text)).map_err(|error| error.to_string())
}

#[test]
fn a_cfg_file_with_no_format_line_reads_as_toml() {
    let read =
        read("[station]\nid = \"hq-1\"\nlisten = \"127.0.0.1:7181\"\n[provider]\npoll = 0.5\n")
            .unwrap();
    assert_eq!(read.station.id.as_deref(), Some("hq-1"));
    assert_eq!(read.station.listen, Some("127.0.0.1:7181".parse().unwrap()));
    assert_eq!(read.provider.poll().as_millis(), 500);
}

#[test]
fn each_extension_reads_its_own_format() {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("ai6.toml", "[station]\nid = \"hq-1\"\n"),
        ("ai6.json", "{\"station\": {\"id\": \"hq-1\"}}\n"),
        ("ai6.yaml", "station:\n  id: hq-1\n"),
        ("ai6.yml", "station:\n  id: hq-1\n"),
    ] {
        let read = Bootstrap::read(&write(dir.path(), name, text)).unwrap();
        assert_eq!(read.station.id.as_deref(), Some("hq-1"), "{name}");
    }
    let refused = Bootstrap::read(&write(dir.path(), "ai6.ini", "station = 1\n")).unwrap_err();
    assert!(
        refused.to_string().contains("cfg, toml, json, yaml, yml"),
        "{refused}"
    );
}

#[test]
fn a_first_line_names_the_format() {
    for text in [
        "#!json\n{\"station\": {\"id\": \"hq-1\"}}\n",
        "#!/usr/bin/env yaml\nstation:\n  id: hq-1\n",
        "#!yml\nstation:\n  id: hq-1\n",
        "%YAML 1.2\n---\nstation:\n  id: hq-1\n",
    ] {
        assert_eq!(
            read(text).unwrap().station.id.as_deref(),
            Some("hq-1"),
            "{text}"
        );
    }
}

#[test]
fn an_unknown_format_name_lists_the_known_ones() {
    let refused = read("#!ini\n[station]\nid = hq-1\n").unwrap_err();
    for format in ["json", "toml", "yaml"] {
        assert!(refused.contains(format), "{refused}");
    }
}

#[test]
fn nothing_is_guessed_from_content() {
    let refused = read("{\"station\": {\"id\": \"hq-1\"}}\n").unwrap_err();
    assert!(
        refused.contains("toml"),
        "a brace does not make JSON: {refused}"
    );
}

#[test]
fn an_unknown_key_is_refused_by_name() {
    let refused = read("config_dir = \"/etc\"\n").unwrap_err();
    assert!(refused.contains("config_dir"), "{refused}");
    let refused = read("[station]\nid = \"hq-1\"\nconfig_dir = \"/etc\"\n").unwrap_err();
    assert!(refused.contains("station.config_dir"), "{refused}");
    let refused = read("[provider]\nlocation = \"x\"\npassword_file = \"p\"\n").unwrap_err();
    assert!(refused.contains("password_file"), "{refused}");
}

#[test]
fn relative_paths_are_beside_the_file_and_defaults_follow_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "ai6.cfg",
        "[provider]\npassword = { file = \"db-password\" }\n\
         [station]\nstate_dir = \"state\"\n[station.plugins]\npaths = [\"plugins\"]\n",
    );
    let located = Bootstrap::find(Some(&path)).unwrap();
    assert_eq!(located.state_dir().unwrap(), dir.path().join("state"));
    assert_eq!(
        located.provider_location().unwrap(),
        dir.path().display().to_string(),
        "the provider defaults to the file's directory"
    );
    assert_eq!(
        located.registry_location().unwrap(),
        dir.path()
            .join("state")
            .join("registry")
            .display()
            .to_string()
    );
    assert_eq!(located.identity_dir().unwrap(), dir.path().join("state"));
    assert_eq!(located.secrets_dir().unwrap(), dir.path().join("secrets"));
    assert_eq!(
        located.value.station.plugins.paths,
        [dir.path().join("plugins")]
    );
    assert_eq!(
        located.value.provider.password,
        Some(SecretRef::File(dir.path().join("db-password")))
    );
    assert!(located.weakenings().is_empty());
}

#[test]
fn a_sql_provider_is_the_registry_unless_one_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "ai6.cfg",
        "[provider]\nlocation = \"postgres://db.example/ai6\"\npassword = { env = \"PGPASS\" }\n",
    );
    let located = Bootstrap::find(Some(&path)).unwrap();
    assert_eq!(
        located.registry_location().unwrap(),
        "postgres://db.example/ai6"
    );
    assert_eq!(
        located.registry_password(),
        Some(&SecretRef::Env("PGPASS".to_owned()))
    );
    assert_eq!(located.value.registry.headquarters_lease().as_secs(), 60);
}

#[test]
fn an_inline_or_environment_secret_is_reported_as_a_weakening() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "ai6.cfg",
        "[provider]\nlocation = \"sqlite:ai6.db\"\npassword = \"hunter2\"\n\
         [unex]\nurl = \"https://unex.example\"\nstation_token = { env = \"UNEX_TOKEN\" }\n",
    );
    let located = Bootstrap::find(Some(&path)).unwrap();
    let weakenings = located.weakenings();
    assert_eq!(weakenings.len(), 2, "{weakenings:?}");
    assert!(weakenings[0].contains("provider password") && weakenings[0].contains("inline"));
    assert!(weakenings[1].contains("UNEX_TOKEN"));
    assert!(
        !format!("{located:?}").contains("hunter2"),
        "a value is never shown"
    );
}

#[test]
fn registry_limits_in_the_file_bound_the_registry_it_opens() {
    let dir = tempfile::tempdir().unwrap();
    for location in ["registry", "sqlite:registry.db"] {
        let path = write(
            dir.path(),
            "ai6.cfg",
            &format!(
                "[registry]\nlocation = {location:?}\nsqlite_busy_timeout = 5\n\
                 [registry.limits.sessions]\nrecords = 2\n"
            ),
        );
        let located = Bootstrap::find(Some(&path)).unwrap();
        let registry = located.value.registry.clone();
        assert_eq!(registry.limits.sessions.records, ai6::Limit::at(2));
        assert_eq!(registry.sqlite_busy_timeout().as_secs(), 5);
        let options = ai6::SqlOptions::for_lease(registry.headquarters_lease())
            .with_busy_timeout(registry.sqlite_busy_timeout())
            .with_limits(registry.limits.clone());
        let opened = ai6::directorates::registry::backend::open_with(
            &located.registry_location().unwrap(),
            &options,
        )
        .unwrap();
        let ttl = std::time::Duration::from_secs(60);
        for id in ["s-1", "s-2"] {
            opened
                .open_session(ai6::NewSession::new(id, "mission-1", "station-a"), ttl)
                .unwrap();
        }
        let refused = opened
            .open_session(ai6::NewSession::new("s-3", "mission-1", "station-a"), ttl)
            .unwrap_err();
        assert!(
            refused.to_string().contains("too many Session records"),
            "{location}: {refused}"
        );
        assert!(located.weakenings().is_empty(), "tighter than the defaults");
    }
}

#[test]
fn an_unbounded_or_larger_limit_is_reported_as_a_weakening() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "ai6.cfg",
        "[provider]\nloadout_bytes = 2097152\n[registry]\nsqlite_busy_timeout = 60\n\
         [registry.limits.sessions]\nrecords = \"none\"\n",
    );
    let located = Bootstrap::find(Some(&path)).unwrap();
    assert_eq!(
        located.value.registry.limits.sessions.records,
        ai6::Limit::UNBOUNDED
    );
    assert_eq!(
        located.weakenings(),
        [
            "registry.limits.sessions.records none is looser than the default 100000",
            "registry.sqlite_busy_timeout 60 is looser than the default 30",
            "provider.loadout_bytes 2097152 is looser than the default 1048576",
        ]
    );
    let json = Bootstrap::read(&write(
        dir.path(),
        "ai6.json",
        "{\"registry\": {\"limits\": {\"cables\": {\"max_ttl\": null}}}}",
    ))
    .unwrap();
    assert_eq!(
        json.registry.limits.cables.max_ttl,
        ai6::TimeLimit::UNBOUNDED
    );
    assert!(read("[registry.limits.sessions]\nrecords = -1\n").is_err());
    assert!(read("[registry.limits.sessions]\nrecordz = 1\n").is_err());
}

#[test]
fn the_search_takes_the_first_place_that_has_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let [explicit, named, user, system] = ["explicit", "named", "user", "system"].map(|name| {
        let dir = dir.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, "ai6.cfg", &format!("[station]\nid = \"{name}\"\n"))
    });
    let search = |explicit: Option<&Path>, named: Option<&Path>| SearchPath {
        explicit: explicit.map(Path::to_path_buf),
        variable: named.map(|path| ("AI6_CONFIG".to_owned(), path.to_path_buf())),
        candidates: vec![user.clone(), system.clone()],
    };
    let station = |search: SearchPath| {
        Bootstrap::find_in(&search)
            .unwrap()
            .value
            .station
            .id
            .unwrap()
    };
    assert_eq!(station(search(Some(&explicit), Some(&named))), "explicit");
    assert_eq!(station(search(None, Some(&named))), "named");
    assert_eq!(station(search(None, None)), "user");
    std::fs::remove_file(&user).unwrap();
    assert_eq!(station(search(None, None)), "system");
    std::fs::remove_file(&system).unwrap();
    let none = Bootstrap::find_in(&search(None, None)).unwrap();
    assert!(none.path.is_none() && none.value.station.id.is_none());

    let missing = dir.path().join("absent.cfg");
    for refused in [search(Some(&missing), None), search(None, Some(&missing))] {
        let error = Bootstrap::find_in(&refused).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("absent.cfg"), "{error}");
    }
}

fn ai6() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai6"));
    command.env_remove("AI6_CONFIG");
    command
}

/// The PEM of a new self-signed certificate authority.
fn ca_pem() -> String {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut parameters = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    parameters.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    parameters.self_signed(&key).unwrap().pem()
}

#[test]
fn a_ca_file_gives_each_certificate_and_is_a_weakening_only_when_set() {
    let dir = tempfile::tempdir().unwrap();
    let two = format!("# the chain\n{}{}", ca_pem(), ca_pem());
    let chain = write(dir.path(), "chain.pem", &two);
    let certificates = read_ca_file(&chain).unwrap();
    assert_eq!(certificates.len(), 2);
    assert_ne!(certificates[0], certificates[1]);

    let path = write(
        dir.path(),
        "ai6.cfg",
        "[unex]\nurl = \"https://unex.example\"\nca_file = \"chain.pem\"\n",
    );
    let located = Bootstrap::find(Some(&path)).unwrap();
    assert_eq!(located.value.unex.ca_file.as_deref(), Some(chain.as_path()));
    assert!(
        located.weakenings().iter().any(|w| w.contains("ca_file")),
        "{:?}",
        located.weakenings()
    );

    let path = write(
        dir.path(),
        "plain.cfg",
        "[unex]\nurl = \"https://unex.example\"\n",
    );
    assert_eq!(
        Bootstrap::find(Some(&path)).unwrap().weakenings(),
        Vec::<String>::new()
    );
}

#[test]
fn a_ca_file_that_trusts_nothing_is_refused_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate().unwrap().serialize_pem();
    let not_x509 = "-----BEGIN CERTIFICATE-----\nAQIDBA==\n-----END CERTIFICATE-----\n";
    let broken = "-----BEGIN CERTIFICATE-----\nAQIDBA==\n";
    for (name, text, says) in [
        ("empty.pem", "", "no certificate"),
        ("words.pem", "not a certificate\n", "no certificate"),
        ("key.pem", key.as_str(), "no certificate"),
        ("not-x509.pem", not_x509, "cannot be read"),
        ("broken.pem", broken, "broken PEM"),
    ] {
        let path = write(dir.path(), name, text);
        let refused = read_ca_file(&path).unwrap_err().to_string();
        assert!(
            refused.contains(name) && refused.contains(says),
            "{name}: {refused}"
        );
    }
    let missing = dir.path().join("missing.pem");
    let refused = read_ca_file(&missing).unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::NotFound);
    assert!(refused.to_string().contains("missing.pem"), "{refused}");
}

#[test]
fn plugins_keep_verified_is_a_count_or_none_and_defaults_to_eight() {
    let dir = tempfile::tempdir().unwrap();
    let read = |text: &str| {
        let path = write(dir.path(), "ai6.cfg", text);
        Bootstrap::find(Some(&path)).map(|located| located.value.station.plugins.keep_verified())
    };
    assert_eq!(read("").unwrap(), ai6::KeepVerified::Newest(8));
    assert_eq!(
        read("[station.plugins]\nkeep_verified = 0\n").unwrap(),
        ai6::KeepVerified::Newest(0)
    );
    assert_eq!(
        read("[station.plugins]\nkeep_verified = 3\n").unwrap(),
        ai6::KeepVerified::Newest(3)
    );
    assert_eq!(
        read("[station.plugins]\nkeep_verified = \"none\"\n").unwrap(),
        ai6::KeepVerified::All
    );
    for refused in ["-1", "\"all\""] {
        assert!(
            read(&format!("[station.plugins]\nkeep_verified = {refused}\n")).is_err(),
            "{refused}"
        );
    }
}

#[test]
fn a_station_with_a_bad_ca_file_does_not_start() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "empty.pem", "");
    for unex in [
        "url = \"https://unex.example\"\nca_file = \"empty.pem\"\n",
        "url = \"https://unex.example\"\nca_file = \"missing.pem\"\nstation_token = \"t\"\n",
    ] {
        let path = write(dir.path(), "ai6.cfg", &format!("[unex]\n{unex}"));
        let output = ai6()
            .args(["serve", "--listen", "127.0.0.1:0", "--config"])
            .arg(&path)
            .arg("--state-dir")
            .arg(dir.path().join("state"))
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!output.status.success(), "{said}");
        assert!(said.contains("[unex] ca_file"), "{said}");
        assert!(said.contains(".pem"), "{said}");
    }
}

#[test]
fn config_path_follows_the_flag_then_the_variable_then_the_user_directory() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let user = home.join("ai6");
    std::fs::create_dir_all(&user).unwrap();
    let in_user = write(&user, "ai6.cfg", "");
    let named = write(dir.path(), "named.cfg", "");
    let flag = write(dir.path(), "flag.cfg", "");
    let path = |flag: Option<&Path>, named: Option<&Path>| {
        let mut command = ai6();
        command
            .args(["config", "path"])
            .env("XDG_CONFIG_HOME", &home)
            .env("APPDATA", &home);
        if let Some(flag) = flag {
            command.arg("--config").arg(flag);
        }
        if let Some(named) = named {
            command.env("AI6_CONFIG", named);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    assert_eq!(path(Some(&flag), Some(&named)), flag.display().to_string());
    assert_eq!(path(None, Some(&named)), named.display().to_string());
    assert_eq!(path(None, None), in_user.display().to_string());
}

#[test]
fn serve_help_lists_none_of_the_removed_flags() {
    let output = ai6().args(["serve", "--help"]).output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for removed in [
        ["--config", "dir"].join("-").as_str(),
        "--config-poll",
        "--headquarters-lease",
        "EXPORTS_",
    ] {
        assert!(!help.contains(removed), "{removed}");
    }
    for kept in [
        "--config",
        "--provider",
        "--registry",
        "--state-dir",
        "--station",
        "--listen",
        "--plugin-path",
        "--plugin-profile",
    ] {
        assert!(help.contains(kept), "{kept}");
    }
}

/// A value a person could keep in a file, and what no message may repeat.
const PLANTED: &str = "pl4nted-s3cret-value";

/// Writes the one file every program starts from in `dir`, with the addresses it needs.
fn one_file(dir: &Path, unex_url: &str, gateway: u16, station: u16) -> PathBuf {
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::write(dir.join("station-token"), "t".repeat(48)).unwrap();
    let text = include_str!("one_file.cfg")
        .replace("UNEX_URL", unex_url)
        .replace("GATEWAY_LISTEN", &format!("127.0.0.1:{gateway}"))
        .replace("STATION_LISTEN", &format!("127.0.0.1:{station}"));
    write(dir, "ai6.cfg", &text)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn a_station_starts_from_the_one_file_every_program_shares() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    // Publishing to the closed port goes on in the background and does not stop the Station.
    let config = one_file(dir.path(), "http://127.0.0.1:9", free_port(), port);
    let mut station = ai6()
        .args(["serve", "--config"])
        .arg(&config)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut listening = false;
    while std::time::Instant::now() < deadline && station.try_wait().unwrap().is_none() {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            listening = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = station.kill();
    let output = station.wait_with_output().unwrap();
    assert!(
        listening,
        "the Station listens on [station] listen: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.path().join("station-state").is_dir(),
        "its state is under [station] state_dir"
    );
    assert!(
        !dir.path().join("unex-state").exists(),
        "the gateway's directory is not the Station's"
    );
}

#[test]
fn a_station_refuses_a_key_nobody_knows_and_does_not_repeat_its_value() {
    let dir = tempfile::tempdir().unwrap();
    let config = one_file(dir.path(), "http://127.0.0.1:9", free_port(), free_port());
    let text = std::fs::read_to_string(&config).unwrap();
    for (document, key) in [
        (
            format!("nobody_knows = \"{PLANTED}\"\n{text}"),
            "nobody_knows",
        ),
        (
            text.replace(
                "[unex.access]",
                &format!("[unex.access]\nnobody = \"{PLANTED}\""),
            ),
            "unex.access.nobody",
        ),
        (
            text.replace("[station]", &format!("[station]\nlisten2 = \"{PLANTED}\"")),
            "station.listen2",
        ),
    ] {
        std::fs::write(&config, document).unwrap();
        let output = ai6()
            .args(["serve", "--config"])
            .arg(&config)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{said}");
        assert!(said.contains(key), "names {key}: {said}");
        assert!(!said.contains(PLANTED), "{said}");
    }
}

/// `[oo]` is the app's: no Rust program asks what is in it.
#[test]
fn every_program_leaves_oo_unread() {
    let read = read("[oo]\nanything = { at = \"all\" }\nnumber = 1\n").unwrap();
    assert_eq!(read.oo["number"], 1);
}

#[test]
fn a_secret_in_a_refused_document_appears_in_no_message_in_any_format() {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in [
        (
            "toml.cfg",
            format!("[provider]\npassword = \"{PLANTED}\"\nunknown = 1\n"),
        ),
        (
            "json.json",
            format!(r#"{{"provider": {{"password": "{PLANTED}", "unknown": 1}}}}"#),
        ),
        (
            "yaml.yaml",
            format!("provider:\n  password: {PLANTED}\n  unknown: 1\n"),
        ),
        ("toml2.cfg", format!("[unex]\nlisten = \"{PLANTED}\"\n")),
        (
            "json2.cfg",
            format!("#!json\n{{\"unex\": {{\"listen\": \"{PLANTED}\"}}}}\n"),
        ),
        ("yaml2.cfg", format!("#!yaml\nunex:\n  listen: {PLANTED}\n")),
        ("broken.cfg", format!("[unex]\nurl = \"{PLANTED}\n")),
        (
            "brokenjson.cfg",
            format!("#!json\n{{\"url\": \"{PLANTED}\n"),
        ),
        ("brokenyaml.yaml", format!("url: \"{PLANTED}\n")),
    ] {
        let path = write(dir.path(), name, &text);
        let refused = Bootstrap::read(&path).unwrap_err();
        let shown = format!("{refused} {refused:?}");
        assert!(!shown.contains(PLANTED), "{name}: {shown}");
        let output = ai6()
            .args(["identity", "--config"])
            .arg(&path)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{name}: {said}");
        assert!(!said.contains(PLANTED), "{name}: {said}");
    }
}

#[test]
fn a_file_of_the_old_layout_is_refused_and_says_where_the_key_is_wrong() {
    for (text, key) in [
        ("station = \"hq-1\"\n", "station"),
        ("state_dir = \"state\"\n", "state_dir"),
        ("[plugins]\npaths = []\n", "plugins"),
        ("[identity]\ndir = \"x\"\n", "identity"),
        ("[secrets]\ndir = \"x\"\n", "secrets"),
        ("[unex]\nurl = \"https://u\"\ntoken = \"t\"\n", "unex.token"),
    ] {
        let refused = read(text).map(|_| ()).unwrap_err();
        assert!(refused.contains(key), "{text}: {refused}");
    }
}

#[test]
fn the_token_is_one_key_for_both_sides_of_the_station_channel() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "station-token", "t");
    let path = write(
        dir.path(),
        "ai6.cfg",
        "[unex]\nurl = \"https://unex.example\"\nstation_token = { file = \"station-token\" }\n",
    );
    let located = Bootstrap::find(Some(&path)).unwrap();
    assert_eq!(
        located.value.unex.station_token,
        Some(SecretRef::File(dir.path().join("station-token"))),
        "relative to the file, whichever program reads it"
    );
}
