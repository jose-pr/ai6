use serde::Deserialize;

/// A template's container settings. Unknown keys are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    /// The image each container is made from.
    pub image: String,
    /// The engine's command line client: `docker` when absent; `podman` works the same.
    #[serde(default)]
    pub engine: Option<String>,
    /// The user the `container` transport runs commands as; the image's own when absent.
    #[serde(default)]
    pub user: Option<String>,
    /// What the container runs to stay up; `["sleep", "infinity"]` when absent.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// More options for the engine's `run`, such as `["--network", "none"]` or a volume.
    #[serde(default)]
    pub run_args: Vec<String>,
    /// Milliseconds one engine command may run; pulling an image can take a while. Default
    /// 600000. `0` waits for the command.
    #[serde(default)]
    pub engine_timeout_ms: Option<u64>,
    /// Bytes of an engine command's output kept. Default 1048576 (1 MiB).
    #[serde(default)]
    pub engine_output_bytes: Option<u64>,
    /// Milliseconds the output readers get to finish once a command has ended. Default 1000.
    #[serde(default)]
    pub reader_grace_ms: Option<u64>,
}

impl Settings {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let settings: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        // A leading '-' would be read by the engine as an option.
        for (name, value) in [
            ("image", Some(&settings.image)),
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
        if settings
            .command
            .as_ref()
            .is_some_and(|command| command.is_empty())
            || settings
                .command
                .iter()
                .flatten()
                .chain(&settings.run_args)
                .any(|word| word.contains('\0'))
        {
            return Err("command must name a program, and no argument may hold a NUL".to_owned());
        }
        Ok(settings)
    }

    pub(crate) fn engine_timeout_ms(&self) -> u64 {
        self.engine_timeout_ms.unwrap_or(600_000)
    }

    pub(crate) fn engine_output_bytes(&self) -> u64 {
        self.engine_output_bytes.unwrap_or(1024 * 1024)
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

    pub(crate) fn command(&self) -> Vec<String> {
        self.command
            .clone()
            .unwrap_or_else(|| vec!["sleep".to_owned(), "infinity".to_owned()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let settings = Settings::parse("image = \"fedora:44\"\n").unwrap();
        assert_eq!(settings.engine(), "docker");
        assert_eq!(settings.command(), ["sleep", "infinity"]);
        for bad in [
            "",
            "image = \"\"\n",
            "image = \"-v\"\n",
            "image = \"i\"\ncommand = []\n",
            "image = \"i\"\nengine = \"-x\"\n",
            "image = \"i\"\nunknown = 1\n",
        ] {
            assert!(Settings::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_engine_bounds_are_settings_with_defaults() {
        let settings = Settings::parse("image = \"i\"\n").unwrap();
        assert_eq!(settings.engine_timeout_ms(), 600_000);
        assert_eq!(settings.engine_output_bytes(), 1024 * 1024);
        assert_eq!(settings.reader_grace(), std::time::Duration::from_secs(1));
        let settings = Settings::parse(
            "image = \"i\"\nengine_timeout_ms = 5\nengine_output_bytes = 7\nreader_grace_ms = 9\n",
        )
        .unwrap();
        assert_eq!(settings.engine_timeout_ms(), 5);
        assert_eq!(settings.engine_output_bytes(), 7);
        assert_eq!(settings.reader_grace(), std::time::Duration::from_millis(9));
    }
}
