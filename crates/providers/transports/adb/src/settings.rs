use serde::Deserialize;

/// A Theater's adb settings. Unknown keys are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    /// The device, as `adb devices` lists it. When absent adb picks the only device, and
    /// refuses if there are several.
    #[serde(default)]
    pub serial: Option<String>,
    /// The adb executable; `adb` when absent.
    #[serde(default)]
    pub executable: Option<String>,
    /// Milliseconds the output readers get to finish once a command has ended; a descendant
    /// that outlives it can hold a pipe open indefinitely. Default 1000.
    #[serde(default)]
    pub reader_grace_ms: Option<u64>,
}

impl Settings {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let settings: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        // A leading '-' would be read by adb as an option.
        for (name, value) in [
            ("serial", &settings.serial),
            ("executable", &settings.executable),
        ] {
            if let Some(value) = value
                && (value.is_empty() || value.starts_with('-') || value.contains('\0'))
            {
                return Err(format!(
                    "{name} must be a non-empty word not starting with '-'"
                ));
            }
        }
        Ok(settings)
    }

    pub(crate) fn reader_grace(&self) -> std::time::Duration {
        std::time::Duration::from_millis(
            self.reader_grace_ms
                .unwrap_or(ai6::directorates::q::transport::kit::DEFAULT_READER_GRACE_MS),
        )
    }

    pub(crate) fn executable(&self) -> String {
        self.executable.clone().unwrap_or_else(|| "adb".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let settings = Settings::parse("").unwrap();
        assert_eq!(settings.executable(), "adb");
        assert!(settings.serial.is_none());
        let settings = Settings::parse("serial = \"192.168.240.112:5555\"\n").unwrap();
        assert_eq!(settings.serial.as_deref(), Some("192.168.240.112:5555"));
        for bad in [
            "serial = \"\"\n",
            "serial = \"-s\"\n",
            "executable = \"-x\"\n",
            "unknown = 1\n",
        ] {
            assert!(Settings::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_reader_grace_is_a_setting() {
        assert_eq!(
            Settings::parse("").unwrap().reader_grace(),
            std::time::Duration::from_secs(1)
        );
        let settings = Settings::parse("reader_grace_ms = 20\n").unwrap();
        assert_eq!(
            settings.reader_grace(),
            std::time::Duration::from_millis(20)
        );
    }
}
