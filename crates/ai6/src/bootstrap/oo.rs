//! `[oo]`: Outer Office's settings, declared here for the app, its settings page and the docs. No
//! Rust program reads them: a key this file does not know is the app's to refuse. The app takes
//! this schema with its build (`clients/outer-office/lib/settings/oo_settings.g.dart`).
//!
//! Each default is the value the app applies when the file says nothing. What a bound below says
//! is what the value can mean: a negative wait, a width of 0, a page of no items.

use guatiao::schema::KindBuilder;
use guatiao::{Alloc, FromValue, MapError, Schema, ToValue, Value, ValueError};
use guatiao_intake::Form;

use crate::bounds::Seconds;

/// The smallest amount the app can tell from 0: it holds a time in microseconds.
const SMALLEST: f64 = 0.000_001;

/// A number more than 0.
mod more_than_zero {
    use super::{Alloc, FromValue, KindBuilder, MapError, SMALLEST, ToValue, Value, ValueError};

    pub(super) fn to_value_in(value: &f64, alloc: Alloc) -> Result<Value, ValueError> {
        value.to_value_in(alloc)
    }

    pub(super) fn from_value(value: &Value) -> Result<f64, MapError> {
        let number = f64::from_value(value)?;
        if number >= SMALLEST {
            Ok(number)
        } else {
            Err(MapError::bad_value("a number more than 0"))
        }
    }

    pub(super) fn kind(alloc: Alloc) -> KindBuilder {
        KindBuilder::float_bounds_in(alloc, Some(SMALLEST), None)
    }
}

/// A number of seconds more than 0.
mod seconds_more_than_zero {
    use super::{Alloc, KindBuilder, MapError, Value, ValueError};

    pub(super) fn to_value_in(value: &f64, alloc: Alloc) -> Result<Value, ValueError> {
        super::more_than_zero::to_value_in(value, alloc)
    }

    pub(super) fn from_value(value: &Value) -> Result<f64, MapError> {
        super::more_than_zero::from_value(value)
    }

    pub(super) fn kind(alloc: Alloc) -> KindBuilder {
        super::more_than_zero::kind(alloc).format("seconds")
    }
}

/// A whole number of 1 or more.
mod one_or_more {
    use super::{Alloc, FromValue, KindBuilder, MapError, ToValue, Value, ValueError};

    pub(super) fn to_value_in(value: &u64, alloc: Alloc) -> Result<Value, ValueError> {
        value.to_value_in(alloc)
    }

    pub(super) fn from_value(value: &Value) -> Result<u64, MapError> {
        match u64::from_value(value)? {
            0 => Err(MapError::bad_value("a whole number of 1 or more")),
            number => Ok(number),
        }
    }

    pub(super) fn kind(alloc: Alloc) -> KindBuilder {
        KindBuilder::int_bounds_in(alloc, Some(1), None)
    }
}

/// `[oo.appearance]`.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoAppearance {
    /// The theme a person who has chosen none starts with, or `system`; what a person chooses
    /// wins. Default: `night-ops`.
    #[schema(
        title = "Theme",
        examples(
            "night-ops",
            "black-ops",
            "dusk",
            "classic",
            "light",
            "red-file",
            "system"
        )
    )]
    pub theme: Option<String>,
}

/// `[oo.chat]`: Moneypenny's chat, in logical pixels. `width` lies between `min_width` and
/// `max_width`, or the file is refused.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoChat {
    /// How narrow the docked chat may be dragged.
    #[schema(title = "Narrowest docked chat")]
    #[map(with = more_than_zero, default = OoChat::default().min_width)]
    pub min_width: f64,
    /// How wide the docked chat may be dragged.
    #[schema(title = "Widest docked chat")]
    #[map(with = more_than_zero, default = OoChat::default().max_width)]
    pub max_width: f64,
    /// The docked chat's width until the person drags it; what they drag to wins.
    #[schema(title = "Width of the docked chat")]
    #[map(with = more_than_zero, default = OoChat::default().width)]
    pub width: f64,
    /// How far an arrow key moves the drag handle.
    #[schema(title = "An arrow key moves it")]
    #[map(with = more_than_zero, default = OoChat::default().width_step)]
    pub width_step: f64,
    /// The page keeps at least this much beside the docked chat.
    #[schema(title = "Page kept beside the chat")]
    #[map(with = more_than_zero, default = OoChat::default().min_page_width)]
    pub min_page_width: f64,
    /// The widest column her text keeps to when expanded.
    #[schema(title = "Widest column of her text")]
    #[map(with = more_than_zero, default = OoChat::default().text_width)]
    pub text_width: f64,
    /// The width her own window opens at (desktop).
    #[schema(title = "Width of her own window")]
    #[map(with = more_than_zero, default = OoChat::default().window_width)]
    pub window_width: f64,
    /// The height her own window opens at (desktop).
    #[schema(title = "Height of her own window")]
    #[map(with = more_than_zero, default = OoChat::default().window_height)]
    pub window_height: f64,
    /// Milliseconds the chat takes to move; none when the system asks for no animations.
    #[schema(title = "The chat moves in")]
    #[map(default = OoChat::default().transition_ms)]
    pub transition_ms: u64,
    /// Seconds her socket may take to say it is ready before it counts as failed.
    #[schema(title = "Her socket says it is ready within", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoChat::default().connect_timeout)]
    pub connect_timeout: f64,
    /// Seconds before her socket's first reconnect; each next one doubles, up to
    /// `retry_max_delay`.
    #[schema(title = "First reconnect after", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoChat::default().retry_delay)]
    pub retry_delay: f64,
    /// The longest wait between reconnects.
    #[schema(title = "Longest wait between reconnects", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoChat::default().retry_max_delay)]
    pub retry_max_delay: f64,
}

impl Default for OoChat {
    fn default() -> Self {
        Self {
            min_width: 320.0,
            max_width: 720.0,
            width: 400.0,
            width_step: 16.0,
            min_page_width: 360.0,
            text_width: 960.0,
            window_width: 520.0,
            window_height: 760.0,
            transition_ms: 200,
            connect_timeout: 10.0,
            retry_delay: 1.0,
            retry_max_delay: 16.0,
        }
    }
}

/// `[oo.routes]`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoRoutes {
    /// Bytes of the `text` filter of a page link; a web URL or one of Moneypenny's links with a
    /// longer one is refused like any other bad link. Keep it equal to the `unex` record's
    /// `route_text_bytes`, or her links that one accepts and this one does not show as a refused
    /// view.
    #[schema(title = "Longest text filter in a link", format = "bytes")]
    #[map(with = one_or_more, default = OoRoutes::default().text_bytes)]
    pub text_bytes: u64,
}

impl Default for OoRoutes {
    fn default() -> Self {
        Self { text_bytes: 128 }
    }
}

/// `[oo.art]`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoArt {
    /// Pixels of the widest hero picture ever fetched; a site on slow links sets 1920.
    #[schema(title = "Widest hero picture")]
    #[map(with = one_or_more, default = OoArt::default().max_width)]
    pub max_width: u64,
}

impl Default for OoArt {
    fn default() -> Self {
        Self { max_width: 3840 }
    }
}

/// `[oo.refresh]`: how often each page asks again; `0` turns that refresh off.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoRefresh {
    /// What the page shown displays from the board of Sessions, Stations and Missions.
    #[schema(title = "The page shown")]
    #[map(default = OoRefresh::default().sessions)]
    pub sessions: Seconds,
    /// The overview's recent activity.
    #[schema(title = "The overview")]
    #[map(default = OoRefresh::default().overview)]
    pub overview: Seconds,
    /// An Operation's timeline.
    #[schema(title = "A timeline")]
    #[map(default = OoRefresh::default().timeline)]
    pub timeline: Seconds,
    /// The account pools.
    #[schema(title = "The account pools")]
    #[map(default = OoRefresh::default().pools)]
    pub pools: Seconds,
    /// What the bell and the search need and the page shown does not display: Sessions, Stations,
    /// held briefs, green ink requests and Mission proposals; `0` reads them with every refresh.
    #[schema(title = "The bell and the search")]
    #[map(default = OoRefresh::default().slow)]
    pub slow: Seconds,
    /// Who the person is and what they may do; `0` reads it only at sign-in and when the operator
    /// menu opens.
    #[schema(title = "Who the person is")]
    #[map(default = OoRefresh::default().profile)]
    pub profile: Seconds,
    /// Everything, while the tab or window cannot be seen; `0` reads nothing until it can.
    #[schema(title = "While nothing can be seen")]
    #[map(default = OoRefresh::default().hidden)]
    pub hidden: Seconds,
    /// A Station that has not checked in for this many `sessions` intervals shows as silent.
    #[schema(title = "Intervals before a Station is silent")]
    #[map(with = one_or_more, default = OoRefresh::default().silent_intervals)]
    pub silent_intervals: u64,
    /// How long before the access token ends the app renews it, and before the sign-in itself ends
    /// the person is told; `0` does neither, and the sign-in ends with the token. A shorter
    /// lifetime than this is renewed halfway through.
    #[schema(title = "Renew a sign-in before it ends")]
    #[map(default = OoRefresh::default().renew_before)]
    pub renew_before: Seconds,
    /// How soon a failed renewal is tried again when `sessions` is off; else it is tried again at
    /// `sessions`.
    #[schema(title = "Retry a failed renewal after", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoRefresh::default().renew_retry)]
    pub renew_retry: f64,
}

impl Default for OoRefresh {
    fn default() -> Self {
        Self {
            sessions: Seconds::new(3),
            overview: Seconds::new(10),
            timeline: Seconds::new(10),
            pools: Seconds::new(15),
            slow: Seconds::new(15),
            profile: Seconds::new(60),
            hidden: Seconds::new(60),
            silent_intervals: 3,
            renew_before: Seconds::new(600),
            renew_retry: 30.0,
        }
    }
}

/// `[oo.controls]`: following a Mission control until the Station reports.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoControls {
    /// How long to follow a control for the Station's outcome; `0` returns once it is queued.
    #[schema(title = "Follow a control for")]
    #[map(default = OoControls::default().wait)]
    pub wait: Seconds,
    /// How often to ask for its status meanwhile.
    #[schema(title = "Ask for its status every", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoControls::default().poll)]
    pub poll: f64,
    /// How long a proposal of Moneypenny's may stay applying before its card says the outcome is
    /// unknown.
    #[schema(title = "A proposal may stay applying", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoControls::default().confirming)]
    pub confirming: f64,
}

impl Default for OoControls {
    fn default() -> Self {
        Self {
            wait: Seconds::new(10),
            poll: 0.5,
            confirming: 30.0,
        }
    }
}

/// `[oo.pages]`: how much a page asks for. What the gateway allows is the `unex` record's.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoPages {
    /// Items in one page of an Operation's timeline.
    #[schema(title = "Timeline items in a page")]
    #[map(with = one_or_more, default = OoPages::default().timeline)]
    pub timeline: u64,
    /// Records in the audit log.
    #[schema(title = "Audit records shown")]
    #[map(with = one_or_more, default = OoPages::default().audit)]
    pub audit: u64,
    /// Items of recent activity, Operations and Stations a summary lists.
    #[schema(title = "Items in a summary")]
    #[map(with = one_or_more, default = OoPages::default().summary)]
    pub summary: u64,
    /// Results the top bar's search lists at once.
    #[schema(title = "Search results listed")]
    #[map(with = one_or_more, default = OoPages::default().search)]
    pub search: u64,
}

impl Default for OoPages {
    fn default() -> Self {
        Self {
            timeline: 50,
            audit: 100,
            summary: 5,
            search: 24,
        }
    }
}

/// `[oo.terminal]`: the live Session terminal.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoTerminal {
    /// Lines the terminal keeps.
    #[schema(title = "Lines kept")]
    #[map(with = one_or_more, default = OoTerminal::default().scrollback_lines)]
    pub scrollback_lines: u64,
    /// How long the socket may take to open.
    #[schema(title = "The socket opens within", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoTerminal::default().connect_timeout)]
    pub connect_timeout: f64,
    /// The wait before the first reconnect; each next one doubles, up to `retry_max_delay`.
    #[schema(title = "First reconnect after", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoTerminal::default().retry_delay)]
    pub retry_delay: f64,
    /// The longest wait between reconnects.
    #[schema(title = "Longest wait between reconnects", format = "seconds")]
    #[map(with = seconds_more_than_zero, default = OoTerminal::default().retry_max_delay)]
    pub retry_max_delay: f64,
}

impl Default for OoTerminal {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            connect_timeout: 10.0,
            retry_delay: 1.0,
            retry_max_delay: 16.0,
        }
    }
}

/// `[oo.limits]`: how much a text field takes, in characters; `0` takes the bound away.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoInputs {
    /// A new Mission's title.
    #[schema(title = "A new Mission's title")]
    #[map(default = OoInputs::default().mission_title_chars)]
    pub mission_title_chars: u64,
    /// A new Mission's instructions.
    #[schema(title = "A new Mission's instructions")]
    #[map(default = OoInputs::default().mission_prompt_chars)]
    pub mission_prompt_chars: u64,
    /// The reason for lowering an Agent's clearance.
    #[schema(title = "The reason for lowering a clearance")]
    #[map(default = OoInputs::default().reason_chars)]
    pub reason_chars: u64,
}

impl Default for OoInputs {
    fn default() -> Self {
        Self {
            mission_title_chars: 512,
            mission_prompt_chars: 65_536,
            reason_chars: 512,
        }
    }
}

/// What a web page may fetch from outside, `[oo.assets] cdn`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
pub enum OoCdn {
    /// Nothing: every file comes from the page's own origin.
    #[default]
    #[map(rename = "none")]
    #[schema(label = "Nothing from outside")]
    None,
    /// Its fonts.
    #[map(rename = "fonts")]
    #[schema(label = "Fonts")]
    Fonts,
    /// Its fonts and its engine.
    #[map(rename = "all")]
    #[schema(label = "Fonts and the engine")]
    All,
}

/// `[oo.assets]`: read by the web page's bootstrap, before the app runs.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoAssets {
    /// What the web page may fetch from outside its own origin; the desktop app reads nothing here.
    #[schema(title = "Fetched from outside")]
    #[map(default)]
    pub cdn: OoCdn,
}

/// `[oo]`: everything in `ai6.cfg` Outer Office reads besides `[unex] url` and `ca_file`.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct OoSettings {
    /// `[oo.appearance]`.
    #[schema(title = "Appearance")]
    #[map(default)]
    #[form(form = OoAppearance)]
    pub appearance: OoAppearance,
    /// `[oo.chat]`: Moneypenny's chat, in logical pixels.
    #[schema(title = "Moneypenny's chat")]
    #[map(default)]
    #[form(form = OoChat)]
    pub chat: OoChat,
    /// `[oo.routes]`.
    #[schema(title = "Page links")]
    #[map(default)]
    #[form(form = OoRoutes)]
    pub routes: OoRoutes,
    /// `[oo.art]`.
    #[schema(title = "Hero pictures")]
    #[map(default)]
    #[form(form = OoArt)]
    pub art: OoArt,
    /// `[oo.refresh]`: how often each page asks again; `0` turns that refresh off.
    #[schema(title = "How often pages ask again")]
    #[map(default)]
    #[form(form = OoRefresh)]
    pub refresh: OoRefresh,
    /// `[oo.controls]`: following a Mission control until the Station reports.
    #[schema(title = "Following a control")]
    #[map(default)]
    #[form(form = OoControls)]
    pub controls: OoControls,
    /// `[oo.pages]`: how much a page asks for.
    #[schema(title = "How much a page asks for")]
    #[map(default)]
    #[form(form = OoPages)]
    pub pages: OoPages,
    /// `[oo.terminal]`: the live Session terminal.
    #[schema(title = "The live terminal")]
    #[map(default)]
    #[form(form = OoTerminal)]
    pub terminal: OoTerminal,
    /// `[oo.limits]`: how much a text field takes, in characters; `0` takes the bound away.
    #[schema(title = "How much a text field takes")]
    #[map(default)]
    #[form(form = OoInputs)]
    pub limits: OoInputs,
    /// `[oo.assets]`: read by the web page's bootstrap, before the app runs.
    #[schema(title = "What a web page fetches")]
    #[map(default)]
    #[form(form = OoAssets)]
    pub assets: OoAssets,
}

/// `#[map(with = ..)]` of [`Bootstrap::oo`](super::Bootstrap::oo): a Rust program keeps the table
/// as it was written and leaves its keys to the app, and its schema is [`OoSettings`]'.
pub(super) mod as_written {
    pub(crate) use guatiao_serde::adapt::{from_value, to_value_in};

    pub(crate) fn kind(alloc: guatiao::Alloc) -> guatiao::schema::KindBuilder {
        <super::OoSettings as guatiao::Schema>::kind(alloc)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn defaults() -> serde_json::Value {
        crate::directorates::m::records::to_json(&OoSettings::default()).unwrap()
    }

    /// Every `[oo.TABLE] KEY` of a document is one the declaration knows, at its default.
    fn says_the_defaults(document: &serde_json::Value, what: &str) {
        let defaults = defaults();
        let oo = document["oo"].as_object().expect(what);
        for (table, keys) in oo {
            for (key, value) in keys.as_object().expect(what) {
                let known = crate::settings::schema(&format!("oo.{table}.{key}"));
                assert!(known.is_some(), "{what}: oo.{table}.{key} is not declared");
                if let Some(default) = defaults[table].get(key) {
                    let same = default == value || default.as_f64() == value.as_f64();
                    assert!(same, "{what}: oo.{table}.{key} is {value}, not {default}");
                }
            }
        }
    }

    fn repository(path: &str) -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(path),
        )
        .unwrap()
    }

    #[test]
    fn the_example_and_the_docs_show_the_defaults_as_they_say() {
        let example = crate::bootstrap::format::parse(
            Path::new("ai6.cfg"),
            &repository("examples/config/ai6.cfg"),
        )
        .unwrap();
        let example = serde_json::to_value(guatiao_serde::Serializable::from(&example)).unwrap();
        says_the_defaults(&example, "examples/config/ai6.cfg");
        let read = OoSettings::from_value(&guatiao_serde::from_serde(&example["oo"]).unwrap());
        assert!(read.is_ok(), "{read:?}");

        let docs = repository("docs/configuration.md");
        let start = docs.find("```toml\n[unex]\nurl =").expect("the sample") + "```toml\n".len();
        let sample = &docs[start..];
        let sample = &sample[..sample.find("```").unwrap()];
        let sample: serde_json::Value = toml::from_str(sample).unwrap();
        says_the_defaults(&sample, "docs/configuration.md");
    }

    #[test]
    fn what_cannot_mean_anything_is_refused_and_the_rest_reads() {
        let read = |text: &str| -> Result<OoSettings, String> {
            crate::directorates::m::records::from_toml(text)
        };
        let set =
            read("[chat]\nwidth = 500\n[refresh]\noverview = 0\n[pages]\naudit = 500\n").unwrap();
        assert_eq!(set.chat.width, 500.0);
        assert_eq!(set.refresh.overview, Seconds::new(0));
        assert_eq!(set.pages.audit, 500);
        assert_eq!(set.pages.timeline, OoPages::default().timeline);
        for (refused, key) in [
            ("[chat]\nwidth = 0\n", "chat.width"),
            ("[chat]\nconnect_timeout = -1\n", "chat.connect_timeout"),
            ("[pages]\naudit = 0\n", "pages.audit"),
            ("[refresh]\nsessions = -1\n", "refresh.sessions"),
            ("[chat]\nno_such = 1\n", "no_such"),
        ] {
            let refusal = read(refused).unwrap_err();
            assert!(refusal.contains(key), "{refused}: {refusal}");
        }
    }

    #[test]
    fn every_key_has_a_title_a_default_and_its_unit_and_a_form_that_fits() {
        crate::directorates::m::records::oo_settings_form().unwrap();
        let schema = crate::settings::schema("oo").unwrap();
        let tables = schema["properties"].as_object().unwrap();
        assert_eq!(tables.len(), 10);
        for (table, node) in tables {
            assert!(node["title"].is_string(), "{table}");
            for (key, field) in node["properties"].as_object().unwrap() {
                assert!(field["title"].is_string(), "{table}.{key}");
                let default = field.get("default").is_some()
                    || field["description"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("Default:");
                assert!(default, "{table}.{key}: {field}");
            }
        }
        let seconds = &tables["refresh"]["properties"]["sessions"];
        assert_eq!(seconds["format"], "seconds", "{seconds}");
        assert_eq!(seconds["minimum"], 0.0, "{seconds}");
        let positive = &tables["controls"]["properties"]["poll"];
        assert_eq!(positive["format"], "seconds", "{positive}");
        assert!(positive["minimum"].as_f64().unwrap() > 0.0, "{positive}");
        assert_eq!(tables["pages"]["properties"]["audit"]["minimum"], 1);
    }
}
