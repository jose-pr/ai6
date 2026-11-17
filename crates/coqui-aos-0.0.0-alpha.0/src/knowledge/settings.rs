//! The limits knowledge serving applies, each a setting with a default and no floor.

use guatiao::value::types::Entry;
use guatiao::{Alloc, FromValue, Schema, ToValue};

use crate::limits::{Limit, NonDefault};

/// The largest integer a call or a result carries, as JSON numbers keep it exactly.
pub const MAX_INTEGER: i64 = 9_007_199_254_740_991;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

/// How much the knowledge tools read, render and accept. Every limit has a default and no floor; a
/// looser one is reported by [`KnowledgeSettings::non_defaults`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeSettings {
    /// Bytes of a served file.
    #[schema(title = "Largest knowledge file", format = "bytes")]
    #[map(default = KnowledgeSettings::default().max_file_bytes)]
    pub max_file_bytes: Limit,
    /// Bytes of the text of one `kb_read`.
    #[schema(title = "Largest read", format = "bytes")]
    #[map(default = KnowledgeSettings::default().max_read_bytes)]
    pub max_read_bytes: Limit,
    /// Bytes of a rendered template.
    #[schema(title = "Largest rendered template", format = "bytes")]
    #[map(default = KnowledgeSettings::default().max_rendered_bytes)]
    pub max_rendered_bytes: Limit,
    /// Characters of a search query.
    #[schema(title = "Longest query")]
    #[map(default = KnowledgeSettings::default().max_query_characters)]
    pub max_query_characters: Limit,
    /// The most `top` a search takes.
    #[schema(title = "Most passages")]
    #[map(default = KnowledgeSettings::default().max_top)]
    pub max_top: Limit,
    /// Bytes of the strings and keys of a call's `context`.
    #[schema(title = "Largest context", format = "bytes")]
    #[map(default = KnowledgeSettings::default().max_context_bytes)]
    pub max_context_bytes: Limit,
    /// Levels a `context` nests, the object itself being one.
    #[schema(title = "Deepest context")]
    #[map(default = KnowledgeSettings::default().max_context_depth)]
    pub max_context_depth: Limit,
    /// `@path` includes, and Mustache partials, one in another.
    #[schema(title = "Deepest include")]
    #[map(default = KnowledgeSettings::default().max_include_depth)]
    pub max_include_depth: Limit,
}

impl Default for KnowledgeSettings {
    fn default() -> Self {
        Self {
            max_file_bytes: Limit::at(MIB),
            max_read_bytes: Limit::at(64 * KIB),
            max_rendered_bytes: Limit::at(MIB),
            max_query_characters: Limit::at(1000),
            max_top: Limit::at(100),
            max_context_bytes: Limit::at(16 * KIB),
            max_context_depth: Limit::at(8),
            max_include_depth: Limit::at(5),
        }
    }
}

macro_rules! setters {
    ($($name:ident),*) => {
        $(
            #[doc = concat!("Sets `", stringify!($name), "`.")]
            #[must_use]
            pub fn $name(mut self, limit: Limit) -> Self {
                self.$name = limit;
                self
            }
        )*
    };
}

impl KnowledgeSettings {
    setters!(
        max_file_bytes,
        max_read_bytes,
        max_rendered_bytes,
        max_query_characters,
        max_top,
        max_context_bytes,
        max_context_depth,
        max_include_depth
    );

    /// Every setting that differs from its default, in the schema's key order.
    #[must_use]
    pub fn non_defaults(&self) -> alloc::vec::Vec<NonDefault> {
        fn entries(
            settings: &KnowledgeSettings,
        ) -> alloc::vec::Vec<(alloc::string::String, Limit)> {
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
