//! Conventional system and per-user configuration paths.
//!
//! Resolving paths does not create files, change directories or read documents.

use std::env;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::error::Error;

fn home() -> Result<PathBuf, Error> {
    env::home_dir().ok_or(Error::HomeUnavailable)
}

fn append(base: PathBuf, name: Option<&Path>) -> PathBuf {
    match name {
        Some(name) if !name.as_os_str().is_empty() => base.join(name),
        _ => base,
    }
}

/// Return the current user's configuration directory or a path beneath it.
///
/// On Windows, this uses `%APPDATA%`, then `~/AppData/Roaming`. On Unix and
/// macOS, it uses an absolute `$XDG_CONFIG_HOME`, then `~/.config`.
/// A relative XDG override is ignored.
///
/// # Errors
///
/// Returns [`Error::HomeUnavailable`] if no home can be determined for a
/// platform-specific fallback.
pub fn user_config_path(name: Option<&Path>) -> Result<PathBuf, Error> {
    let base = if cfg!(windows) {
        env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| home().map(|dir| dir.join("AppData").join("Roaming")))?
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(Ok)
            .unwrap_or_else(|| home().map(|dir| dir.join(".config")))?
    };
    Ok(append(base, name))
}

/// Return the machine-wide configuration directory or a path beneath it.
///
/// Windows uses `%PROGRAMDATA%`, falling back to `C:\\ProgramData`.
/// Other platforms use `/etc` (not `$XDG_CONFIG_DIRS`).
#[must_use]
pub fn system_config_path(name: Option<&Path>) -> PathBuf {
    let base = if cfg!(windows) {
        env::var_os("PROGRAMDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
    } else {
        PathBuf::from("/etc")
    };
    append(base, name)
}

/// Find existing system and user configuration files, in overriding order.
///
/// Returns `[system, user]`; each entry is `None` if the file is absent or
/// points to a directory. The returned paths can be converted to
/// [`crate::Source::path`] and passed to [`crate::Loader::load`]. No paths are
/// opened beyond the `is_file` checks. Missing files are not errors.
///
/// `name` must be a nonempty relative filename, optionally with subdirectories.
/// An absolute path or an escaping `..` component is rejected.
///
/// # Errors
///
/// Rejects an invalid name or an unavailable home directory.
pub fn default_configs(name: impl AsRef<Path>) -> Result<[Option<PathBuf>; 2], Error> {
    let name = name.as_ref();
    check_name(name)?;
    let system = system_config_path(Some(name));
    let user = user_config_path(Some(name))?;
    Ok([
        system.is_file().then_some(system),
        user.is_file().then_some(user),
    ])
}

fn check_name(name: &Path) -> Result<(), Error> {
    if name.as_os_str().is_empty()
        || name.is_absolute()
        || name
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::InvalidConfigName);
    }
    Ok(())
}

/// Finds the one configuration file a program reads: the first that exists of an explicit path,
/// the path a named environment variable holds, the user's file and the system's file.
///
/// The working directory is never searched. A relative path the caller or the variable names is
/// taken as named, against the working directory, because the person chose it.
///
/// ```no_run
/// use anycfg::Search;
/// use std::path::Path;
///
/// let flag: Option<&Path> = None;
/// let found = Search::new("ai6/ai6.cfg").explicit(flag).variable("AI6_CONFIG").find()?;
/// # Ok::<(), anycfg::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct Search {
    name: PathBuf,
    explicit: Option<PathBuf>,
    variable: Option<String>,
}

impl Search {
    /// A search for `name` beneath the [user's](user_config_path) and the
    /// [system's](system_config_path) configuration directories.
    ///
    /// `name` is relative and may hold subdirectories; [`Search::find`] refuses any other.
    pub fn new(name: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            explicit: None,
            variable: None,
        }
    }

    /// The path a person gave, such as a `--config` flag's. `None` leaves it to the rest.
    pub fn explicit(mut self, path: Option<&Path>) -> Self {
        self.explicit = path.map(Path::to_path_buf);
        self
    }

    /// The environment variable that may name the file. Set but empty counts as unset.
    pub fn variable(mut self, name: impl Into<String>) -> Self {
        self.variable = Some(name.into());
        self
    }

    /// The first file found, or `None` when no place holds one: nothing there is not a failure.
    ///
    /// Reads the variable and checks for each file; opens nothing. A place whose directory
    /// cannot be determined (no home directory, say) is skipped.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] naming the path when the explicit path or the variable's is not a
    /// file, whatever else exists. [`Error::InvalidConfigName`] for a `name` that is empty,
    /// absolute or climbs out of the directories.
    pub fn find(&self) -> Result<Option<PathBuf>, Error> {
        check_name(&self.name)?;
        let user = user_config_path(Some(&self.name)).ok();
        let system = system_config_path(Some(&self.name));
        self.find_among(|variable| env::var_os(variable), [user, Some(system)])
    }

    fn find_among(
        &self,
        read_variable: impl Fn(&str) -> Option<OsString>,
        places: [Option<PathBuf>; 2],
    ) -> Result<Option<PathBuf>, Error> {
        if let Some(path) = &self.explicit {
            return must_exist(path, None).map(Some);
        }
        if let Some(variable) = &self.variable
            && let Some(value) = read_variable(variable).filter(|value| !value.is_empty())
        {
            return must_exist(Path::new(&value), Some(variable)).map(Some);
        }
        Ok(places.into_iter().flatten().find(|path| path.is_file()))
    }
}

fn must_exist(path: &Path, variable: Option<&str>) -> Result<PathBuf, Error> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    Err(Error::NotFound {
        path: path.to_path_buf(),
        variable: variable.map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn the_users_file_is_found_before_the_systems_and_the_systems_when_alone() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.cfg");
        let system = directory.path().join("system.cfg");
        let places = || [Some(user.clone()), Some(system.clone())];
        let search = Search::new("app.cfg");
        let no_variable = |_: &str| None;

        assert_eq!(search.find_among(no_variable, places()).unwrap(), None);
        fs::write(&system, "").unwrap();
        assert_eq!(
            search.find_among(no_variable, places()).unwrap(),
            Some(system.clone())
        );
        fs::write(&user, "").unwrap();
        assert_eq!(
            search.find_among(no_variable, places()).unwrap(),
            Some(user.clone())
        );
    }

    #[test]
    fn a_place_with_no_known_directory_is_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let system = directory.path().join("system.cfg");
        fs::write(&system, "").unwrap();
        let found = Search::new("app.cfg")
            .find_among(|_| None, [None, Some(system.clone())])
            .unwrap();
        assert_eq!(found, Some(system));
    }
}
