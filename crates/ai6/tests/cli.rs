use std::process::Command;

fn ai6() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai6"));
    // A bootstrap file named in the test's own environment must not reach the command.
    command.env_remove("AI6_CONFIG");
    command
}

/// Writes `ai6.cfg` in `dir` keeping the state in `dir/state`, with `extra` appended, and
/// returns its path. Its directory is the configuration provider and holds the Registry.
fn bootstrap(dir: &std::path::Path, extra: &str) -> std::path::PathBuf {
    let path = dir.join("ai6.cfg");
    std::fs::write(&path, format!("[station]\nstate_dir = \"state\"\n{extra}")).unwrap();
    path
}

#[test]
fn serve_help_lists_only_what_starting_needs() {
    let output = ai6()
        .args(["serve", "--help"])
        .output()
        .expect("the AI6 command should start");

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("help should be UTF-8");
    let flags: Vec<&str> = help
        .split_whitespace()
        .filter(|word| word.starts_with("--"))
        .map(|word| word.trim_end_matches([',', '.', ')']))
        .filter(|word| !word.contains('`'))
        .collect();
    let mut flags: Vec<&str> = flags.into_iter().filter(|flag| *flag != "--help").collect();
    flags.sort_unstable();
    flags.dedup();
    assert_eq!(
        flags,
        [
            "--config",
            "--listen",
            "--plugin-path",
            "--plugin-profile",
            "--provider",
            "--registry",
            "--state-dir",
            "--station",
        ]
    );
}

#[test]
fn plugins_pin_prints_an_entry_for_a_provider_library_and_refuses_anything_else() {
    let library = built_plugin("coqui_roles_plugin");
    let output = ai6()
        .args(["plugins", "pin"])
        .arg(&library)
        .output()
        .expect("the AI6 command should start");
    assert!(output.status.success(), "{output:?}");
    let entry = String::from_utf8(output.stdout).unwrap();
    assert!(entry.starts_with("[[library]]\n"), "{entry}");
    assert!(entry.contains("pin = \"sha256:"), "{entry}");

    let output = ai6()
        .args(["plugins", "pin"])
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("the AI6 command should start");
    assert!(!output.status.success());
}

#[test]
fn plugins_pin_import_replaces_a_rebuilt_library_s_record_and_keeps_the_others() {
    use ai6::ConfigProvider as _;

    let root = tempfile::tempdir().unwrap();
    let config = bootstrap(root.path(), "[provider]\nlocation = \"sqlite:ai6.db\"\n");
    let plugins = root.path().join("plugins");
    std::fs::create_dir(&plugins).unwrap();
    let built = std::fs::read(built_plugin("coqui_roles_plugin")).unwrap();
    let name = |stem: &str| format!("{stem}.{}", std::env::consts::DLL_EXTENSION);
    let (first, second) = (plugins.join(name("first")), plugins.join(name("second")));
    std::fs::write(&first, &built).unwrap();
    std::fs::write(&second, &built).unwrap();
    let digest = |bytes: &[u8]| format!("sha256:{}", hex(&sha256(bytes)));
    let import = |paths: &[&std::path::Path]| {
        let mut command = ai6();
        command.args(["plugins", "pin", "--import", "--config"]);
        command.arg(&config).args(paths);
        command.output().expect("the AI6 command should start")
    };
    let stored = || {
        let sql =
            ai6::SqlProvider::open(&format!("sqlite:{}", root.path().join("ai6.db").display()))
                .unwrap();
        sql.definitions(ai6::ConfigRecord::Plugin, &ai6::ConfigScope::Global)
            .unwrap()
            .records
            .into_iter()
            .map(|(file, definition)| (file, definition["pin"].as_str().unwrap().to_owned()))
            .collect::<Vec<_>>()
    };

    let output = import(&[&first, &second]);
    assert!(output.status.success(), "{output:?}");
    let said = text(&output.stdout);
    assert!(
        said.contains(&format!("added {}: pin {}", name("first"), digest(&built))),
        "{said}"
    );
    assert!(
        said.contains(&format!("added {}", name("second"))),
        "{said}"
    );
    assert_eq!(
        stored(),
        [
            (name("first"), digest(&built)),
            (name("second"), digest(&built))
        ]
    );

    // One library is rebuilt: its record follows its bytes, and the other stays as it was.
    let mut rebuilt = built.clone();
    rebuilt.extend_from_slice(b"rebuilt");
    std::fs::write(&first, &rebuilt).unwrap();
    let output = import(&[&first, &second]);
    assert!(output.status.success(), "{output:?}");
    let said = text(&output.stdout);
    assert!(
        said.contains(&format!(
            "changed {}: pin {} -> pin {}",
            name("first"),
            digest(&built),
            digest(&rebuilt)
        )),
        "{said}"
    );
    assert!(
        said.contains(&format!("unchanged {}", name("second"))),
        "{said}"
    );
    assert!(
        said.contains("1 of 2 plugin records written at global"),
        "{said}"
    );
    assert_eq!(
        stored(),
        [
            (name("first"), digest(&rebuilt)),
            (name("second"), digest(&built))
        ]
    );

    // Naming one library leaves every other record alone.
    std::fs::write(&first, &built).unwrap();
    let output = import(&[&first]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stored(),
        [
            (name("first"), digest(&built)),
            (name("second"), digest(&built))
        ]
    );

    // A directory of family files takes no changes: nothing is written there.
    let files = tempfile::tempdir().unwrap();
    let read_only = bootstrap(files.path(), "");
    let output = ai6()
        .args(["plugins", "pin", "--import", "--config"])
        .arg(&read_only)
        .arg(&first)
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        text(&output.stderr).contains("takes no changes"),
        "{output:?}"
    );
    assert!(!files.path().join("plugins.toml").exists());

    // --scope means nothing without --import.
    let output = ai6()
        .args(["plugins", "pin", "--scope", "station:a"])
        .arg(&first)
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
}

/// An access plugin Cargo built beside this test binary: they are dev-dependencies.
fn built_plugin(name: &str) -> std::path::PathBuf {
    let deps = std::env::current_exe().unwrap();
    std::fs::read_dir(deps.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == std::env::consts::DLL_EXTENSION)
                && path
                    .file_name()
                    .is_some_and(|name_found| name_found.to_string_lossy().contains(name))
        })
        .expect("the plugin is built beside the test binary")
}

/// Runs `ai6 plugins` with these arguments.
macro_rules! plugins_run {
    ($($arg:expr),+ $(,)?) => {{
        let mut command = ai6();
        command.arg("plugins");
        $(command.arg($arg);)+
        command.output().expect("the AI6 command should start")
    }};
}

/// A deployment in a temporary directory: `ai6.cfg` and its records, `plugins/roles.EXT` with a
/// signature once signed, and a publisher key the test generated there.
struct Signed {
    root: tempfile::TempDir,
    key: std::path::PathBuf,
    publisher: String,
    library: std::path::PathBuf,
    config: std::path::PathBuf,
}

impl Signed {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = bootstrap(root.path(), "[station.plugins]\npaths = [\"plugins\"]\n");
        let plugins = root.path().join("plugins");
        std::fs::create_dir(&plugins).unwrap();
        let library = plugins.join(library_name());
        std::fs::copy(built_plugin("coqui_roles_plugin"), &library).unwrap();
        let key = root.path().join("publisher.key");
        let output = plugins_run!("keygen", "--out", &key);
        assert!(output.status.success(), "{output:?}");
        let publisher = String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .strip_prefix("publisher = \"")
            .unwrap()
            .strip_suffix('"')
            .unwrap()
            .to_owned();
        Self {
            root,
            key,
            publisher,
            library,
            config,
        }
    }

    fn sign(&self) {
        let output = plugins_run!("sign", "--key", &self.key, &self.library);
        assert!(output.status.success(), "{output:?}");
    }

    /// Writes `plugins.toml`, the provider's `plugin` records.
    fn records(&self, text: &str) {
        std::fs::write(self.root.path().join("plugins.toml"), text).unwrap();
    }

    fn publisher_entry(&self) -> String {
        format!(
            "[[library]]\nfile = \"{}\"\npublisher = \"{}\"\n",
            library_name(),
            self.publisher
        )
    }

    fn check(&self) -> std::process::Output {
        plugins_run!("check", "--config", &self.config)
    }
}

fn library_name() -> String {
    format!("roles.{}", std::env::consts::DLL_EXTENSION)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Standard base64 with padding: what a PEM file holds.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let group = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                let digit = (group >> (18 - 6 * index)) & 63;
                out.push(ALPHABET[digit as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A random Ed25519 key as OpenSSL writes one, PKCS#8 version 1 with no public key: its DER, its
/// PEM and its public half.
fn openssl_style_key() -> (Vec<u8>, String, String) {
    use ring::rand::SecureRandom;
    use ring::signature::KeyPair;
    let mut seed = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut seed).unwrap();
    let mut der = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    der.extend_from_slice(&seed);
    let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    let pem = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        base64(&der)
    );
    let public = format!("ed25519:{}", hex(pair.public_key().as_ref()));
    (der, pem, public)
}

#[test]
fn plugins_keygen_sign_and_check_pass_over_the_tag_the_name_and_the_digest() {
    let signed = Signed::new();
    assert_eq!(signed.publisher.len(), "ed25519:".len() + 64);

    let output = plugins_run!("sign", "--key", &signed.key, &signed.library,);
    assert!(output.status.success(), "{output:?}");
    let name = library_name();
    let signature_path = signed.library.with_file_name(format!("{name}.sig"));
    assert_eq!(
        text(&output.stdout).trim(),
        signature_path.display().to_string()
    );

    // Spelled out here, and not through the crate's own function: the format is the contract.
    let signature = std::fs::read(&signature_path).unwrap();
    assert_eq!(signature.len(), 64);
    let mut message = b"ai6-provider-library-v1\0".to_vec();
    message.extend_from_slice(name.as_bytes());
    message.push(0);
    message.extend_from_slice(&sha256(&std::fs::read(&signed.library).unwrap()));
    let public = (0..32)
        .map(|index| {
            u8::from_str_radix(&signed.publisher["ed25519:".len()..][index * 2..][..2], 16).unwrap()
        })
        .collect::<Vec<u8>>();
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public)
        .verify(&message, &signature)
        .expect("the signature is the key's, over the tag, the name and the digest");

    // The entry reads the same from the key text and from the key's file.
    for key in [
        std::ffi::OsString::from(&signed.publisher),
        signed.key.clone().into_os_string(),
    ] {
        let output = plugins_run!("pin", "--publisher", &key, &signed.library);
        assert!(output.status.success(), "{output:?}");
        let printed = ai6::PinSet::from_toml(&text(&output.stdout)).unwrap();
        assert_eq!(printed.libraries[0].file, name);
        assert_eq!(
            printed.libraries[0].publisher.unwrap().to_string(),
            signed.publisher
        );
        assert_eq!(printed.libraries[0].pin, None);
        assert_eq!(printed.libraries[0].require, ai6::Require::Either);
        assert!(
            text(&output.stderr).contains(&format!(
                "publisher {} alone admits 1 library: anything it signed loads, older builds included",
                signed.publisher
            )),
            "{output:?}"
        );
    }
    signed.records(&signed.publisher_entry());

    let output = signed.check();
    assert!(output.status.success(), "{output:?}");
    let said = text(&output.stdout);
    assert!(
        said.contains(&format!(
            "sha256:{}",
            hex(&sha256(&std::fs::read(&signed.library).unwrap()))
        )) && said.contains(&format!("accepted: publisher {}", signed.publisher)),
        "{said}"
    );
    assert!(text(&output.stderr).contains("alone admits"), "{output:?}");
    assert!(
        !signed.root.path().join("state/plugins/verified").exists(),
        "a check writes no copy"
    );
}

#[test]
fn plugins_check_refuses_a_changed_byte_naming_the_digest() {
    let signed = Signed::new();
    signed.sign();
    signed.records(&signed.publisher_entry());
    assert!(signed.check().status.success());

    let original = std::fs::read(&signed.library).unwrap();
    let mut changed = original.clone();
    *changed.last_mut().unwrap() ^= 0x5a;
    std::fs::write(&signed.library, &changed).unwrap();
    let output = signed.check();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let said = text(&output.stdout);
    assert!(
        said.contains(&format!("sha256:{}", hex(&sha256(&changed)))),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "refused: publisher {}: the signature does not verify",
            signed.publisher
        )),
        "{said}"
    );

    // A digest entry says what it expected as well.
    signed.records(&format!(
        "[[library]]\nfile = \"{}\"\npin = \"sha256:{}\"\n",
        library_name(),
        hex(&sha256(&original))
    ));
    let output = signed.check();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        text(&output.stdout).contains(&format!(
            "expected sha256:{}, found sha256:{}",
            hex(&sha256(&original)),
            hex(&sha256(&changed))
        )),
        "{output:?}"
    );
}

#[test]
fn plugins_check_refuses_a_signature_made_for_another_file_name() {
    let signed = Signed::new();
    signed.sign();
    // The same bytes and signature, moved into another provider's slot.
    let other = format!("other.{}", std::env::consts::DLL_EXTENSION);
    let plugins = signed.library.parent().unwrap();
    std::fs::rename(&signed.library, plugins.join(&other)).unwrap();
    std::fs::rename(
        plugins.join(format!("{}.sig", library_name())),
        plugins.join(format!("{other}.sig")),
    )
    .unwrap();
    signed.records(&format!(
        "[[library]]\nfile = \"{other}\"\npublisher = \"{}\"\n",
        signed.publisher
    ));
    let output = signed.check();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        text(&output.stdout).contains(&format!(
            "refused: publisher {}: the signature does not verify",
            signed.publisher
        )),
        "{output:?}"
    );
}

#[test]
fn plugins_check_refuses_another_key_a_missing_signature_and_a_library_with_no_entry() {
    let signed = Signed::new();
    let (_, _, stranger) = openssl_style_key();
    signed.sign();
    signed.records(&format!(
        "[[library]]\nfile = \"{}\"\npublisher = \"{stranger}\"\n",
        library_name()
    ));
    let output = signed.check();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        text(&output.stdout).contains(&format!(
            "refused: publisher {stranger}: the signature does not verify"
        )),
        "{output:?}"
    );

    signed.records(&signed.publisher_entry());
    std::fs::remove_file(
        signed
            .library
            .with_file_name(format!("{}.sig", library_name())),
    )
    .unwrap();
    let output = signed.check();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        text(&output.stdout).contains(&format!("no signature file {}.sig", library_name())),
        "{output:?}"
    );

    signed.records("");
    let output = signed.check();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(text(&output.stdout).contains("unpinned"), "{output:?}");

    // With nothing to look at, a check has shown nothing.
    std::fs::remove_file(&signed.library).unwrap();
    let output = signed.check();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        text(&output.stderr).contains("no AI6 provider library to check"),
        "{output:?}"
    );
}

#[test]
fn plugins_check_under_require_both_wants_the_exact_bytes_and_lists_no_weakening() {
    let signed = Signed::new();
    signed.sign();
    let original = std::fs::read(&signed.library).unwrap();
    signed.records(&format!(
        "[[library]]\nfile = \"{}\"\npin = \"sha256:{}\"\npublisher = \"{}\"\nrequire = \"both\"\n",
        library_name(),
        hex(&sha256(&original)),
        signed.publisher
    ));
    let output = signed.check();
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains(&format!(
            "accepted: pin sha256:{} and publisher {}",
            hex(&sha256(&original)),
            signed.publisher
        )),
        "{output:?}"
    );
    assert!(!text(&output.stderr).contains("alone admits"), "{output:?}");

    // A rebuild its publisher signed is still not the pinned bytes.
    let mut rebuilt = original.clone();
    rebuilt.extend_from_slice(b"rebuilt");
    std::fs::write(&signed.library, &rebuilt).unwrap();
    signed.sign();
    let output = signed.check();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        text(&output.stdout).contains(&format!(
            "expected sha256:{}, found sha256:{}",
            hex(&sha256(&original)),
            hex(&sha256(&rebuilt))
        )),
        "{output:?}"
    );
}

#[test]
fn plugins_keygen_refuses_to_overwrite() {
    let signed = Signed::new();
    let held = std::fs::read(&signed.key).unwrap();
    let output = plugins_run!("keygen", "--out", &signed.key);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(std::fs::read(&signed.key).unwrap(), held);
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn plugins_sign_refuses_what_is_not_a_provider_library_and_a_file_that_is_no_key() {
    let signed = Signed::new();
    let notes = signed.root.path().join("notes.txt");
    std::fs::write(&notes, "text").unwrap();
    let output = plugins_run!("sign", "--key", &signed.key, &notes);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(!signed.root.path().join("notes.txt.sig").exists());

    let output = plugins_run!("sign", "--key", &notes, &signed.library);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        text(&output.stderr).contains("not a PKCS#8 Ed25519 private key"),
        "{output:?}"
    );
    assert!(
        !signed
            .library
            .with_file_name(format!("{}.sig", library_name()))
            .exists()
    );
}

#[test]
fn plugins_sign_and_pin_read_a_key_as_openssl_writes_one() {
    let signed = Signed::new();
    let (der, pem, public) = openssl_style_key();
    let der_path = signed.root.path().join("openssl.der");
    let pem_path = signed.root.path().join("openssl.pem");
    std::fs::write(&der_path, der).unwrap();
    std::fs::write(&pem_path, pem).unwrap();
    signed.records(&format!(
        "[[library]]\nfile = \"{}\"\npublisher = \"{public}\"\n",
        library_name()
    ));
    for key in [&der_path, &pem_path] {
        let output = plugins_run!("pin", "--publisher", key, &signed.library);
        assert!(text(&output.stdout).contains(&public), "{output:?}");
        let output = plugins_run!("sign", "--key", key, &signed.library);
        assert!(output.status.success(), "{output:?}");
        assert!(signed.check().status.success());
    }
}

#[test]
fn plugins_key_text_appears_in_no_output() {
    let signed = Signed::new();
    let der = std::fs::read(&signed.key).unwrap();
    // The layout `spilled` assumes: the seed derives the public key this key prints.
    let seed = &der[16..48];
    assert!(
        signed
            .publisher
            .ends_with(&hex(ring::signature::KeyPair::public_key(
                &ring::signature::Ed25519KeyPair::from_seed_unchecked(seed).unwrap()
            )
            .as_ref())),
        "those are the bytes the assertions below look for"
    );
    let pem_path = signed.root.path().join("broken.pem");
    std::fs::write(
        &pem_path,
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}!\n-----END PRIVATE KEY-----\n",
            base64(&der)
        ),
    )
    .unwrap();
    signed.records(&signed.publisher_entry());

    let spilled = |der: &[u8], output: std::process::Output| {
        // PKCS#8 version 2 as `ring` writes it: 16 bytes of framing, then the secret seed.
        let seed = &der[16..48];
        for stream in [&output.stdout, &output.stderr] {
            let said = text(stream);
            assert!(!stream.windows(32).any(|part| part == seed), "{output:?}");
            assert!(!said.contains(&hex(seed)), "{output:?}");
            assert!(!said.contains(&hex(der)), "{output:?}");
            assert!(!said.contains(&base64(der)), "{output:?}");
            assert!(!said.contains(&format!("{der:?}")), "{output:?}");
            assert!(!said.contains("PRIVATE KEY-----"), "{output:?}");
        }
    };

    // A new key, with what `keygen` said held up against that key.
    let fresh = signed.root.path().join("fresh.key");
    let output = plugins_run!("keygen", "--out", &fresh);
    spilled(&std::fs::read(&fresh).unwrap(), output);

    let spilled = |output| spilled(&der, output);
    spilled(plugins_run!("keygen", "--out", &signed.key));
    spilled(plugins_run!("sign", "--key", &signed.key, &signed.library));
    spilled(plugins_run!(
        "pin",
        "--publisher",
        &signed.key,
        &signed.library,
    ));
    spilled(signed.check());
    // Each way the key file is handed to the wrong place, and a key that does not parse.
    spilled(plugins_run!("sign", "--key", &signed.key, &signed.key));
    spilled(plugins_run!("pin", &signed.key));
    spilled(plugins_run!("sign", "--key", &pem_path, &signed.library));
    spilled(plugins_run!(
        "pin",
        "--publisher",
        &pem_path,
        &signed.library,
    ));
}

/// The library Windows signs in with is the one a deployment pins: the check that decides whether
/// a file is a provider library accepts it.
#[cfg(windows)]
#[test]
fn plugins_pin_and_sign_accept_the_system_access_plugin() {
    let signed = Signed::new();
    let library = signed.root.path().join(format!(
        "coqui_access_system_plugin.{}",
        std::env::consts::DLL_EXTENSION
    ));
    std::fs::copy(built_plugin("coqui_access_system_plugin"), &library).unwrap();
    let output = plugins_run!("pin", &library);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains("pin = \"sha256:"),
        "{output:?}"
    );
    let output = plugins_run!("sign", "--key", &signed.key, &library);
    assert!(output.status.success(), "{output:?}");
    let output = plugins_run!("pin", "--publisher", &signed.key, &library);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains(&signed.publisher),
        "{output:?}"
    );
}

/// What a process in a test waits for: generous, since it only ever ends the wait early.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(300);

fn until<T>(what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// A process stopped when dropped, so that a failed test leaves nothing running.
struct Running(std::process::Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn verified_copy(state: &std::path::Path, bytes: &[u8]) -> std::path::PathBuf {
    state.join("plugins").join("verified").join(format!(
        "sha256-{}.{}",
        hex(&sha256(bytes)),
        std::env::consts::DLL_EXTENSION
    ))
}

/// Starts a Station on a port of its own, and returns it with what it says about its plugins.
fn station_plugins(signed: &Signed, log: &std::path::Path) -> (Running, serde_json::Value) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let log_file = std::fs::File::create(log).unwrap();
    let station = Running(
        ai6()
            .args(["serve", "--listen"])
            .arg(format!("127.0.0.1:{port}"))
            .arg("--config")
            .arg(&signed.config)
            .stdin(std::process::Stdio::null())
            .stdout(log_file.try_clone().unwrap())
            .stderr(log_file)
            .spawn()
            .expect("the AI6 command should start"),
    );
    let state = signed.root.path().join("state");
    let summary = until("the Station to answer", || {
        let token = std::fs::read_to_string(state.join("api-token")).ok()?;
        ureq::get(format!("http://127.0.0.1:{port}/internal/plugins"))
            .header("Authorization", format!("Bearer {}", token.trim()))
            .call()
            .ok()?
            .body_mut()
            .read_json::<serde_json::Value>()
            .ok()
    });
    (station, summary)
}

/// Every file under `directory` that holds `needle`.
fn holding_text(directory: &std::path::Path, needle: &str) -> bool {
    let Ok(listing) = std::fs::read_dir(directory) else {
        return false;
    };
    listing.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            holding_text(&path, needle)
        } else {
            std::fs::read(&path).is_ok_and(|bytes| text(&bytes).contains(needle))
        }
    })
}

#[test]
fn plugins_a_station_under_strict_loads_a_rebuilt_library_with_no_record_change() {
    let signed = Signed::new();
    signed.sign();
    signed.records(&signed.publisher_entry());
    let records = std::fs::read(signed.root.path().join("plugins.toml")).unwrap();
    let state = signed.root.path().join("state");
    let log = signed.root.path().join("station.log");
    let original = std::fs::read(&signed.library).unwrap();

    let (first, summary) = station_plugins(&signed, &log);
    let shown = summary.to_string();
    assert_eq!(summary["profile"], "strict", "{shown}");
    assert_eq!(
        summary["checked"][0]["verdict"],
        format!("accepted: publisher {}", signed.publisher),
        "{shown}"
    );
    let line = format!(
        "publisher {} alone admits 1 library: anything it signed loads, older builds included",
        signed.publisher
    );
    assert_eq!(summary["weakened"][0], line.as_str(), "{shown}");
    assert!(verified_copy(&state, &original).exists(), "{shown}");
    assert!(
        std::fs::read_to_string(&log).unwrap().contains(&line),
        "the log says it too"
    );
    until("the check-in to carry the weakening", || {
        holding_text(&state.join("registry"), "alone admits").then_some(())
    });
    drop(first);

    // The build changes; the signature follows it; no record does.
    let mut rebuilt = original.clone();
    rebuilt.extend_from_slice(b"rebuilt");
    std::fs::write(&signed.library, &rebuilt).unwrap();
    signed.sign();

    let (second, summary) = station_plugins(&signed, &log);
    let shown = summary.to_string();
    assert_eq!(
        summary["checked"][0]["digest"],
        format!("sha256:{}", hex(&sha256(&rebuilt))),
        "{shown}"
    );
    assert_eq!(
        summary["loaded"][0],
        signed.library.display().to_string(),
        "{shown}"
    );
    assert!(verified_copy(&state, &rebuilt).exists(), "{shown}");
    assert!(
        verified_copy(&state, &original).exists(),
        "kept by the default: {shown}"
    );
    drop(second);

    // Told to keep nothing else, the next start clears the old build and only that.
    std::fs::write(
        &signed.config,
        "[station]\nstate_dir = \"state\"\n[station.plugins]\npaths = [\"plugins\"]\nkeep_verified = 0\n",
    )
    .unwrap();
    let (third, summary) = station_plugins(&signed, &log);
    let shown = summary.to_string();
    assert_eq!(
        summary["verified_removed"][0],
        verified_copy(&state, &original).display().to_string(),
        "{shown}"
    );
    assert!(!verified_copy(&state, &original).exists(), "{shown}");
    assert!(verified_copy(&state, &rebuilt).exists(), "{shown}");
    drop(third);

    assert_eq!(
        std::fs::read(signed.root.path().join("plugins.toml")).unwrap(),
        records
    );
}

#[test]
fn plugins_a_running_process_keeps_its_copy_while_another_clears_the_directory() {
    let signed = Signed::new();
    signed.sign();
    let pins = signed.root.path().join("pins.toml");
    std::fs::write(&pins, signed.publisher_entry()).unwrap();
    let state = signed.root.path().join("runner");
    let plugins = signed.library.parent().unwrap();
    let runner = || {
        let mut command = ai6();
        command
            .args(["runner", "serve", "--listen", "127.0.0.1:0", "--state-dir"])
            .arg(&state)
            .arg("--plugin-path")
            .arg(plugins)
            .arg("--pins")
            .arg(&pins)
            .args(["--keep-verified", "0"])
            .stdin(std::process::Stdio::null());
        command
    };
    let original = std::fs::read(&signed.library).unwrap();
    let old_copy = verified_copy(&state, &original);

    // One process has the build mapped and is running.
    let holder = Running(
        runner()
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    until("the first runner to start", || {
        state.join("runner.json").exists().then_some(())
    });
    assert!(old_copy.exists());

    // A second finds a new build and keeps nothing else. It then stops, the state directory
    // being taken, which is no part of this.
    let mut rebuilt = original.clone();
    rebuilt.extend_from_slice(b"rebuilt");
    std::fs::write(&signed.library, &rebuilt).unwrap();
    signed.sign();
    let output = runner().output().unwrap();
    let said = text(&output.stderr);
    assert!(said.contains("another runner serves"), "{said}");
    assert!(
        said.contains(&format!(
            "old verified copy left: {}: another process holds it",
            old_copy.display()
        )),
        "{said}"
    );
    assert!(old_copy.exists(), "{said}");
    assert!(verified_copy(&state, &rebuilt).exists(), "{said}");
    drop(holder);

    // With the holder gone, the same scan clears it.
    let log = signed.root.path().join("runner.log");
    let log_file = std::fs::File::create(&log).unwrap();
    let cleared = Running(
        runner()
            .stdout(log_file.try_clone().unwrap())
            .stderr(log_file)
            .spawn()
            .unwrap(),
    );
    until("the old copy to go", || (!old_copy.exists()).then_some(()));
    assert!(verified_copy(&state, &rebuilt).exists());
    drop(cleared);
}

#[test]
fn a_named_pin_set_that_does_not_exist_stops_the_runner() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("absent-plugins.toml");
    let output = ai6()
        .args(["runner", "serve", "--state-dir"])
        .arg(directory.path().join("state"))
        .arg("--pins")
        .arg(&missing)
        .output()
        .expect("the AI6 command should start");
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("absent-plugins.toml") && error.contains("does not exist"),
        "{error}"
    );
}

#[test]
fn harness_pin_prints_the_manifest_and_its_pin() {
    let checkout = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(checkout.path().join(".claude")).unwrap();
    std::fs::write(checkout.path().join(".claude/settings.json"), "{}").unwrap();
    let output = ai6()
        .args(["harness", "pin"])
        .arg(checkout.path())
        .output()
        .expect("the AI6 command should start");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(".claude/settings.json"), "{text}");
    assert!(
        text.lines().last().unwrap().starts_with("pin = \"sha256:"),
        "{text}"
    );
}

/// The pin `ai6 assistant pin-skill` prints is the one a roster takes and a Station checks its
/// copy against: the digest of [`ai6::ProjectManifest::read_tree`].
#[test]
fn assistant_pin_skill_prints_the_pin_the_roster_takes() {
    let config = tempfile::tempdir().unwrap();
    let skill = config.path().join("skills").join("charts");
    std::fs::create_dir_all(skill.join("scripts")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: charts\ndescription: Draws charts.\n---\nDraw.\n",
    )
    .unwrap();
    std::fs::write(skill.join("scripts").join("plot.py"), "print(1)\n").unwrap();
    let output = ai6()
        .args(["assistant", "pin-skill"])
        .arg(&skill)
        .output()
        .expect("the AI6 command should start");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("  SKILL.md") && text.contains("  scripts/plot.py"),
        "{text}"
    );
    let pin = text
        .lines()
        .last()
        .and_then(|line| line.strip_prefix("pin = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap()
        .to_owned();
    assert_eq!(
        pin,
        ai6::ProjectManifest::read_tree(&skill)
            .unwrap()
            .digest()
            .to_string()
    );
    std::fs::write(
        config.path().join("agents.toml"),
        format!("[assistant.skills.charts]\npath = \"skills/charts\"\npin = \"{pin}\"\n"),
    )
    .unwrap();
    let roster = ai6::Roster::load(config.path()).unwrap();
    assert_eq!(
        roster.assistant().skills()["charts"]
            .pin()
            .unwrap()
            .to_string(),
        pin
    );

    let missing = ai6()
        .args(["assistant", "pin-skill"])
        .arg(config.path().join("missing"))
        .output()
        .expect("the AI6 command should start");
    assert!(!missing.status.success());
}

#[test]
fn a_station_record_the_station_refuses_stops_it() {
    for (station, named) in [
        ("safe_house_caches = [\"../outside\"]\n", "../outside"),
        ("stall_timeout = -1\n", "stall_timeout"),
        ("roles = [\"spy\"]\n", "station.roles"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("station.toml"), station).unwrap();
        let output = ai6()
            .args(["serve", "--listen", "127.0.0.1:0", "--config"])
            .arg(bootstrap(directory.path(), ""))
            .output()
            .expect("the AI6 command should start");
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(named), "{station}: {error}");
    }
}

#[test]
fn theater_provision_names_what_is_missing_before_contacting_anything() {
    let config = tempfile::tempdir().unwrap();
    std::fs::write(
        config.path().join("theaters.toml"),
        "[theater.box]\naccess = { ssh = { host = \"b.example\" } }\n",
    )
    .unwrap();
    let run = |theater: &str| {
        let output = ai6()
            .args(["theater", "provision", theater, "--check", "--config"])
            .arg(bootstrap(config.path(), ""))
            .output()
            .expect("the AI6 command should start");
        (
            output.status.success(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };
    let (ok, error) = run("absent");
    assert!(!ok && error.contains("declares no Theater \"absent\""));
    let (ok, error) = run("box");
    assert!(
        !ok && error.contains("no provider `ssh` is loaded"),
        "{error}"
    );
}

#[test]
fn sleepers_are_listed_authorized_and_revoked() {
    let config = tempfile::tempdir().unwrap();
    let file = bootstrap(config.path(), "");
    std::fs::write(
        config.path().join("operations.toml"),
        "[operation.G]\n[operation.G.sleepers.deps]\nschedule = \"0 3 * * *\"\ntitle = \"Update dependencies\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = ai6()
            .args(["sleeper", "--config"])
            .arg(&file)
            .args(args)
            .output()
            .expect("the AI6 command should start");
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
        )
    };

    assert!(run(&["list"]).1.contains("waiting for authorization"));
    let (ok, text) = run(&["authorize", "G", "deps"]);
    assert!(ok && text.contains("Authorized sleeper deps"), "{text}");
    assert!(run(&["list"]).1.contains("authorized by"));
    assert!(!run(&["authorize", "G", "missing"]).0);
    assert!(run(&["revoke", "G", "deps"]).1.contains("Revoked"));
    assert!(run(&["list"]).1.contains("waiting for authorization"));
}

#[test]
fn identity_and_peers_work_on_a_state_directory_with_the_source_address_shown() {
    use ai6::station::identity::{Identity, PeerId, TrustStore, TrustStoreExt};

    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let file = bootstrap(dir.path(), "");
    let run = |args: &[&str]| {
        let output = ai6()
            .args(args)
            .arg("--config")
            .arg(&file)
            .output()
            .expect("the AI6 command should start");
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
        )
    };

    let (ok, shown) = run(&["identity"]);
    assert!(ok);
    assert!(shown.starts_with("sha256:"), "{shown}");
    let own = shown.trim().parse::<PeerId>().unwrap();
    assert_eq!(run(&["identity"]).1, shown, "the identity is kept");
    assert_eq!(
        Identity::read(&state.join("identity.pem"))
            .unwrap()
            .unwrap()
            .fingerprint(),
        own
    );

    let (ok, none) = run(&["peer", "waiting"]);
    assert!(ok && none.contains("No Stations are waiting"));

    let stranger = Identity::generate().unwrap();
    let source: std::net::SocketAddr = "192.0.2.7:4000".parse().unwrap();
    TrustStore::open(&state)
        .unwrap()
        .record_waiting_certificate(stranger.certificate_der(), Some(source));
    let (ok, waiting) = run(&["peer", "waiting"]);
    assert!(ok);
    assert!(
        waiting.contains(&stranger.fingerprint().to_string()),
        "{waiting}"
    );
    assert!(waiting.contains("192.0.2.7:4000"), "{waiting}");

    // Any spelling of the fingerprint is accepted, grouped base32 included.
    let (ok, accepted) = run(&[
        "peer",
        "accept",
        &stranger.fingerprint().to_base32_grouped(),
    ]);
    assert!(ok && accepted.contains("Accepted station"), "{accepted}");
    assert!(
        run(&["peer", "waiting"])
            .1
            .contains("No Stations are waiting")
    );
    assert!(!run(&["peer", "accept", &stranger.fingerprint().to_string()]).0);
}

#[test]
fn agents_are_quarantined_listed_and_released() {
    let dir = tempfile::tempdir().unwrap();
    let file = bootstrap(
        dir.path(),
        "[registry]
location = \"registry\"
",
    );
    let run = |args: &[&str]| {
        let output = ai6()
            .arg("quarantine")
            .args(args)
            .arg("--config")
            .arg(&file)
            .output()
            .expect("the AI6 command should start");
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
        )
    };

    assert!(run(&["list"]).1.contains("No Agents are quarantined"));
    let (ok, text) = run(&["add", "007", "looping", "--operation", "G"]);
    assert!(ok && text.contains("Quarantined 007 in G"), "{text}");
    let listed = run(&["list"]).1;
    assert!(
        listed.contains("007\tG\t") && listed.contains("looping"),
        "{listed}"
    );
    assert!(
        !run(&["release", "007"]).0,
        "a release names an Operation or --all"
    );
    assert!(
        run(&["release", "007", "--all"])
            .1
            .contains("not quarantined")
    );
    assert!(
        run(&["release", "007", "--operation", "G"])
            .1
            .contains("Released 007")
    );
    assert!(run(&["list"]).1.contains("No Agents are quarantined"));
}

#[test]
fn a_detached_runner_relays_acp_until_it_stops() {
    use std::io::{BufRead, Write};
    use std::process::Stdio;

    let state = tempfile::tempdir().unwrap();
    // The settings come on standard input, as a Station starting a runner over a transport
    // sends them.
    let mut starter = ai6()
        .args([
            "runner",
            "serve",
            "--detach",
            "--settings",
            "-",
            "--state-dir",
        ])
        .arg(state.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the AI6 command should start");
    starter
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"runner": {"replay_batch": 5, "pump_poll": 0.5}}"#)
        .unwrap();
    // Only the first line is read: on Windows the runner inherits the starter's output pipe,
    // so the pipe stays open for as long as the runner runs.
    let mut line = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(starter.stdout.take().unwrap()),
        &mut line,
    )
    .unwrap();
    assert!(starter.wait().unwrap().success());
    let info: serde_json::Value = serde_json::from_str(&line).unwrap();
    let pid = info["pid"].as_u64().unwrap();
    assert!(
        info["address"].as_str().unwrap().starts_with("127.0.0.1:"),
        "{info}"
    );

    // `ai6 runner connect` authenticates for its caller, says it is ready, then relays ACP.
    let connect = || {
        ai6()
            .args(["runner", "connect", "--state-dir"])
            .arg(state.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut child = connect();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    assert_eq!(ready.trim(), "ai6-runner-connect-ready");
    let recorded = ai6::station::runner::RunnerHandoff::in_state_dir(state.path()).unwrap();
    assert_eq!(recorded.runner.replay_batch, 5);
    assert_eq!(
        recorded.runner.pump_poll,
        ai6::Seconds(std::time::Duration::from_millis(500))
    );
    let mut ask = |id: u32, method: &str, params: serde_json::Value| {
        let request =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        let mut answer = String::new();
        stdout.read_line(&mut answer).unwrap();
        serde_json::from_str::<serde_json::Value>(&answer).unwrap()
    };
    let init = ask(1, "initialize", serde_json::json!({"protocolVersion": 1}));
    assert_eq!(init["result"]["agentInfo"]["name"], "ai6-runner", "{init}");
    assert_eq!(init["result"]["authMethods"][0]["id"], "ai6-runner-token");
    // The runner's refusal is relayed, not hidden: it has no such Session.
    let missing = ask(
        2,
        "_ai6/session_status",
        serde_json::json!({"key": "nothing"}),
    );
    assert_eq!(missing["error"]["code"], -32002, "{missing}");
    drop(stdin);
    drop(stdout);
    let _ = child.wait();

    let stopped = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
    } else {
        Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
    }
    .unwrap();
    assert!(stopped.status.success());
    let mut code = Some(0);
    for _ in 0..200 {
        let mut again = connect();
        drop(again.stdin.take());
        code = again.wait().unwrap().code();
        if code != Some(0) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(code, Some(3), "a connect with no runner to answer exits 3");
}

#[test]
fn runner_settings_that_cannot_work_stop_the_runner() {
    let state = tempfile::tempdir().unwrap();
    let settings = state.path().join("given.json");
    std::fs::write(&settings, r#"{"runner": {"replay_batch": 0}}"#).unwrap();
    let output = ai6()
        .args(["runner", "serve", "--settings"])
        .arg(&settings)
        .arg("--state-dir")
        .arg(state.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runner.replay_batch"), "{stderr}");
}

/// Runs `ai6 green-ink gate` as Claude Code runs a `PreToolUse` hook: the hook's JSON on
/// standard input, no Cable endpoint in the environment.
fn gate(command: &str) -> std::process::Output {
    use std::io::Write;
    let mut child = ai6()
        .args(["green-ink", "gate"])
        .env_remove("AI6_CABLE_MCP_URL")
        .env_remove("AI6_CABLE_MCP_TOKEN")
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the AI6 command should start");
    let input = serde_json::json!({
        "session_id": "abc123",
        "cwd": std::env::temp_dir(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command, "description": "run it" },
    });
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn the_green_ink_hook_denies_a_governed_step_and_stays_silent_otherwise() {
    let held = gate("gh release create v1.2.0");
    assert!(held.status.success(), "a hook decision exits 0");
    let decision: serde_json::Value = serde_json::from_slice(&held.stdout).unwrap();
    let output = &decision["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreToolUse");
    assert_eq!(output["permissionDecision"], "deny");
    let reason = output["permissionDecisionReason"].as_str().unwrap();
    assert!(
        reason.contains("release tag v1.2.0") && reason.contains("no Station"),
        "{reason}"
    );

    let quiet = gate("cargo test --workspace");
    assert!(quiet.status.success());
    assert!(
        quiet.stdout.is_empty(),
        "nothing printed lets the call go on"
    );

    // Input the hook cannot read is denied, not let through.
    let mut child = ai6()
        .args(["green-ink", "gate", "--cable-url", "http://127.0.0.1:9/mcp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the AI6 command should start");
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"not json").unwrap();
    }
    let garbled = child.wait_with_output().unwrap();
    assert!(garbled.status.success());
    let decision: serde_json::Value = serde_json::from_slice(&garbled.stdout).unwrap();
    assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn config_import_into_sqlite_then_export_round_trips_the_examples() {
    use ai6::ConfigProvider as _;

    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config");
    let dir = tempfile::tempdir().unwrap();
    let file = bootstrap(dir.path(), "[provider]\nlocation = \"sqlite:ai6.db\"\n");
    let run = |args: &[&std::ffi::OsStr]| {
        let output = ai6()
            .arg("config")
            .args(args)
            .arg("--config")
            .arg(&file)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    let imported = run(&["import".as_ref(), examples.as_os_str()]);
    assert!(imported.starts_with("Imported "), "{imported}");
    let shown = run(&["show".as_ref(), "agent".as_ref(), "007".as_ref()]);
    assert!(shown.contains("[agent.007]"), "{shown}");
    let out = dir.path().join("exported");
    run(&["export".as_ref(), out.as_os_str()]);
    let shape = |dir: &std::path::Path| {
        let snapshot = ai6::FileProvider::new(dir).load("").unwrap();
        format!("{:?}\n{:?}", snapshot.roster, snapshot.settings)
    };
    assert_eq!(shape(&out), shape(&examples));

    let refused = ai6()
        .args(["config", "show", "agents", "--config"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(!refused.status.success());
}
