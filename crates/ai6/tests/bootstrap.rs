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
        read("station = \"hq-1\"\nlisten = \"127.0.0.1:7181\"\n[provider]\npoll = 0.5\n").unwrap();
    assert_eq!(read.station.as_deref(), Some("hq-1"));
    assert_eq!(read.listen, Some("127.0.0.1:7181".parse().unwrap()));
    assert_eq!(read.provider.poll().as_millis(), 500);
}

#[test]
fn each_extension_reads_its_own_format() {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("ai6.toml", "station = \"hq-1\"\n"),
        ("ai6.json", "{\"station\": \"hq-1\"}\n"),
        ("ai6.yaml", "station: hq-1\n"),
        ("ai6.yml", "station: hq-1\n"),
    ] {
        let read = Bootstrap::read(&write(dir.path(), name, text)).unwrap();
        assert_eq!(read.station.as_deref(), Some("hq-1"), "{name}");
    }
    let refused = Bootstrap::read(&write(dir.path(), "ai6.ini", "station = 1\n")).unwrap_err();
    assert!(
        refused.to_string().contains("cfg, toml, json, yaml, yml"),
        "{refused}"
    );
}

#[test]
#[ignore = "guatiao text::cfg"]
fn a_first_line_names_the_format() {
    for text in [
        "#!json\n{\"station\": \"hq-1\"}\n",
        "#!/usr/bin/env yaml\nstation: hq-1\n",
        "#!yml\nstation: hq-1\n",
        "%YAML 1.2\n---\nstation: hq-1\n",
    ] {
        assert_eq!(
            read(text).unwrap().station.as_deref(),
            Some("hq-1"),
            "{text}"
        );
    }
}

#[test]
#[ignore = "guatiao text::cfg"]
fn an_unknown_format_name_lists_the_known_ones() {
    let refused = read("#!ini\nstation = hq-1\n").unwrap_err();
    for format in ["json", "toml", "yaml"] {
        assert!(refused.contains(format), "{refused}");
    }
}

#[test]
fn nothing_is_guessed_from_content() {
    let refused = read("{\"station\": \"hq-1\"}\n").unwrap_err();
    assert!(
        refused.contains("toml"),
        "a brace does not make JSON: {refused}"
    );
}

#[test]
fn an_unknown_key_is_refused_by_name() {
    let refused = read("station = \"hq-1\"\nconfig_dir = \"/etc\"\n").unwrap_err();
    assert!(refused.contains("config_dir"), "{refused}");
    let refused = read("[provider]\nlocation = \"x\"\npassword_file = \"p\"\n").unwrap_err();
    assert!(refused.contains("password_file"), "{refused}");
}

#[test]
fn relative_paths_are_beside_the_file_and_defaults_follow_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "ai6.cfg",
        "state_dir = \"state\"\n[provider]\npassword = { file = \"db-password\" }\n\
         [plugins]\npaths = [\"plugins\"]\n",
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
    assert_eq!(located.value.plugins.paths, [dir.path().join("plugins")]);
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
         [unex]\nurl = \"https://unex.example\"\ntoken = { env = \"UNEX_TOKEN\" }\n",
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
        write(&dir, "ai6.cfg", &format!("station = \"{name}\"\n"))
    });
    let search = |explicit: Option<&Path>, named: Option<&Path>| SearchPath {
        explicit: explicit.map(Path::to_path_buf),
        variable: named.map(|path| ("AI6_CONFIG".to_owned(), path.to_path_buf())),
        candidates: vec![user.clone(), system.clone()],
    };
    let station = |search: SearchPath| Bootstrap::find_in(&search).unwrap().value.station.unwrap();
    assert_eq!(station(search(Some(&explicit), Some(&named))), "explicit");
    assert_eq!(station(search(None, Some(&named))), "named");
    assert_eq!(station(search(None, None)), "user");
    std::fs::remove_file(&user).unwrap();
    assert_eq!(station(search(None, None)), "system");
    std::fs::remove_file(&system).unwrap();
    let none = Bootstrap::find_in(&search(None, None)).unwrap();
    assert!(none.path.is_none() && none.value.station.is_none());

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
    let unex = located.value.unex.as_ref().unwrap();
    assert_eq!(unex.ca_file.as_deref(), Some(chain.as_path()));
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
fn a_station_with_a_bad_ca_file_does_not_start() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "empty.pem", "");
    for unex in [
        "url = \"https://unex.example\"\nca_file = \"empty.pem\"\n",
        "url = \"https://unex.example\"\nca_file = \"missing.pem\"\ntoken = \"t\"\n",
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
