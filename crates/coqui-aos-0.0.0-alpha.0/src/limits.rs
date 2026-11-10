//! [`Limit`], a count or a size that may be lifted, and [`Settings`], every limit the reader
//! applies, each a setting with a default.

use core::fmt;

use guatiao::schema::KindBuilder;
use guatiao::value::types::{Entry, Tag};
use guatiao::{Alloc, FromValue, MapError, Schema, ToValue, Value, ValueError};

/// The word a bound that is lifted is written as.
const NO_BOUND: &str = "none";
/// What the word reads as in a form.
const NO_BOUND_LABEL: &str = "No limit";

const MIB: u64 = 1024 * 1024;

/// A count or a number of bytes, or no bound at all. Written as a whole number, or `null` or
/// `"none"` for no bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Limit(Option<u64>);

impl Limit {
    /// No bound.
    pub const UNBOUNDED: Self = Self(None);

    /// A bound of `n`.
    #[must_use]
    pub const fn at(n: u64) -> Self {
        Self(Some(n))
    }

    /// The bound, `None` when there is none.
    #[must_use]
    pub const fn get(self) -> Option<u64> {
        self.0
    }

    /// Whether `n` is within the bound: at most the bound, or anything when there is none. A
    /// value that does not fit a `u64` is outside every bound.
    pub fn allows<N: TryInto<u64>>(self, n: N) -> bool {
        match (self.0, n.try_into()) {
            (None, _) => true,
            (Some(bound), Ok(n)) => n <= bound,
            (Some(_), Err(_)) => false,
        }
    }
}

impl fmt::Display for Limit {
    /// The number, or `none`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(bound) => write!(f, "{bound}"),
            None => f.write_str(NO_BOUND),
        }
    }
}

fn is_no_bound(value: &Value) -> bool {
    matches!(value.tag(), Ok(Tag::GUATIAO_NULL))
        || guatiao::value::read::str_or(Some(value), "") == NO_BOUND
}

impl ToValue for Limit {
    fn to_value_in(&self, alloc: Alloc) -> Result<Value, ValueError> {
        match self.0 {
            Some(bound) => bound.to_value_in(alloc),
            None => NO_BOUND.to_owned().to_value_in(alloc),
        }
    }
}

impl FromValue for Limit {
    fn from_value(value: &Value) -> Result<Self, MapError> {
        if is_no_bound(value) {
            return Ok(Self::UNBOUNDED);
        }
        u64::from_value(value)
            .map(Self::at)
            .map_err(|_| MapError::bad_value("a whole number, 0 or more, or \"none\" for no bound"))
    }
}

/// A whole number, 0 or more, or the word for no bound.
impl Schema for Limit {
    fn kind(alloc: Alloc) -> KindBuilder {
        KindBuilder::union_in(
            alloc,
            vec![
                KindBuilder::int_bounds_in(alloc, Some(0), None),
                KindBuilder::enumeration_in(alloc, &[(NO_BOUND, NO_BOUND_LABEL)]),
            ],
        )
    }
}

/// How much a reader reads and builds. Every limit has a default and no floor; a looser one is
/// reported by [`Settings::non_defaults`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct Settings {
    /// Bytes of a manifest, an `aos` env file or a catalog, its byte-order mark and format line
    /// included.
    #[schema(title = "Largest manifest", format = "bytes")]
    #[map(default = Settings::default().manifest_bytes)]
    pub manifest_bytes: Limit,
    /// Nesting of a document's arrays and mappings, its top mapping being the first level.
    #[schema(title = "Deepest manifest")]
    #[map(default = Settings::default().manifest_depth)]
    pub manifest_depth: Limit,
    /// Files and directories in one overlay, the excluded names not counted.
    #[schema(title = "Files and directories in an overlay")]
    #[map(default = Settings::default().tree_files)]
    pub tree_files: Limit,
    /// Levels of directories in one overlay, the overlay's own directory being the first.
    #[schema(title = "Deepest overlay directory")]
    #[map(default = Settings::default().tree_depth)]
    pub tree_depth: Limit,
    /// Bytes of one overlay's files together.
    #[schema(title = "Largest overlay", format = "bytes")]
    #[map(default = Settings::default().tree_bytes)]
    pub tree_bytes: Limit,
    /// Bytes of one file of an overlay.
    #[schema(title = "Largest overlay file", format = "bytes")]
    #[map(default = Settings::default().file_bytes)]
    pub file_bytes: Limit,
    /// Bytes of the body of an `OVERLAY.md` that is the overlay's context.
    #[schema(title = "Largest context body", format = "bytes")]
    #[map(default = Settings::default().context_bytes)]
    pub context_bytes: Limit,
    /// Bytes of a catalog file.
    #[schema(title = "Largest catalog", format = "bytes")]
    #[map(default = Settings::default().catalog_bytes)]
    pub catalog_bytes: Limit,
    /// Bytes of an icon file.
    #[schema(title = "Largest icon", format = "bytes")]
    #[map(default = Settings::default().icon_bytes)]
    pub icon_bytes: Limit,
    /// Width of an icon in pixels.
    #[schema(title = "Widest icon")]
    #[map(default = Settings::default().icon_width)]
    pub icon_width: Limit,
    /// Height of an icon in pixels.
    #[schema(title = "Tallest icon")]
    #[map(default = Settings::default().icon_height)]
    pub icon_height: Limit,
    /// Seconds the consumer lets one hook run, whatever its entry asks.
    #[schema(title = "Longest hook run")]
    #[map(default = Settings::default().hook_timeout)]
    pub hook_timeout: Limit,
    /// Seconds the hooks of one event may take together.
    #[schema(title = "Longest event of hooks")]
    #[map(default = Settings::default().hook_event_timeout)]
    pub hook_event_timeout: Limit,
    /// Bytes of a hook's output.
    #[schema(title = "Largest hook output", format = "bytes")]
    #[map(default = Settings::default().hook_output_bytes)]
    pub hook_output_bytes: Limit,
    /// Bytes of the payload a hook is sent.
    #[schema(title = "Largest hook payload", format = "bytes")]
    #[map(default = Settings::default().hook_payload_bytes)]
    pub hook_payload_bytes: Limit,
    /// Characters of the reason a hook gives.
    #[schema(title = "Longest hook reason")]
    #[map(default = Settings::default().hook_reason_chars)]
    pub hook_reason_chars: Limit,
    /// Bytes of one substituted value.
    #[schema(title = "Largest substituted value", format = "bytes")]
    #[map(default = Settings::default().substitution_bytes)]
    pub substitution_bytes: Limit,
    /// Nesting of `${…}` expressions within one value.
    #[schema(title = "Deepest substitution")]
    #[map(default = Settings::default().substitution_depth)]
    pub substitution_depth: Limit,
    /// Steps one substitution may take.
    #[schema(title = "Longest substitution")]
    #[map(default = Settings::default().substitution_steps)]
    pub substitution_steps: Limit,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            manifest_bytes: Limit::at(MIB),
            manifest_depth: Limit::at(64),
            tree_files: Limit::at(4096),
            tree_depth: Limit::at(64),
            tree_bytes: Limit::at(64 * MIB),
            file_bytes: Limit::at(16 * MIB),
            context_bytes: Limit::at(MIB),
            catalog_bytes: Limit::at(4 * MIB),
            icon_bytes: Limit::at(512 * 1024),
            icon_width: Limit::at(1024),
            icon_height: Limit::at(1024),
            hook_timeout: Limit::at(60),
            hook_event_timeout: Limit::at(120),
            hook_output_bytes: Limit::at(64 * 1024),
            hook_payload_bytes: Limit::at(MIB),
            hook_reason_chars: Limit::at(1024),
            substitution_bytes: Limit::at(MIB),
            substitution_depth: Limit::at(32),
            substitution_steps: Limit::at(10_000_000),
        }
    }
}

/// One setting that is not at its default.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct NonDefault {
    /// The setting's key.
    pub setting: String,
    /// Its value, as the settings are written.
    pub value: String,
    /// The default it differs from.
    pub default: String,
}

impl Settings {
    /// Sets `manifest_bytes`.
    #[must_use]
    pub fn manifest_bytes(mut self, limit: Limit) -> Self {
        self.manifest_bytes = limit;
        self
    }

    /// Sets `manifest_depth`.
    #[must_use]
    pub fn manifest_depth(mut self, limit: Limit) -> Self {
        self.manifest_depth = limit;
        self
    }

    /// Sets `tree_files`.
    #[must_use]
    pub fn tree_files(mut self, limit: Limit) -> Self {
        self.tree_files = limit;
        self
    }

    /// Sets `tree_depth`.
    #[must_use]
    pub fn tree_depth(mut self, limit: Limit) -> Self {
        self.tree_depth = limit;
        self
    }

    /// Sets `tree_bytes`.
    #[must_use]
    pub fn tree_bytes(mut self, limit: Limit) -> Self {
        self.tree_bytes = limit;
        self
    }

    /// Sets `file_bytes`.
    #[must_use]
    pub fn file_bytes(mut self, limit: Limit) -> Self {
        self.file_bytes = limit;
        self
    }

    /// Sets `context_bytes`.
    #[must_use]
    pub fn context_bytes(mut self, limit: Limit) -> Self {
        self.context_bytes = limit;
        self
    }

    /// Sets `substitution_bytes`.
    #[must_use]
    pub fn substitution_bytes(mut self, limit: Limit) -> Self {
        self.substitution_bytes = limit;
        self
    }

    /// Sets `substitution_depth`.
    #[must_use]
    pub fn substitution_depth(mut self, limit: Limit) -> Self {
        self.substitution_depth = limit;
        self
    }

    /// Sets `substitution_steps`.
    #[must_use]
    pub fn substitution_steps(mut self, limit: Limit) -> Self {
        self.substitution_steps = limit;
        self
    }

    /// Sets `catalog_bytes`.
    #[must_use]
    pub fn catalog_bytes(mut self, limit: Limit) -> Self {
        self.catalog_bytes = limit;
        self
    }

    /// Sets `icon_bytes`.
    #[must_use]
    pub fn icon_bytes(mut self, limit: Limit) -> Self {
        self.icon_bytes = limit;
        self
    }

    /// Sets `icon_width`.
    #[must_use]
    pub fn icon_width(mut self, limit: Limit) -> Self {
        self.icon_width = limit;
        self
    }

    /// Sets `icon_height`.
    #[must_use]
    pub fn icon_height(mut self, limit: Limit) -> Self {
        self.icon_height = limit;
        self
    }

    /// Sets `hook_timeout`.
    #[must_use]
    pub fn hook_timeout(mut self, limit: Limit) -> Self {
        self.hook_timeout = limit;
        self
    }

    /// Sets `hook_event_timeout`.
    #[must_use]
    pub fn hook_event_timeout(mut self, limit: Limit) -> Self {
        self.hook_event_timeout = limit;
        self
    }

    /// Sets `hook_output_bytes`.
    #[must_use]
    pub fn hook_output_bytes(mut self, limit: Limit) -> Self {
        self.hook_output_bytes = limit;
        self
    }

    /// Sets `hook_payload_bytes`.
    #[must_use]
    pub fn hook_payload_bytes(mut self, limit: Limit) -> Self {
        self.hook_payload_bytes = limit;
        self
    }

    /// Sets `hook_reason_chars`.
    #[must_use]
    pub fn hook_reason_chars(mut self, limit: Limit) -> Self {
        self.hook_reason_chars = limit;
        self
    }

    /// Every setting that differs from its default, in the schema's key order, read from the
    /// described type itself.
    #[must_use]
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        fn entries(settings: &Settings) -> Vec<(String, Limit)> {
            let value = settings
                .to_value_in(Alloc::rust())
                .expect("settings are a map of limits");
            <&[Entry]>::try_from(&value)
                .expect("settings are a map")
                .iter()
                .map(|entry| {
                    (
                        entry.key().to_owned(),
                        Limit::from_value(entry.value()).expect("a setting is a limit"),
                    )
                })
                .collect()
        }
        entries(self)
            .into_iter()
            .zip(entries(&Self::default()))
            .filter(|(now, was)| now.1 != was.1)
            .map(|((setting, value), (_, default))| NonDefault {
                setting,
                value: value.to_string(),
                default: default.to_string(),
            })
            .collect()
    }
}
