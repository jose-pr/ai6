//! Claude Code's access table: what each setting of an [`Access`] becomes in Claude Code's own words.

use coqui_acp_harness::{Access, Approvals, Part, Sandbox, Target, Translation};

/// Claude Code, as the target of an access.
pub const TARGET: Target = Target::new("claude-code", translate);

const fn translate(access: Access) -> Translation {
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
    Translation::new(sandbox, network, approvals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_well_formed() {
        assert_eq!(TARGET.well_formed(), Ok(()));
    }

    #[test]
    fn the_standard_access_is_the_default_mode_and_no_settings() {
        let standard = Access::STANDARD.translate(&TARGET);
        assert_eq!(standard.mode(), Some("default"));
        assert_eq!(standard.json(), None);
    }

    #[test]
    fn a_read_only_sandbox_is_the_plan_mode() {
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::ReadOnly;
        access.approvals = Approvals::Never;
        assert_eq!(access.translate(&TARGET).mode(), Some("plan"));
    }

    #[test]
    fn settings_add_up_from_the_sandbox_and_the_network() {
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::Workspace;
        access.network = false;
        let merged = access.translate(&TARGET).json().expect("settings");
        assert_eq!(merged["enabled"], true);
        assert_eq!(merged["allowUnsandboxedCommands"], false);
        assert_eq!(merged["network"]["strictAllowlist"], true);
    }
}
