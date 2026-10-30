use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

const DEFAULT_PORT: u16 = 22;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_EXIT_GRACE: Duration = Duration::from_secs(1);
const DEFAULT_KEEPALIVE_SECS: u64 = 30;
const DEFAULT_KEEPALIVE_MAX: usize = 3;

/// A Theater's SSH settings, as the Station serializes `SshAccess`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub host: String,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub host_key: Option<String>,
    #[serde(default)]
    pub trust_on_first_use: bool,
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,
    /// Milliseconds output and the end of the channel are awaited after a command has exited;
    /// a descendant that outlives it can keep the channel open indefinitely. Default 1000.
    #[serde(default)]
    pub exit_grace_ms: Option<u64>,
    /// Seconds between keepalive probes of an idle connection; `0` sends none. Default 30.
    #[serde(default)]
    pub keepalive_secs: Option<u64>,
    /// Unanswered keepalive probes before the connection is dropped. Default 3.
    #[serde(default)]
    pub keepalive_max: Option<usize>,
}

impl Settings {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let settings: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        if settings.host.is_empty() || settings.host.starts_with('-') {
            return Err("host must be a non-empty name or address".to_owned());
        }
        if settings.port == Some(0) {
            return Err("port must be 1 through 65535".to_owned());
        }
        if let Some(pin) = &settings.host_key
            && !is_pin(pin)
        {
            return Err("host_key must be \"sha256:\" and 64 hex digits".to_owned());
        }
        if settings.connect_timeout_secs == Some(0) {
            return Err("connect_timeout_secs must be at least 1".to_owned());
        }
        Ok(settings)
    }

    pub(crate) fn port(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }

    pub(crate) fn connect_timeout(&self) -> Duration {
        self.connect_timeout_secs
            .map_or(DEFAULT_CONNECT_TIMEOUT, Duration::from_secs)
    }

    pub(crate) fn exit_grace(&self) -> Duration {
        self.exit_grace_ms
            .map_or(DEFAULT_EXIT_GRACE, Duration::from_millis)
    }

    /// The keepalive interval, `None` when the setting turns it off.
    pub(crate) fn keepalive(&self) -> Option<Duration> {
        match self.keepalive_secs.unwrap_or(DEFAULT_KEEPALIVE_SECS) {
            0 => None,
            secs => Some(Duration::from_secs(secs)),
        }
    }

    pub(crate) fn keepalive_max(&self) -> usize {
        self.keepalive_max.unwrap_or(DEFAULT_KEEPALIVE_MAX)
    }

    /// The remote user: the setting, else the Station's own user name.
    pub(crate) fn user(&self) -> String {
        self.user
            .clone()
            .or_else(|| std::env::var("USER").ok())
            .or_else(|| std::env::var("USERNAME").ok())
            .unwrap_or_else(|| "root".to_owned())
    }

    /// The pinned host key, lowercased.
    pub(crate) fn pin(&self) -> Option<String> {
        self.host_key.as_ref().map(|pin| pin.to_ascii_lowercase())
    }

    /// Where the private key is: the setting, else the first default key that exists.
    pub(crate) fn identity_path(&self) -> Result<PathBuf, String> {
        if let Some(identity) = &self.identity {
            return Ok(expand_home(identity));
        }
        let home = home_dir().ok_or("no home directory to look for a default SSH key in")?;
        ["id_ed25519", "id_ecdsa", "id_rsa"]
            .iter()
            .map(|name| home.join(".ssh").join(name))
            .find(|path| path.is_file())
            .ok_or_else(|| {
                "no `identity` is set and ~/.ssh holds no id_ed25519, id_ecdsa or id_rsa key"
                    .to_owned()
            })
    }
}

fn is_pin(pin: &str) -> bool {
    pin.strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => home_dir().map_or_else(|| PathBuf::from(path), |home| home.join(rest)),
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let settings = Settings::parse("host = \"h\"\n").unwrap();
        assert_eq!(settings.port(), 22);
        assert_eq!(settings.connect_timeout(), DEFAULT_CONNECT_TIMEOUT);
        assert!(!settings.trust_on_first_use && settings.pin().is_none());
        for bad in [
            "",
            "host = \"\"\n",
            "host = \"-oProxyCommand=x\"\n",
            "host = \"h\"\nport = 0\n",
            "host = \"h\"\nhost_key = \"md5:aa\"\n",
            "host = \"h\"\nunknown = 1\n",
            "host = \"h\"\nconnect_timeout_secs = 0\n",
        ] {
            assert!(Settings::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_pin_is_compared_in_lower_case() {
        let pin = format!("sha256:{}", "AB".repeat(32));
        let settings = Settings::parse(&format!("host = \"h\"\nhost_key = \"{pin}\"\n")).unwrap();
        assert_eq!(settings.pin(), Some(pin.to_ascii_lowercase()));
    }

    #[test]
    fn a_home_relative_identity_is_expanded() {
        let settings = Settings::parse("host = \"h\"\nidentity = \"/abs/key\"\n").unwrap();
        assert_eq!(settings.identity_path().unwrap(), PathBuf::from("/abs/key"));
        let expanded = expand_home("~/k");
        assert!(expanded.ends_with("k") && expanded.as_os_str() != "~/k");
    }

    #[test]
    fn the_session_bounds_are_settings_with_defaults() {
        let settings = Settings::parse("host = \"h\"\n").unwrap();
        assert_eq!(settings.exit_grace(), Duration::from_secs(1));
        assert_eq!(settings.keepalive(), Some(Duration::from_secs(30)));
        assert_eq!(settings.keepalive_max(), 3);
        let settings = Settings::parse(
            "host = \"h\"\nexit_grace_ms = 50\nkeepalive_secs = 0\nkeepalive_max = 9\n",
        )
        .unwrap();
        assert_eq!(settings.exit_grace(), Duration::from_millis(50));
        assert_eq!(settings.keepalive(), None);
        assert_eq!(settings.keepalive_max(), 9);
    }
}
