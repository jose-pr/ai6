//! The modes of `coqui_acp_harness::mode`, as Codex's approval policy, sandbox, approvals reviewer and collaboration
//! mode.
//!
//! Measured on Codex 0.166.0 (`app-server generate-json-schema`, then `thread/start` and `collaborationMode/list`
//! with no turn): the approval policy is `untrusted`, `on-request` or `never`; the sandbox `read-only`,
//! `workspace-write` or `danger-full-access`; the reviewer `user`, `auto_review` or `guardian_subagent`; the
//! collaboration modes `plan` and `default`. `turn/start` takes each of them, so a mode holds from the next turn.

use alloc::string::String;

use coqui_acp_harness::mode::Mode;
use serde_json::{Map, Value, json};

/// What Codex is told for a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSettings {
    /// `approvalPolicy`.
    pub approval: &'static str,
    /// `approvalsReviewer`.
    pub reviewer: &'static str,
    /// The sandbox, as `sandboxPolicy`'s `type`.
    pub sandbox: &'static str,
    /// `collaborationMode`'s `mode`.
    pub collaboration: &'static str,
}

/// The one table of modes: each mode's settings.
#[must_use]
pub const fn settings(mode: Mode) -> ModeSettings {
    let (approval, reviewer, sandbox, collaboration) = match mode {
        Mode::Manual => ("untrusted", "user", "workspaceWrite", "default"),
        Mode::AcceptEdits => ("on-request", "user", "workspaceWrite", "default"),
        Mode::Auto => ("on-request", "auto_review", "workspaceWrite", "default"),
        Mode::Plan => ("on-request", "user", "readOnly", "plan"),
        Mode::Bypass => ("never", "user", "dangerFullAccess", "default"),
        _ => ("untrusted", "user", "readOnly", "default"),
    };
    ModeSettings {
        approval,
        reviewer,
        sandbox,
        collaboration,
    }
}

/// What a thread is in: one of the modes, or settings no mode names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// The settings of exactly this mode.
    Known(Mode),
    /// None of the modes: the real values, as a person reads them.
    Custom(String),
}

/// What a thread is in, from what Codex answered to `thread/start` or `thread/resume`: a mode only when its approval
/// policy, sandbox and reviewer are exactly the ones the mode sets, else the real values. A thread has no collaboration
/// mode of its own to read, so Plan is read as its policy and sandbox, `on-request` with `readOnly`. `None` when the
/// answer names no policy or sandbox.
#[must_use]
pub fn of_thread(opened: &Value) -> Option<Shown> {
    let approval = match opened.get("approvalPolicy")? {
        Value::String(policy) => policy.as_str(),
        _ => "granular",
    };
    let reviewer = opened
        .get("approvalsReviewer")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let sandbox = opened.pointer("/sandbox/type")?.as_str()?;
    // Plan is a collaboration mode, which a thread does not report: its policy and sandbox alone are read-only
    // `on-request`, which a thread that was never put in Plan has too, so they are shown as they are.
    for mode in Mode::ALL.into_iter().filter(|mode| *mode != Mode::Plan) {
        let table = settings(mode);
        if (table.approval, table.sandbox, table.reviewer) == (approval, sandbox, reviewer) {
            return Some(Shown::Known(mode));
        }
    }
    let mut label = format!("{approval}, {sandbox}");
    if reviewer != "user" {
        label.push_str(&format!(", reviewed by {reviewer}"));
    }
    Some(Shown::Custom(label))
}

/// The mode a host's own approval policy and sandbox (`CodexLaunch`'s words for them) amount to, `None` for a pair no
/// mode names.
#[must_use]
pub fn configured(approval: &str, sandbox: &str) -> Option<Mode> {
    let sandbox = match sandbox {
        "read-only" => "readOnly",
        "workspace-write" => "workspaceWrite",
        "danger-full-access" => "dangerFullAccess",
        other => other,
    };
    Mode::ALL.into_iter().find(|mode| {
        let table = settings(*mode);
        table.reviewer == "user" && (table.approval, table.sandbox) == (approval, sandbox)
    })
}

/// The `turn/start` parameters that put a turn in `mode`. `model` and `effort` fill the collaboration mode's
/// settings, which name both; `workspace` is the thread's own `sandbox_workspace_write` table, so a workspace
/// sandbox keeps the writable roots and network setting the host gave the thread.
#[must_use]
pub fn turn_params(
    mode: Mode,
    model: Option<&str>,
    effort: Option<&str>,
    instructions: Option<&str>,
    workspace: &Map<String, Value>,
) -> Map<String, Value> {
    let table = settings(mode);
    let mut sandbox = Map::new();
    sandbox.insert("type".into(), json!(table.sandbox));
    if table.sandbox == "workspaceWrite" {
        if let Some(roots) = workspace.get("writable_roots") {
            sandbox.insert("writableRoots".into(), roots.clone());
        }
        if let Some(network) = workspace.get("network_access").and_then(Value::as_bool) {
            sandbox.insert("networkAccess".into(), json!(network));
        }
    }
    let mut params = Map::new();
    params.insert("approvalPolicy".into(), json!(table.approval));
    params.insert("approvalsReviewer".into(), json!(table.reviewer));
    params.insert("sandboxPolicy".into(), Value::Object(sandbox));
    // Codex takes the collaboration mode over the model, effort and developer instructions the thread was started with
    // (its schema says so), and `null` instructions mean the mode's built-in ones: so the thread's own go in, as does
    // what the session uses now, and nothing is sent as null. Without a model there is no collaboration mode to name.
    if let Some(model) = model {
        let mut settings = Map::new();
        settings.insert("model".into(), json!(model));
        if let Some(effort) = effort {
            settings.insert("reasoning_effort".into(), json!(effort));
        }
        if let Some(instructions) = instructions {
            settings.insert("developer_instructions".into(), json!(instructions));
        }
        params.insert(
            "collaborationMode".into(),
            json!({"mode": table.collaboration, "settings": settings}),
        );
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(approval: &str, sandbox: &str, reviewer: &str) -> Value {
        json!({"approvalPolicy": approval, "approvalsReviewer": reviewer, "sandbox": {"type": sandbox}})
    }

    #[test]
    fn a_thread_in_exactly_the_state_a_mode_sets_is_in_that_mode_except_plan() {
        for mode in Mode::ALL {
            let table = settings(mode);
            let read = of_thread(&opened(table.approval, table.sandbox, table.reviewer));
            if mode == Mode::Plan {
                // Plan is a collaboration mode a thread does not report: its policy and sandbox alone are shown as they are.
                assert_eq!(
                    read,
                    Some(Shown::Custom("on-request, readOnly".to_string()))
                );
            } else {
                assert_eq!(read, Some(Shown::Known(mode)), "{mode:?}");
            }
        }
    }

    #[test]
    fn any_other_pair_is_custom_with_its_real_values_never_a_mode() {
        for (approval, sandbox, reviewer, label) in [
            ("untrusted", "readOnly", "user", "untrusted, readOnly"),
            ("never", "workspaceWrite", "user", "never, workspaceWrite"),
            (
                "untrusted",
                "dangerFullAccess",
                "user",
                "untrusted, dangerFullAccess",
            ),
            (
                "on-request",
                "workspaceWrite",
                "guardian_subagent",
                "on-request, workspaceWrite, reviewed by guardian_subagent",
            ),
            (
                "never",
                "dangerFullAccess",
                "auto_review",
                "never, dangerFullAccess, reviewed by auto_review",
            ),
        ] {
            assert_eq!(
                of_thread(&opened(approval, sandbox, reviewer)),
                Some(Shown::Custom(label.to_string())),
            );
        }
        assert_eq!(
            of_thread(
                &json!({"approvalPolicy": {"granular": {}}, "sandbox": {"type": "readOnly"}})
            ),
            Some(Shown::Custom("granular, readOnly".to_string()))
        );
        assert_eq!(of_thread(&json!({})), None);
    }

    #[test]
    fn a_hosts_policy_and_sandbox_amount_to_a_mode_or_to_none() {
        assert_eq!(
            configured("untrusted", "workspace-write"),
            Some(Mode::Manual)
        );
        assert_eq!(
            configured("on-request", "workspace-write"),
            Some(Mode::AcceptEdits)
        );
        assert_eq!(configured("on-request", "read-only"), Some(Mode::Plan));
        assert_eq!(
            configured("never", "danger-full-access"),
            Some(Mode::Bypass)
        );
        assert_eq!(configured("never", "workspace-write"), None);
    }

    #[test]
    fn a_turn_names_the_sessions_effort_and_instructions_and_never_null() {
        let params = turn_params(
            Mode::Plan,
            Some("gpt"),
            Some("low"),
            Some("Answer briefly."),
            &Map::new(),
        );
        assert_eq!(
            params["collaborationMode"]["settings"],
            json!({"model": "gpt", "reasoning_effort": "low", "developer_instructions": "Answer briefly."})
        );
        let bare = turn_params(Mode::Manual, Some("gpt"), None, None, &Map::new());
        assert_eq!(
            bare["collaborationMode"]["settings"],
            json!({"model": "gpt"})
        );
        assert!(
            turn_params(Mode::Manual, None, None, None, &Map::new())
                .get("collaborationMode")
                .is_none()
        );
    }
}
