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
}
