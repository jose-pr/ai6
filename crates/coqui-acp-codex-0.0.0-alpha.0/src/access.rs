//! Codex's access table: what each setting of an [`Access`] becomes in Codex's own words.

use coqui_acp_harness::{Access, Approvals, Part, Sandbox, Target, Translation};

/// Codex, as the target of an access.
pub const TARGET: Target = Target::new("codex", translate);

const fn translate(access: Access) -> Translation {
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
    fn the_standard_access_is_no_sandbox_and_untrusted_approvals() {
        let standard = Access::STANDARD.translate(&TARGET);
        assert_eq!(standard.sandbox.word, Some("danger-full-access"));
        assert_eq!(standard.approvals.word, Some("untrusted"));
    }

    #[test]
    fn a_read_only_sandbox_is_codexs_own_read_only() {
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::ReadOnly;
        access.approvals = Approvals::Never;
        assert_eq!(access.translate(&TARGET).mode(), Some("read-only"));
    }
}
