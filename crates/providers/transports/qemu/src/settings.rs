use serde::Deserialize;

/// A Theater's QEMU guest-agent settings. Unknown keys are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    /// `host:port` of the guest agent's TCP chardev (`-chardev socket,host=...,port=...`).
    #[serde(default)]
    pub address: Option<String>,
    /// Path of the guest agent's Unix socket chardev (`-chardev socket,path=...`); Unix only.
    #[serde(default)]
    pub socket: Option<String>,
    /// Seconds to wait for one reply from the agent; 30 when absent.
    #[serde(default)]
    pub reply_timeout_secs: Option<u64>,
}

impl Settings {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let settings: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        match (&settings.address, &settings.socket) {
            (Some(_), Some(_)) | (None, None) => {
                return Err("set exactly one of `address` and `socket`".to_owned());
            }
            (Some(address), None) => {
                let ok = address
                    .rsplit_once(':')
                    .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok());
                if !ok {
                    return Err("address must be host:port".to_owned());
                }
            }
            (None, Some(socket)) => {
                if socket.is_empty() {
                    return Err("socket must not be empty".to_owned());
                }
                if cfg!(not(unix)) {
                    return Err("socket needs a Unix host; use `address` here".to_owned());
                }
            }
        }
        if settings.reply_timeout_secs == Some(0) {
            return Err("reply_timeout_secs must be at least 1".to_owned());
        }
        Ok(settings)
    }

    pub(crate) fn reply_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.reply_timeout_secs.unwrap_or(30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_endpoint_is_needed() {
        assert!(Settings::parse("address = \"127.0.0.1:4555\"\n").is_ok());
        for bad in [
            "",
            "address = \"127.0.0.1:4555\"\nsocket = \"/run/qga.sock\"\n",
            "address = \"nope\"\n",
            "address = \"h:0x\"\n",
            "address = \"127.0.0.1:1\"\nreply_timeout_secs = 0\n",
            "address = \"127.0.0.1:1\"\nunknown = 1\n",
        ] {
            assert!(Settings::parse(bad).is_err(), "{bad}");
        }
    }
}
