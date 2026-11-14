//! The specification's conformance vectors, run: a [`Runner`] reads each vector file of a
//! directory the caller names and checks every case against this crate. Behind the
//! `test-support` feature, for a host that wants to prove its own build of the readers still
//! passes the vectors, adding the non-default settings it reports itself.
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

mod catalog;
mod front_matter;
mod hooks;
mod reading;
mod skills;
mod support;
mod templating;

use std::fmt;
use std::path::PathBuf;

use serde_json::Value;

use crate::{Document, Effective, Error, Facts, NonDefaultSetting, effective};

type SkipFn = Box<dyn Fn(&str, &Value) -> Option<&'static str>>;
type EffectiveFn = Box<dyn Fn(&Document, &Facts) -> Result<Effective, Error>>;

/// What running the cases of one vector file found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    /// The vector file, such as `reading.json`.
    pub file: String,
    /// Cases that passed.
    pub passed: usize,
    /// `<id>: <reason>` of each case the runner was told to skip.
    pub skipped: Vec<String>,
    /// `<id>: <what differed>` of each case that failed.
    pub failed: Vec<String>,
}

impl Report {
    /// Whether no case failed.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }

    /// Panics naming each failing case, when any failed.
    #[track_caller]
    pub fn assert_clean(&self) {
        assert!(
            self.is_clean(),
            "{}:\n{}",
            self.file,
            self.failed.join("\n")
        );
    }
}

/// Runs the specification's conformance vectors against this crate.
///
/// A host that wraps [`effective`] and adds its own non-default settings gives the wrapper to
/// [`Runner::effective`] and the settings it adds to [`Runner::host_non_defaults`]: the runner
/// takes exactly those out of a result before comparing it with the vector's, so a vector fails
/// for a setting the host did not declare.
pub struct Runner {
    dir: PathBuf,
    skip: Option<SkipFn>,
    effective: EffectiveFn,
    host: Vec<NonDefaultSetting>,
}

impl fmt::Debug for Runner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Runner")
            .field("dir", &self.dir)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

impl Runner {
    /// The vector files this crate runs.
    pub const FILES: [&'static str; 12] = [
        "catalog.json",
        "compose.json",
        "digest.json",
        "effective.json",
        "front_matter.json",
        "hooks.json",
        "names.json",
        "reading.json",
        "skills.json",
        "source.json",
        "templating.json",
        "validation.json",
    ];

    /// A runner of the vectors in `dir`, the specification's `tests/vectors/` or a copy of it.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            skip: None,
            effective: Box::new(effective),
            host: Vec::new(),
        }
    }

    /// Names the cases to leave out: `skip(file, case)` gives the reason a case is not run. Each
    /// skip is in its [`Report`].
    #[must_use]
    pub fn skip(mut self, skip: impl Fn(&str, &Value) -> Option<&'static str> + 'static) -> Self {
        self.skip = Some(Box::new(skip));
        self
    }

    /// Computes each `effective.json` case with `effective` instead of [`effective`].
    #[must_use]
    pub fn effective(
        mut self,
        effective: impl Fn(&Document, &Facts) -> Result<Effective, Error> + 'static,
    ) -> Self {
        self.effective = Box::new(effective);
        self
    }

    /// The settings the function given to [`Runner::effective`] adds to a result's
    /// `non_defaults`, each taken out once before the result is compared.
    #[must_use]
    pub fn host_non_defaults(mut self, settings: Vec<NonDefaultSetting>) -> Self {
        self.host = settings;
        self
    }

    /// The vector files in the directory this crate has no runner for: each one is a
    /// specification change nothing checks yet.
    ///
    /// # Panics
    ///
    /// When the directory cannot be read.
    #[must_use]
    pub fn unrun(&self) -> Vec<String> {
        let mut files: Vec<String> = std::fs::read_dir(&self.dir)
            .unwrap_or_else(|error| panic!("{}: {error}", self.dir.display()))
            .map(|entry| {
                entry
                    .expect("a directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| {
                name.ends_with(".json")
                    && name != "AOS-SOURCE.json"
                    && !Self::FILES.contains(&name.as_str())
            })
            .collect();
        files.sort();
        files
    }

    /// Runs one vector file of [`Runner::FILES`]; `None` for any other name.
    ///
    /// # Panics
    ///
    /// When the file cannot be read or is not a vector file.
    #[must_use]
    pub fn run_file(&self, file: &str) -> Option<Report> {
        Some(match file {
            "catalog.json" => catalog::catalogs_and_icons(self),
            "compose.json" => reading::compose_vectors(self),
            "digest.json" => reading::digest(self),
            "effective.json" => reading::effective_vectors(self),
            "front_matter.json" => front_matter::front_matter(self),
            "hooks.json" => hooks::hooks(self),
            "names.json" => reading::names(self),
            "reading.json" => reading::reading(self),
            "skills.json" => skills::skills_from_elsewhere(self),
            "source.json" => reading::source(self),
            "templating.json" => templating::templating(self),
            "validation.json" => reading::validation(self),
            _ => return None,
        })
    }

    /// Runs every vector file of [`Runner::FILES`], in that order.
    ///
    /// # Panics
    ///
    /// When a file cannot be read.
    #[must_use]
    pub fn run_all(&self) -> Vec<Report> {
        Self::FILES
            .iter()
            .filter_map(|file| self.run_file(file))
            .collect()
    }

    /// Computes `effective` for a case, the host's settings taken out of the result.
    pub(crate) fn effective_of(
        &self,
        manifest: &Document,
        facts: &Facts,
    ) -> Result<Effective, Error> {
        let mut result = (self.effective)(manifest, facts)?;
        for setting in &self.host {
            if let Some(at) = result.non_defaults.iter().position(|got| got == setting) {
                result.non_defaults.remove(at);
            }
        }
        Ok(result)
    }

    fn cases(&self, file: &str) -> Vec<Value> {
        let path = self.dir.join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let document: Value = serde_json::from_str(&text).expect("a vector file is JSON");
        document["cases"]
            .as_array()
            .expect("a vector file has cases")
            .clone()
    }

    /// Runs `check` on every case of `file`, collecting the failures.
    pub(crate) fn each(&self, file: &str, check: impl Fn(&Value) -> Result<(), String>) -> Report {
        let mut report = Report {
            file: file.to_owned(),
            ..Report::default()
        };
        for case in self.cases(file) {
            let id = case["id"].as_str().unwrap_or_default().to_owned();
            if let Some(reason) = self.skip.as_ref().and_then(|skip| skip(file, &case)) {
                report.skipped.push(format!("{id}: {reason}"));
                continue;
            }
            match check(&case) {
                Ok(()) => report.passed += 1,
                Err(why) => report.failed.push(format!("{id}: {why}")),
            }
        }
        report
    }
}
