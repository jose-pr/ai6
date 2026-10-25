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
}
