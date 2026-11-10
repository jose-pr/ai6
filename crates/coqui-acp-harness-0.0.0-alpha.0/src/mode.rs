//! The modes a harness is run in, named once for every translator.
//!
//! A translator offers the modes its command-line tool has, as one select option whose id is [`OPTION_ID`] and whose
//! category is `mode`, so a client shows the same control over every harness.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::acp::{ConfigOption, OptionValue};

/// The id of the mode option.
pub const OPTION_ID: &str = "mode";

/// How much a harness asks before it acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Mode {
    /// Asks before everything that is not read-only.
    Manual,
    /// Edits files in the working directory without asking; asks for the rest.
    AcceptEdits,
    /// Decides what to ask on its own, through the tool's own automatic review.
    Auto,
    /// Plans and reads, and changes nothing until the plan is accepted.
    Plan,
    /// Asks for nothing and has no sandbox of its own.
    Bypass,
}

impl Mode {
    /// Every mode, in the order a client cycles them.
    pub const ALL: [Self; 5] = [
        Self::Manual,
        Self::AcceptEdits,
        Self::Auto,
        Self::Plan,
        Self::Bypass,
    ];

    /// How much the mode lets the harness do without asking: a higher number is looser. Plan reads only, Manual asks for
    /// everything, Accept edits and Auto ask for less, and Bypass for nothing.
    #[must_use]
    pub const fn looseness(self) -> u8 {
        match self {
            Self::Plan => 0,
            Self::Manual => 1,
            Self::AcceptEdits => 2,
            Self::Auto => 3,
            _ => 4,
        }
    }

    /// The id a client sends.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::AcceptEdits => "accept_edits",
            Self::Auto => "auto",
            Self::Plan => "plan",
            Self::Bypass => "bypass",
        }
    }

    /// The name people see.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::AcceptEdits => "Accept edits",
            Self::Auto => "Auto",
            Self::Plan => "Plan",
            Self::Bypass => "Bypass",
        }
    }

    /// What the mode does, in a line.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Manual => "Asks before it changes anything or runs a command",
            Self::AcceptEdits => "Edits files without asking; asks before other commands",
            Self::Auto => "Decides for itself what needs asking",
            Self::Plan => "Reads and plans; changes nothing",
            Self::Bypass => "Asks for nothing",
        }
    }

    /// The mode a client's `id` names.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }
}

/// The modes a client may choose on a harness whose host configured it to run in `loosest`: that mode and every stricter
/// one, and Bypass when `allow_bypass`, which is that switch's own consent. With no named mode (the harness runs in a state no mode names) only Plan,
/// which reads and changes nothing, is offered.
#[must_use]
pub fn up_to(loosest: Option<Mode>, allow_bypass: bool) -> Vec<Mode> {
    Mode::ALL
        .into_iter()
        .filter(|mode| match (mode, loosest) {
            // Bypass has its own switch, and the host that turned it on has said it may be chosen.
            (Mode::Bypass, _) => allow_bypass,
            (_, Some(loosest)) => mode.looseness() <= loosest.looseness(),
            (_, None) => *mode == Mode::Plan,
        })
        .collect()
}

/// What [`up_to`] gives widened by the modes a host named, in the order of [`Mode::ALL`]; Bypass only when `allow_bypass`.
#[must_use]
pub fn widened(base: &[Mode], named: &[Mode], allow_bypass: bool) -> Vec<Mode> {
    Mode::ALL
        .into_iter()
        .filter(|mode| {
            (base.contains(mode) || named.contains(mode)) && (*mode != Mode::Bypass || allow_bypass)
        })
        .collect()
}

/// The id of the value a harness in a state no mode names shows.
pub const CUSTOM: &str = "custom";

/// The mode option for a harness in a state no mode names: `label` says what the state is, so the person sees the real
/// settings, and nothing can choose it.
#[must_use]
pub fn custom_option(label: &str, offered: &[Mode]) -> ConfigOption {
    let mut option = option_of(offered, None);
    option.values.insert(
        0,
        OptionValue::new(
            CUSTOM,
            format!("Custom: {label}"),
            Some("Settings no mode names".to_string()),
        ),
    );
    option.current = CUSTOM.to_string();
    option
}

fn option_of(offered: &[Mode], current: Option<Mode>) -> ConfigOption {
    let values: Vec<OptionValue> = Mode::ALL
        .into_iter()
        .filter(|mode| offered.contains(mode) || Some(*mode) == current)
        .map(|mode| OptionValue::new(mode.id(), mode.name(), Some(mode.description().to_string())))
        .collect();
    ConfigOption::new(OPTION_ID, "Mode", current.map_or("", Mode::id), values).category("mode")
}

/// The mode option, offering `offered` in the order of [`Mode::ALL`] whatever order they are given in.
#[must_use]
pub fn option(current: Mode, offered: &[Mode]) -> ConfigOption {
    option_of(offered, Some(current))
}

/// The mode `value` of the mode option names, if `offered` holds it.
///
/// # Errors
///
/// The reason, for a refusal: the value is no mode, or this harness does not offer it.
pub fn chosen(value: &str, offered: &[Mode]) -> Result<Mode, String> {
    let mode = Mode::from_id(value).ok_or_else(|| format!("{value} is not a value of mode"))?;
    if offered.contains(&mode) {
        Ok(mode)
    } else {
        Err(format!("this agent does not offer the {} mode", mode.id()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_option_lists_only_what_is_offered_in_the_cycle_order() {
        let option = option(Mode::Manual, &[Mode::Plan, Mode::Manual]);
        let ids: Vec<&str> = option
            .values
            .iter()
            .map(|value| value.id.as_str())
            .collect();
        assert_eq!(ids, ["manual", "plan"]);
        assert_eq!(option.category.as_deref(), Some("mode"));
        assert_eq!(option.current, "manual");
    }

    #[test]
    fn a_mode_in_use_is_listed_even_when_not_offered_for_choosing() {
        let option = option(Mode::Bypass, &[Mode::Manual]);
        assert!(option.values.iter().any(|value| value.id == "bypass"));
    }

    #[test]
    fn a_mode_not_offered_is_refused() {
        assert_eq!(chosen("plan", &[Mode::Plan]), Ok(Mode::Plan));
        assert!(chosen("bypass", &[Mode::Plan]).is_err());
        assert!(chosen("bogus", &Mode::ALL).is_err());
    }

    #[test]
    fn a_host_names_the_loosest_mode_and_a_client_gets_it_and_the_stricter_ones() {
        assert_eq!(
            up_to(Some(Mode::Manual), true),
            [Mode::Manual, Mode::Plan, Mode::Bypass],
            "Bypass follows its own switch"
        );
        assert_eq!(up_to(Some(Mode::Manual), false), [Mode::Manual, Mode::Plan]);
        assert_eq!(
            up_to(Some(Mode::AcceptEdits), false),
            [Mode::Manual, Mode::AcceptEdits, Mode::Plan]
        );
        assert!(!up_to(Some(Mode::Bypass), false).contains(&Mode::Bypass));
        assert!(up_to(Some(Mode::Bypass), true).contains(&Mode::Bypass));
        assert_eq!(
            up_to(None, false),
            [Mode::Plan],
            "no named mode: only the one that changes nothing"
        );
        assert_eq!(
            widened(
                &[Mode::Manual],
                &[Mode::Auto, Mode::Bypass, Mode::Plan],
                false
            ),
            [Mode::Manual, Mode::Auto, Mode::Plan]
        );
    }

    #[test]
    fn a_state_no_mode_names_shows_what_it_is_and_cannot_be_chosen() {
        let option = custom_option("untrusted, read-only", &[Mode::Plan]);
        assert_eq!(option.current, CUSTOM);
        assert_eq!(option.values[0].id, CUSTOM);
        assert!(option.values[0].name.contains("untrusted, read-only"));
        assert!(chosen(CUSTOM, &Mode::ALL).is_err());
    }

    #[test]
    fn every_id_round_trips() {
        for mode in Mode::ALL {
            assert_eq!(Mode::from_id(mode.id()), Some(mode));
        }
    }
}
