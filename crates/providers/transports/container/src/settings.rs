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
    /// Milliseconds the output readers get to finish once a command has ended; a descendant
    /// that outlives it can hold a pipe open indefinitely. Default 1000.
    #[serde(default)]
    pub reader_grace_ms: Option<u64>,
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

    pub(crate) fn reader_grace(&self) -> std::time::Duration {
        std::time::Duration::from_millis(
            self.reader_grace_ms
                .unwrap_or(ai6::directorates::q::transport::kit::DEFAULT_READER_GRACE_MS),
        )
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

    #[test]
    fn the_reader_grace_is_a_setting() {
        assert_eq!(
            Settings::parse(
                "container = \"c\"
"
            )
            .unwrap()
            .reader_grace(),
            std::time::Duration::from_secs(1)
        );
        let settings = Settings::parse(
            "container = \"c\"
reader_grace_ms = 20
",
        )
        .unwrap();
        assert_eq!(
            settings.reader_grace(),
            std::time::Duration::from_millis(20)
        );
    }
}
