#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

extern crate alloc;

mod apply;
mod catalog;
mod codecs;
mod compose;
mod defaults;
mod digest;
mod error;
mod front_matter;
mod fs;
mod hook_env;
mod hooks;
mod kinds;
mod limits;
mod names;
mod overlay;
mod read;
mod skills;
mod source;
mod subst;
mod template;
mod validate;
#[cfg(feature = "test-support")]
pub mod vectors;

pub use apply::{
    Change, ComposedEnvironment, Effective, Environment, Facts, Needs, NonDefaultSetting, Phase,
    Preparation, ScriptRun, SetupRecord, SetupRun, composed_environment, effective, preparation,
    read_dotenv, setup_phase,
};
pub use catalog::{
    Catalog, CatalogEntry, CatalogForm, CatalogSection, EntryRefusal, Icon, IconFormat,
    ListedEntry, Listing, catalog_format, list_catalogs, read_catalog, read_catalog_document,
    read_icon,
};
pub use compose::{
    Composition, Cycle, Environments, LeftEnvironment, PackageConflict, PackageEnvironment,
    Resolution, compose, composition_order, environments, resolve,
};
pub use defaults::{
    DEFAULT_KEYS, Dependency, INTERPRETERS, OverlayTree, PartialLock, SkillEntry, SkillOrigin,
    bundled_skills, check_requirements, context_ignored, context_of, default_on, default_paths,
    dependencies_of, env_files_of, harness_context_of, paths_of, rules_of, skill_entries,
    skills_of,
};
pub use digest::{Digest, TreeDigest, TreeEntry, digest_entries, read_tree, tree_digest};
pub use error::{Class, Error};
pub use front_matter::{
    Case, FrontMatter, Split, SplitCase, context_file_text, read_front_matter, split_front_matter,
};
#[cfg(feature = "fs")]
pub use fs::StdFs;
pub use fs::{FileKind, FileSystem, Metadata};
pub use hook_env::{EnvSet, OverlayProcess, hook_environment, hook_order, process_environment};
pub use hooks::{
    Decision, FiredEvent, Hook, HookEvent, HookFacts, HookFailure, HookOutput, HookPlan,
    HookReport, HookRun, HookScope, OnFailure, fire_event, fire_event_after, fire_event_for_call,
    hook_matches, hook_payload, hook_plan, hook_result, hooks_declared, hooks_of, read_hook_output,
};
pub use kinds::{FailStep, Format, LeftState, ManifestFile, NonDefaultId, Runtime, Step};
pub use limits::{Limit, NonDefault, Settings};
pub use names::{
    EXCLUDED_COMPONENTS, current_os, is_reserved_variable, is_sensitive_variable, is_variable_name,
    normalize_name, os_matches, root_variable, state_variable, valid_overlay_name,
    valid_relative_path,
};
pub use overlay::{OverlayRead, read_overlay};
pub use read::{
    Document, DotenvLine, ScriptOutput, parse_dotenv, read_document, read_env_script_output,
};
pub use serde_json;
pub use skills::{
    Fetch, FetchedPlanned, FetchedSkill, PlannedOrigin, PlannedSkill, SkillPlan, SkillSource,
    fetch_planned, read_fetched_skill, resolve_skills,
};
pub use source::{Source, SourceKind};
pub use subst::{Lookup, substitute, substitute_with};
pub use template::{Template, TemplateFacts, context_text, skill_text, template, template_values};
pub use validate::{Issue, IssueCode, applies, named_files, skill_flags, valid_license, validate};
