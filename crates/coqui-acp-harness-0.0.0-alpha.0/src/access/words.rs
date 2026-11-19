//! The one table that turns an [`Access`] into each harness's own words.
//!
//! The launch of a translator, the arguments and variables a host gives a program it starts, and the text a host reports
//! about what became of each setting all read [`Access::translate`], so none can say something the others do not.

use alloc::vec::Vec;
use serde_json::{Map, Value};

use super::{Access, Approvals, Sandbox};

/// A harness an [`Access`] is turned into the words of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Target {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
    /// Pi.
    Pi,
    /// Grok, started as `grok agent stdio`.
    Grok,
    /// Antigravity, started as `agy acp`.
    Antigravity,
    /// OpenCode, started as `opencode acp`.
    OpenCode,
}

impl Target {
    /// Every target.
    pub const ALL: [Self; 6] = [
        Self::Claude,
        Self::Codex,
        Self::Pi,
        Self::Grok,
        Self::Antigravity,
        Self::OpenCode,
    ];

    /// The AOS harness id the target goes by.
    #[must_use]
    pub const fn harness(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::Grok => "grok",
            Self::Antigravity => "antigravity",
            Self::OpenCode => "opencode",
        }
    }

    /// The target an AOS harness `id` names.
    #[must_use]
    pub fn from_harness(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|target| target.harness() == id)
    }
}

/// What one setting of an access became for a target.
///
/// A part whose `said` is an error passes nothing: its other fields are empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Part {
    /// The target's own word for it, where it has one: Codex's sandbox and approval policy, Claude Code's and Pi's mode
    /// (`plan` for a read-only sandbox, which Claude Code's sandbox cannot express), Grok's sandbox profile, and for Codex's
    /// workspace sandbox the value of `network_access`.
    pub word: Option<&'static str>,
    /// Arguments it adds to the program's command line, after the arguments the program is started with.
    pub args: &'static [&'static str],
    /// Variables it sets in the program's environment.
    pub env: &'static [(&'static str, &'static str)],
    /// A JSON object it adds to what the target takes as JSON: Claude Code's `--settings`, Grok's session `_meta`.
    pub json: Option<&'static str>,
    /// What became of it in words, or why the target cannot express it. Both are one clause, without a full stop.
    pub said: Result<&'static str, &'static str>,
}

impl Part {
    const NOTHING: Self = Self {
        word: None,
        args: &[],
        env: &[],
        json: None,
        said: Ok(""),
    };

    const fn word(word: &'static str, said: &'static str) -> Self {
        Self {
            word: Some(word),
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    const fn said(said: &'static str) -> Self {
        Self {
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    const fn cannot(why: &'static str) -> Self {
        Self {
            said: Err(why),
            ..Self::NOTHING
        }
    }

    const fn json(json: &'static str, said: &'static str) -> Self {
        Self {
            json: Some(json),
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    const fn args(args: &'static [&'static str], said: &'static str) -> Self {
        Self {
            args,
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    const fn env(env: &'static [(&'static str, &'static str)], said: &'static str) -> Self {
        Self {
            env,
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    /// Whether the target takes it.
    #[must_use]
    pub const fn applies(&self) -> bool {
        self.said.is_ok()
    }
}

/// An [`Access`] in a target's words, part by part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Translation {
    /// How much a command may reach.
    pub sandbox: Part,
    /// Whether a command may use the network.
    pub network: Part,
    /// What is asked first.
    pub approvals: Part,
}

impl Translation {
    fn parts(&self) -> [&Part; 3] {
        [&self.sandbox, &self.network, &self.approvals]
    }

    /// The target's mode for the access, where it has one: the sandbox's word (`plan` when read-only), else the approvals'.
    #[must_use]
    pub const fn mode(&self) -> Option<&'static str> {
        match self.sandbox.word {
            Some(word) => Some(word),
            None => self.approvals.word,
        }
    }

    /// The arguments to add to the program's command line.
    #[must_use]
    pub fn args(&self) -> Vec<&'static str> {
        self.parts()
            .iter()
            .flat_map(|part| part.args.iter().copied())
            .collect()
    }

    /// The variables to set in the program's environment.
    #[must_use]
    pub fn env(&self) -> Vec<(&'static str, &'static str)> {
        self.parts()
            .iter()
            .flat_map(|part| part.env.iter().copied())
            .collect()
    }

    /// The JSON object the parts add up to, each key set by the last part that names it; none when no part adds one.
    #[must_use]
    pub fn json(&self) -> Option<Map<String, Value>> {
        let mut merged: Option<Map<String, Value>> = None;
        for part in self.parts() {
            let Some(Value::Object(object)) = part
                .json
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
            else {
                continue;
            };
            merged.get_or_insert_with(Map::new).extend(object);
        }
        merged
    }
}

impl Access {
    /// This access in `target`'s words. What a harness cannot express is a part that says why and passes nothing.
    #[must_use]
    pub const fn translate(self, target: Target) -> Translation {
        match target {
            Target::Claude => claude(self),
            Target::Codex => codex(self),
            Target::Pi => pi(self),
            Target::Grok => grok(self),
            Target::Antigravity => antigravity(self),
            Target::OpenCode => opencode(self),
        }
    }
}

const fn codex(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Workspace => Part::word("workspace-write", "Codex's sandbox workspace-write"),
        Sandbox::ReadOnly => Part::word("read-only", "Codex's sandbox read-only"),
        _ => Part::word("danger-full-access", "Codex's sandbox danger-full-access"),
    };
    let network = match (access.network, access.sandbox) {
        (true, Sandbox::ReadOnly) => Part::cannot("Codex's read-only sandbox has no network"),
        (false, Sandbox::Unrestricted) => Part::cannot(
            "Codex's danger-full-access has no network setting: use the workspace sandbox",
        ),
        (open, Sandbox::Workspace) => Part::word(
            if open { "true" } else { "false" },
            "Codex's sandbox_workspace_write.network_access",
        ),
        _ => Part::said("the sandbox's own: it is open with no sandbox and closed when read-only"),
    };
    let approvals = match access.approvals {
        Approvals::Edits => Part::word("on-request", "Codex's approval policy on-request"),
        Approvals::Never => Part::word("never", "Codex's approval policy never"),
        _ => Part::word("untrusted", "Codex's approval policy untrusted"),
    };
    Translation {
        sandbox,
        network,
        approvals,
    }
}

const fn claude(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Workspace => Part::json(
            r#"{"enabled":true,"allowUnsandboxedCommands":false}"#,
            "Claude Code's sandbox, on, with no retry outside it (--settings)",
        ),
        Sandbox::ReadOnly => Part::word(
            "plan",
            "the plan permission mode: Claude Code's sandbox has no switch that forbids every write",
        ),
        _ => Part::said("Claude Code's own sandbox stays off"),
    };
    let network = match (access.network, access.sandbox) {
        (true, _) => Part::said("Claude Code's sandbox leaves the network to its permission asks"),
        (false, Sandbox::Workspace) => Part::json(
            r#"{"network":{"strictAllowlist":true}}"#,
            "Claude Code's sandbox network proxy with no allowed host and no asking (--settings); its WebFetch and MCP tools are not covered",
        ),
        (false, _) => Part::json(
            r#"{"enabled":true,"filesystem":{"disabled":true},"network":{"strictAllowlist":true}}"#,
            "Claude Code's sandbox network proxy with no allowed host and no asking (--settings); its WebFetch and MCP tools are not covered",
        ),
    };
    let approvals = match (access.sandbox, access.approvals) {
        (Sandbox::ReadOnly, _) => {
            Part::cannot("the plan permission mode asks nothing: it changes nothing")
        }
        (_, Approvals::Edits) => {
            Part::word("acceptEdits", "Claude Code's permission mode acceptEdits")
        }
        (_, Approvals::Never) => Part::word(
            "bypassPermissions",
            "Claude Code's permission mode bypassPermissions",
        ),
        _ => Part::word("default", "Claude Code's permission mode default"),
    };
    Translation {
        sandbox,
        network,
        approvals,
    }
}

const fn pi(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Unrestricted => Part::said("Pi has no sandbox"),
        Sandbox::ReadOnly => Part::word("plan", "the Plan mode of Pi's permission gate"),
        _ => Part::cannot(
            "Pi has no sandbox: its permission gate asks about paths outside the session's directories in every mode but Bypass",
        ),
    };
    let network = if access.network {
        Part::said("Pi has no network setting; it is on")
    } else {
        Part::cannot("Pi has no network setting")
    };
    let approvals = match (access.sandbox, access.approvals) {
        (Sandbox::ReadOnly, _) => Part::cannot("the Plan mode asks nothing: it changes nothing"),
        (_, Approvals::Edits) => Part::word(
            "accept_edits",
            "the Accept edits mode of Pi's permission gate",
        ),
        (_, Approvals::Never) => Part::word("bypass", "the Bypass mode of Pi's permission gate"),
        _ => Part::word("manual", "the Manual mode of Pi's permission gate"),
    };
    Translation {
        sandbox,
        network,
        approvals,
    }
}

const fn grok(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Workspace => Part::env(
            &[("GROK_SANDBOX", "workspace")],
            "Grok's sandbox profile workspace (GROK_SANDBOX)",
        ),
        Sandbox::ReadOnly => Part::env(
            &[("GROK_SANDBOX", "read-only")],
            "Grok's sandbox profile read-only (GROK_SANDBOX)",
        ),
        _ => Part::env(
            &[("GROK_SANDBOX", "off")],
            "Grok's sandbox profile off (GROK_SANDBOX)",
        ),
    };
    let network = match (access.network, access.sandbox) {
        (true, Sandbox::ReadOnly) => Part::cannot(
            "Grok's read-only profile blocks its commands' network, on Linux by its documentation",
        ),
        (true, _) => Part::said("the profile's own: its commands may use the network"),
        (false, Sandbox::ReadOnly) => Part::said(
            "Grok's read-only profile blocks its commands' network, on Linux by its documentation",
        ),
        (false, _) => Part::cannot(
            "Grok's built-in sandbox profiles have no network switch: a custom profile's restrict_network has",
        ),
    };
    let approvals = match access.approvals {
        Approvals::Never => Part::json(
            r#"{"yoloMode":true}"#,
            "Grok's always-approve for the session (_meta yoloMode)",
        ),
        Approvals::Edits => Part::cannot("Grok's agent mode has no accept-edits setting"),
        _ => Part::said("Grok asks before it runs a tool, as it does by default"),
    };
    Translation {
        sandbox,
        network,
        approvals,
    }
}

const fn antigravity(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Workspace => Part::args(
            &["--sandbox"],
            "`agy acp --sandbox`, which restricts its terminal",
        ),
        Sandbox::ReadOnly => Part::args(
            &["--sandbox"],
            "`agy acp --sandbox`, which restricts its terminal; Antigravity does not say it forbids every write",
        ),
        _ => Part::said("`agy acp` without --sandbox: its terminal is not restricted"),
    };
    let network = if access.network {
        Part::cannot("Antigravity has no network setting: its own applies")
    } else {
        Part::cannot("Antigravity has no network setting")
    };
    let approvals = match access.approvals {
        Approvals::Never => Part::args(
            &["--dangerously-skip-permissions"],
            "`agy acp --dangerously-skip-permissions`",
        ),
        Approvals::Edits => Part::cannot("Antigravity has no accept-edits setting"),
        _ => Part::said("Antigravity asks before it runs a tool, as it does by default"),
    };
    Translation {
        sandbox,
        network,
        approvals,
    }
}

/// OpenCode's `OPENCODE_PERMISSION`, a JSON object of tool -> `allow`, `ask` or `deny` it reads when it starts. A tool set to
/// `deny` is not offered to the model at all.
const fn opencode(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Unrestricted => Part::said("OpenCode has no sandbox"),
        Sandbox::ReadOnly => Part::env(
            &[("OPENCODE_PERMISSION", r#"{"edit":"deny"}"#)],
            "OpenCode's edit permission set to deny (OPENCODE_PERMISSION), which takes its edit and write tools away; its commands are not restricted",
        ),
        _ => Part::cannot(
            "OpenCode has no sandbox: its external_directory rule asks about paths outside the session's directory unless approvals are never",
        ),
    };
    let network = if access.network {
        Part::said("OpenCode has no network setting; it is on")
    } else {
        Part::cannot("OpenCode has no network setting")
    };
    let approvals = match (access.sandbox, access.approvals) {
        (Sandbox::ReadOnly, _) => {
            Part::cannot("a read-only sandbox denies every edit, so nothing is asked about one")
        }
        (_, Approvals::Edits) => Part::env(
            &[("OPENCODE_PERMISSION", r#"{"edit":"allow","bash":"ask"}"#)],
            "OpenCode's edit permission allow and bash permission ask (OPENCODE_PERMISSION)",
        ),
        (_, Approvals::Never) => Part::env(
            &[(
                "OPENCODE_PERMISSION",
                r#"{"edit":"allow","bash":"allow","read":"allow","external_directory":"allow","doom_loop":"allow"}"#,
            )],
            "OpenCode's permissions all allow, none asks (OPENCODE_PERMISSION)",
        ),
        _ => Part::env(
            &[("OPENCODE_PERMISSION", r#"{"edit":"ask","bash":"ask"}"#)],
            "OpenCode's edit and bash permissions ask (OPENCODE_PERMISSION)",
        ),
    };
    Translation {
        sandbox,
        network,
        approvals,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    fn every() -> Vec<Access> {
        let mut all = Vec::new();
        for sandbox in [Sandbox::Unrestricted, Sandbox::Workspace, Sandbox::ReadOnly] {
            for network in [true, false] {
                for approvals in [Approvals::Ask, Approvals::Edits, Approvals::Never] {
                    let mut access = Access::STANDARD;
                    access.sandbox = sandbox;
                    access.network = network;
                    access.approvals = approvals;
                    all.push(access);
                }
            }
        }
        all
    }

    #[test]
    fn a_part_the_target_cannot_express_passes_nothing() {
        for target in Target::ALL {
            for access in every() {
                let translation = access.translate(target);
                for part in translation.parts() {
                    if part.said.is_err() {
                        assert!(
                            part.word.is_none()
                                && part.args.is_empty()
                                && part.env.is_empty()
                                && part.json.is_none(),
                            "{target:?} {access:?}: {part:?}"
                        );
                    }
                    if let Ok(said) = part.said {
                        assert!(!said.is_empty(), "{target:?} {access:?}");
                    }
                    if let Some(json) = part.json {
                        assert!(
                            serde_json::from_str::<Value>(json).is_ok_and(|v| v.is_object()),
                            "{json}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn standard_access_in_each_targets_words() {
        let standard = |target| Access::STANDARD.translate(target);
        assert_eq!(
            standard(Target::Codex).sandbox.word,
            Some("danger-full-access")
        );
        assert_eq!(standard(Target::Codex).approvals.word, Some("untrusted"));
        assert_eq!(standard(Target::Claude).mode(), Some("default"));
        assert_eq!(standard(Target::Claude).json(), None);
        assert_eq!(standard(Target::Pi).mode(), Some("manual"));
        assert_eq!(standard(Target::Grok).env(), vec![("GROK_SANDBOX", "off")]);
        assert_eq!(standard(Target::Grok).json(), None);
        assert!(standard(Target::Antigravity).args().is_empty());
        assert_eq!(
            standard(Target::OpenCode).env(),
            vec![("OPENCODE_PERMISSION", r#"{"edit":"ask","bash":"ask"}"#)]
        );
    }

    #[test]
    fn a_read_only_sandbox_is_the_plan_mode_where_the_target_has_no_sandbox_switch() {
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::ReadOnly;
        access.approvals = Approvals::Never;
        assert_eq!(access.translate(Target::Claude).mode(), Some("plan"));
        assert_eq!(access.translate(Target::Pi).mode(), Some("plan"));
        assert_eq!(access.translate(Target::Codex).mode(), Some("read-only"));
    }

    #[test]
    fn grok_and_antigravity_take_the_sandbox_and_approvals_as_variables_arguments_and_meta() {
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::Workspace;
        access.approvals = Approvals::Never;
        let grok = access.translate(Target::Grok);
        assert_eq!(grok.env(), vec![("GROK_SANDBOX", "workspace")]);
        assert_eq!(
            grok.json().and_then(|json| json.get("yoloMode").cloned()),
            Some(Value::Bool(true))
        );
        assert!(grok.args().is_empty());
        let agy = access.translate(Target::Antigravity);
        assert_eq!(
            agy.args(),
            vec!["--sandbox", "--dangerously-skip-permissions"]
        );
        assert!(agy.env().is_empty());
        assert_eq!(agy.json(), None);
    }

    #[test]
    fn opencode_takes_its_permissions_as_one_variable() {
        let permission = |sandbox, approvals| {
            let mut access = Access::STANDARD;
            access.sandbox = sandbox;
            access.approvals = approvals;
            let translation = access.translate(Target::OpenCode);
            let env = translation.env();
            assert_eq!(env.len(), 1, "{access:?}: {env:?}");
            assert_eq!(env[0].0, "OPENCODE_PERMISSION");
            serde_json::from_str::<Value>(env[0].1).expect("a JSON object")
        };
        assert_eq!(
            permission(Sandbox::Workspace, Approvals::Edits),
            serde_json::json!({"edit": "allow", "bash": "ask"})
        );
        assert_eq!(
            permission(Sandbox::Unrestricted, Approvals::Never)["external_directory"],
            "allow"
        );
        // A read-only sandbox denies edits whatever the approvals are, and says nothing else about them.
        for approvals in [Approvals::Ask, Approvals::Edits, Approvals::Never] {
            assert_eq!(
                permission(Sandbox::ReadOnly, approvals),
                serde_json::json!({"edit": "deny"})
            );
        }
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::ReadOnly;
        assert!(!access.translate(Target::OpenCode).approvals.applies());
        assert_eq!(access.translate(Target::OpenCode).mode(), None);
    }

    #[test]
    fn claude_settings_add_up_from_the_sandbox_and_the_network() {
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::Workspace;
        access.network = false;
        let merged = access.translate(Target::Claude).json().expect("settings");
        assert_eq!(merged["enabled"], true);
        assert_eq!(merged["allowUnsandboxedCommands"], false);
        assert_eq!(merged["network"]["strictAllowlist"], true);
    }

    #[test]
    fn a_harness_id_names_its_target() {
        for target in Target::ALL {
            assert_eq!(Target::from_harness(target.harness()), Some(target));
        }
        assert_eq!(Target::from_harness("gemini-cli"), None);
    }
}
