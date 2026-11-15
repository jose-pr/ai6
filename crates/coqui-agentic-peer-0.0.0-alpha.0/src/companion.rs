//! The native companion: what a browser extension cannot do itself, over native messaging. It answers one question, who is
//! around, and carries no agent traffic. The browser checks which extensions may start it, against the manifest
//! [`install`] writes. See <https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging>.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};

use crate::Error;
use crate::discovery::{self, Candidate};

/// A message from the browser. All it ever says is short, so anything longer is not a browser speaking this.
pub const MAX_REQUEST: usize = 64 * 1024;
/// The browser's limit on one message to it.
pub const MAX_REPLY: usize = 1024 * 1024;
/// How long a search may be asked to take.
const MAX_SEARCH: Duration = Duration::from_secs(10);
const DEFAULT_SEARCH: Duration = Duration::from_secs(3);

/// How a browser starts a native host: its first argument is the caller's origin, so the host cannot be given a subcommand.
#[must_use]
pub fn started_by_browser(first_argument: &str) -> bool {
    first_argument.starts_with("chrome-extension://")
}

/// One message: a 32-bit length in native byte order, then that much JSON. `None` when the browser has gone.
///
/// # Errors
/// When it is longer than [`MAX_REQUEST`], or the stream fails or ends inside it.
pub fn read_message(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    read_message_up_to(input, MAX_REQUEST)
}

/// [`read_message`] for a host that takes longer messages than a question: `limit` bytes at most, checked before anything is
/// allocated.
///
/// # Errors
/// When it is longer than `limit`, or the stream fails or ends inside it.
pub fn read_message_up_to(input: &mut impl Read, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0_u8; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = usize::try_from(u32::from_ne_bytes(length)).unwrap_or(usize::MAX);
    if length > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a message is too long",
        ));
    }
    let mut message = vec![0; length];
    input.read_exact(&mut message)?;
    Ok(Some(message))
}

/// One message, framed.
///
/// # Errors
/// When it is longer than [`MAX_REPLY`], or the stream fails.
pub fn write_message(output: &mut impl Write, message: &[u8]) -> io::Result<()> {
    if message.len() > MAX_REPLY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a message is too long",
        ));
    }
    let length = u32::try_from(message.len()).map_err(|_| io::ErrorKind::InvalidData)?;
    output.write_all(&length.to_ne_bytes())?;
    output.write_all(message)?;
    output.flush()
}

/// Each field of a candidate is bounded, so [`discovery::MAX_CANDIDATES`] of them fit in one message.
fn reply_with(candidates: &[Candidate]) -> Vec<u8> {
    let list: Vec<Value> = candidates
        .iter()
        .take(discovery::MAX_CANDIDATES)
        .map(Candidate::to_json)
        .collect();
    json!({ "candidates": list }).to_string().into_bytes()
}

fn answer(request: &[u8], search: &dyn Fn(Duration) -> Vec<Candidate>) -> Vec<u8> {
    let Ok(request) = serde_json::from_slice::<Value>(request) else {
        return json!({ "error": "not JSON" }).to_string().into_bytes();
    };
    if request.get("op").and_then(Value::as_str) != Some("discover") {
        return json!({ "error": "unknown op" }).to_string().into_bytes();
    }
    let wait = request
        .get("seconds")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_SEARCH, Duration::from_secs)
        .min(MAX_SEARCH);
    reply_with(&search(wait))
}

/// Serves until the browser closes the stream.
///
/// # Errors
/// When a stream fails or a message is too long, which ends the host: the browser reports that to the extension.
pub fn run(input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    while let Some(request) = read_message(input)? {
        write_message(output, &answer(&request, &discovery::discover))?;
    }
    Ok(())
}

fn valid_extension_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
}

/// What Chrome accepts as a native host's name: lowercase letters, digits, underscores and single dots between them.
fn valid_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.')
        })
}

/// A Chromium browser whose native host registrations [`install`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Browser {
    /// Google Chrome, and Chrome for Testing where it reads the same place.
    Chrome,
    /// Chromium.
    Chromium,
    /// Microsoft Edge.
    Edge,
}

impl Browser {
    /// Every browser, in the order [`install`] writes them.
    pub const ALL: [Self; 3] = [Self::Chrome, Self::Chromium, Self::Edge];

    /// The name the command line spells it by.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Edge => "edge",
        }
    }

    const fn windows_key(self) -> &'static str {
        match self {
            Self::Chrome => r"HKCU\Software\Google\Chrome\NativeMessagingHosts",
            Self::Chromium => r"HKCU\Software\Chromium\NativeMessagingHosts",
            Self::Edge => r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts",
        }
    }

    const fn unix_profile(self) -> &'static str {
        match (self, cfg!(target_os = "macos")) {
            (Self::Chrome, true) => "Library/Application Support/Google/Chrome",
            (Self::Chromium, true) => "Library/Application Support/Chromium",
            (Self::Edge, true) => "Library/Application Support/Microsoft Edge",
            (Self::Chrome, false) => ".config/google-chrome",
            (Self::Chromium, false) => ".config/chromium",
            (Self::Edge, false) => ".config/microsoft-edge",
        }
    }
}

/// A native messaging host to register: its name, the program the browser starts, and who may start it.
#[derive(Clone, Debug)]
#[must_use]
pub struct Registration {
    name: String,
    description: String,
    program: PathBuf,
    extensions: Vec<String>,
    browsers: Vec<Browser>,
    manifest_dir: Option<PathBuf>,
    profiles: Vec<PathBuf>,
}

impl Registration {
    /// A registration of `program` as the native host `name` (what an extension passes to `connectNative`), for every browser.
    ///
    /// # Errors
    /// [`Error::Discovery`] when `name` is not one Chrome accepts.
    pub fn new(name: &str, description: &str, program: impl Into<PathBuf>) -> Result<Self, Error> {
        if !valid_host_name(name) {
            return Err(Error::Discovery(
                "a native host's name is lowercase letters, digits, underscores and dots"
                    .to_owned(),
            ));
        }
        Ok(Self {
            name: name.to_owned(),
            description: description.to_owned(),
            program: program.into(),
            extensions: Vec::new(),
            browsers: Browser::ALL.to_vec(),
            manifest_dir: None,
            profiles: Vec::new(),
        })
    }

    /// The extensions that may start it, by id.
    pub fn extensions(mut self, ids: impl IntoIterator<Item = String>) -> Self {
        self.extensions = ids.into_iter().collect();
        self
    }

    /// Only these browsers; none given means every one.
    pub fn browsers(mut self, browsers: &[Browser]) -> Self {
        self.browsers = if browsers.is_empty() {
            Browser::ALL.to_vec()
        } else {
            browsers.to_vec()
        };
        self
    }

    /// Where the manifest file is kept on Windows, which names it in the registry: a directory of the program's own. Without it
    /// the library's own folder under the local application data is used.
    pub fn manifest_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.manifest_dir = Some(dir.into());
        self
    }

    /// A browser's user-data directory to register in too, as `DIR/NativeMessagingHosts`: where Chromium on Linux and macOS
    /// looks when it is started with `--user-data-dir`. Created if absent.
    pub fn profile(mut self, user_data_dir: impl Into<PathBuf>) -> Self {
        self.profiles.push(user_data_dir.into());
        self
    }
}

fn manifest(registration: &Registration) -> Value {
    let origins: Vec<String> = registration
        .extensions
        .iter()
        .map(|id| format!("chrome-extension://{id}/"))
        .collect();
    json!({
        "name": registration.name,
        "description": registration.description,
        "path": registration.program,
        "type": "stdio",
        "allowed_origins": origins,
    })
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// The per-user directories native host manifests go in, for the browsers that have a profile here.
fn unix_directories(browsers: &[Browser]) -> Vec<PathBuf> {
    let Some(home) = home() else {
        return Vec::new();
    };
    browsers
        .iter()
        .map(|browser| home.join(browser.unix_profile()))
        .filter(|profile| profile.is_dir())
        .map(|profile| profile.join("NativeMessagingHosts"))
        .collect()
}

fn windows_manifest(name: &str, dir: Option<&Path>) -> Result<PathBuf, Error> {
    let base = match dir {
        Some(dir) => dir.to_owned(),
        None => PathBuf::from(
            std::env::var_os("LOCALAPPDATA")
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::Discovery("LOCALAPPDATA is not set".to_owned()))?,
        )
        .join("coqui-acp")
        .join("native"),
    };
    Ok(base.join(format!("{name}.json")))
}

fn reg(arguments: &[&str]) -> Result<bool, Error> {
    let output = Command::new("reg")
        .args(arguments)
        .output()
        .map_err(|error| Error::Discovery(format!("reg could not run: {error}")))?;
    Ok(output.status.success())
}

/// Lets the registration's extensions start its program as a native host, for this user. Returns what it wrote: the manifest
/// and, on Windows, each registry key.
///
/// # Errors
/// [`Error::Discovery`] for a malformed extension id, no browser profile to install into, or a file or the registry refusing.
pub fn install(registration: &Registration) -> Result<Vec<String>, Error> {
    let extensions = &registration.extensions;
    if extensions.is_empty() || !extensions.iter().all(|id| valid_extension_id(id)) {
        return Err(Error::Discovery(
            "an extension id is 32 letters from a to p".to_owned(),
        ));
    }
    let fail =
        |error: io::Error| Error::Discovery(format!("the manifest could not be written: {error}"));
    let text = serde_json::to_string_pretty(&manifest(registration))
        .map_err(|error| Error::Discovery(error.to_string()))?;
    let mut wrote = Vec::new();
    let name = &registration.name;
    if cfg!(windows) {
        let path = windows_manifest(name, registration.manifest_dir.as_deref())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(fail)?;
        }
        std::fs::write(&path, &text).map_err(fail)?;
        let value = path.to_string_lossy().into_owned();
        wrote.push(value.clone());
        for browser in &registration.browsers {
            let key = format!(r"{}\{name}", browser.windows_key());
            if !reg(&["add", &key, "/ve", "/t", "REG_SZ", "/d", &value, "/f"])? {
                return Err(Error::Discovery(format!("the registry refused {key}")));
            }
            wrote.push(key);
        }
    } else {
        let mut directories = unix_directories(&registration.browsers);
        directories.extend(
            registration
                .profiles
                .iter()
                .map(|profile| profile.join("NativeMessagingHosts")),
        );
        if directories.is_empty() {
            return Err(Error::Discovery(
                "no Chrome, Chromium or Edge profile for this user".to_owned(),
            ));
        }
        for directory in directories {
            std::fs::create_dir_all(&directory).map_err(fail)?;
            let path = directory.join(format!("{name}.json"));
            std::fs::write(&path, &text).map_err(fail)?;
            wrote.push(path.to_string_lossy().into_owned());
        }
    }
    Ok(wrote)
}

/// Removes what [`install`] wrote for `name`, from every browser and every given profile. What is already gone is not a failure.
///
/// # Errors
/// [`Error::Discovery`] when a file exists and cannot be removed, or `reg` cannot run.
pub fn uninstall(
    name: &str,
    manifest_dir: Option<&Path>,
    profiles: &[PathBuf],
) -> Result<(), Error> {
    if !valid_host_name(name) {
        return Err(Error::Discovery(
            "a native host's name is lowercase letters, digits, underscores and dots".to_owned(),
        ));
    }
    let remove = |path: &Path| match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(Error::Discovery(format!(
            "the manifest could not be removed: {error}"
        ))),
        _ => Ok(()),
    };
    if cfg!(windows) {
        for browser in Browser::ALL {
            let key = format!(r"{}\{name}", browser.windows_key());
            let _ = reg(&["delete", &key, "/f"])?;
        }
        remove(&windows_manifest(name, manifest_dir)?)
    } else {
        for directory in unix_directories(&Browser::ALL) {
            remove(&directory.join(format!("{name}.json")))?;
        }
        for profile in profiles {
            remove(
                &profile
                    .join("NativeMessagingHosts")
                    .join(format!("{name}.json")),
            )?;
        }
        Ok(())
    }
}

/// Where `name` is registered now: each manifest file or registry key that exists, with what it points at.
///
/// # Errors
/// [`Error::Discovery`] when `reg` cannot run.
pub fn installed(
    name: &str,
    manifest_dir: Option<&Path>,
    profiles: &[PathBuf],
) -> Result<Vec<String>, Error> {
    let mut found = Vec::new();
    if cfg!(windows) {
        for browser in Browser::ALL {
            let key = format!(r"{}\{name}", browser.windows_key());
            if reg(&["query", &key])? {
                found.push(key);
            }
        }
        let path = windows_manifest(name, manifest_dir)?;
        if path.is_file() {
            found.push(path.to_string_lossy().into_owned());
        }
    } else {
        let mut files: Vec<PathBuf> = unix_directories(&Browser::ALL)
            .into_iter()
            .map(|directory| directory.join(format!("{name}.json")))
            .collect();
        files.extend(profiles.iter().map(|profile| {
            profile
                .join("NativeMessagingHosts")
                .join(format!("{name}.json"))
        }));
        found.extend(
            files
                .into_iter()
                .filter(|file| file.is_file())
                .map(|file| file.to_string_lossy().into_owned()),
        );
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn framed(message: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write_message(&mut out, message).unwrap();
        out
    }

    #[test]
    fn messages_round_trip_and_the_end_is_no_message() {
        let mut input = Cursor::new(framed(b"{}"));
        assert_eq!(read_message(&mut input).unwrap().unwrap(), b"{}");
        assert!(read_message(&mut input).unwrap().is_none());
    }

    #[test]
    fn a_length_past_the_limit_is_refused_before_anything_is_allocated() {
        let mut input = Cursor::new(u32::MAX.to_ne_bytes().to_vec());
        assert!(read_message(&mut input).is_err());
        let over = u32::try_from(MAX_REQUEST + 1).unwrap();
        let mut input = Cursor::new(over.to_ne_bytes().to_vec());
        assert!(read_message(&mut input).is_err());
    }

    #[test]
    fn a_message_cut_short_is_an_error_not_a_message() {
        let mut bytes = framed(b"{\"op\":\"discover\"}");
        bytes.truncate(8);
        assert!(read_message(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn it_answers_only_discover_and_bounds_the_wait() {
        let asked = std::cell::Cell::new(Duration::ZERO);
        let search = |wait: Duration| {
            asked.set(wait);
            Vec::new()
        };
        let reply: Value =
            serde_json::from_slice(&answer(br#"{"op":"discover","seconds":3600}"#, &search))
                .unwrap();
        assert_eq!(reply["candidates"], json!([]));
        assert_eq!(asked.get(), MAX_SEARCH);
        let reply: Value =
            serde_json::from_slice(&answer(br#"{"op":"run","program":"x"}"#, &search)).unwrap();
        assert_eq!(reply["error"], "unknown op");
        let reply: Value = serde_json::from_slice(&answer(b"nope", &search)).unwrap();
        assert_eq!(reply["error"], "not JSON");
    }

    #[test]
    fn a_reply_always_fits_what_the_browser_takes() {
        // The longest of each field a record can give, in the characters JSON writes longest.
        let big = Candidate {
            label: "\"".repeat(63),
            url: format!("wss://{}:65535/{}", "h".repeat(253), "%".repeat(127)),
            peer: Some(format!("sha256:{}", "0".repeat(64))),
            method: crate::auth::Method::WebCrypto,
            source: discovery::Source::Network,
            webtransport: Some(65535),
            webtransport_hash: Some(format!("sha256:{}", "0".repeat(64))),
            acp_versions: vec![u32::MAX; 8],
            offers: vec!["x".repeat(64); 16],
        };
        let reply = reply_with(&vec![big; discovery::MAX_CANDIDATES + 1]);
        assert!(reply.len() <= MAX_REPLY, "{}", reply.len());
        let reply: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(
            reply["candidates"].as_array().unwrap().len(),
            discovery::MAX_CANDIDATES
        );
    }

    #[test]
    fn extension_ids_are_checked_and_the_browser_is_recognised() {
        assert!(valid_extension_id(&"a".repeat(32)));
        assert!(!valid_extension_id(&"z".repeat(32)));
        assert!(!valid_extension_id("abc"));
        let registration = |name: &str| Registration::new(name, "x", "/bin/x");
        assert!(
            install(
                &registration("dev.example.host")
                    .unwrap()
                    .extensions(["../../x".to_owned()])
            )
            .is_err()
        );
        assert!(install(&registration("dev.example.host").unwrap()).is_err());
        assert!(started_by_browser("chrome-extension://abc/"));
        assert!(!started_by_browser("serve"));
    }
    #[test]
    fn a_longer_limit_is_the_callers_and_is_still_checked_first() {
        let big = vec![b'x'; MAX_REQUEST + 10];
        let mut input = Cursor::new(framed(&big));
        assert!(read_message(&mut input).is_err());
        let mut input = Cursor::new(framed(&big));
        assert_eq!(
            read_message_up_to(&mut input, MAX_REQUEST + 10)
                .unwrap()
                .unwrap()
                .len(),
            big.len()
        );
        let mut input = Cursor::new(u32::MAX.to_ne_bytes().to_vec());
        assert!(read_message_up_to(&mut input, 4 << 20).is_err());
    }

    #[test]
    fn a_host_name_is_one_chrome_accepts_and_the_manifest_carries_it() {
        for bad in ["", "Dev.x", ".x", "x.", "a..b", "a-b", "a b", "a/b"] {
            assert!(Registration::new(bad, "d", "/p").is_err(), "{bad}");
        }
        let registration = Registration::new("dev.example.host", "A host", "/opt/host")
            .unwrap()
            .extensions(["a".repeat(32)]);
        let manifest = manifest(&registration);
        assert_eq!(manifest["name"], "dev.example.host");
        assert_eq!(manifest["type"], "stdio");
        assert_eq!(manifest["path"], "/opt/host");
        assert_eq!(
            manifest["allowed_origins"],
            json!([format!("chrome-extension://{}/", "a".repeat(32))])
        );
        assert!(uninstall("Bad Name", None, &[]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_profile_directory_is_installed_into_and_taken_out_again() {
        let dir = std::env::temp_dir().join(format!("companion-profile-{}", std::process::id()));
        let registration = Registration::new("dev.example.profiled", "A host", "/opt/host")
            .unwrap()
            .extensions(["a".repeat(32)])
            .browsers(&[])
            .profile(&dir);
        let wrote = install(&registration).unwrap();
        let file = dir
            .join("NativeMessagingHosts")
            .join("dev.example.profiled.json");
        assert!(wrote.contains(&file.to_string_lossy().into_owned()));
        assert!(file.is_file());
        let profiles = [dir.clone()];
        assert!(
            !installed("dev.example.profiled", None, &profiles)
                .unwrap()
                .is_empty()
        );
        uninstall("dev.example.profiled", None, &profiles).unwrap();
        assert!(!file.exists());
        assert!(
            installed("dev.example.profiled", None, &profiles)
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[cfg(windows)]
    #[test]
    fn the_registry_keys_of_a_test_name_are_written_and_removed() {
        let name = format!("dev.example.test_{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("companion-manifest-{}", std::process::id()));
        let registration = Registration::new(&name, "A host", r"C:\opt\host.exe")
            .unwrap()
            .extensions(["a".repeat(32)])
            .browsers(&[Browser::Edge])
            .manifest_dir(&dir);
        let outcome = std::panic::catch_unwind(|| {
            let wrote = install(&registration).unwrap();
            assert_eq!(wrote.len(), 2, "{wrote:?}");
            assert_eq!(installed(&name, Some(&dir), &[]).unwrap().len(), 2);
        });
        uninstall(&name, Some(&dir), &[]).unwrap();
        assert!(installed(&name, Some(&dir), &[]).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        outcome.unwrap();
    }
}
