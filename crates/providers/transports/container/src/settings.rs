use serde::Deserialize;

/// A Theater's container settings. Unknown keys are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    /// The running container, by name or ID.
    pub container: String,
    /// The engine's command line client: `docker` when absent; `podman` works the same.
    #[serde(default)]
    pub engine: Option<String>,
    /// The user to run commands as inside the container (`-u`); the container's own when
    /// absent.
    #[serde(default)]
    pub user: Option<String>,
}

impl Settings {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let settings: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        // A leading '-' would be read by the engine as an option.
        for (name, value) in [
            ("container", Some(&settings.container)),
            ("engine", settings.engine.as_ref()),
            ("user", settings.user.as_ref()),
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

    pub(crate) fn engine(&self) -> String {
        self.engine.clone().unwrap_or_else(|| "docker".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let settings = Settings::parse("container = \"build\"\n").unwrap();
        assert_eq!(settings.engine(), "docker");
        assert!(settings.user.is_none());
        let settings =
            Settings::parse("container = \"c\"\nengine = \"podman\"\nuser = \"1000\"\n").unwrap();
        assert_eq!(settings.engine(), "podman");
        for bad in [
            "",
            "container = \"\"\n",
            "container = \"-v\"\n",
            "container = \"c\"\nengine = \"-x\"\n",
            "container = \"c\"\nunknown = 1\n",
        ] {
            assert!(Settings::parse(bad).is_err(), "{bad}");
        }
    }
}
