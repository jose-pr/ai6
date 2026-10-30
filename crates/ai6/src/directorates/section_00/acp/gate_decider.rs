//! The green-ink gate as a [`PermissionDecider`]: an external agent's request to run a shell
//! command is judged by the same function a Claude Code `PreToolUse` hook calls.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use super::{PermissionDecider, PermissionDecision, PermissionRequest};
use crate::directorates::section_00::gate::{GateClient, GateVerdict, gate};

/// Asks the Station, through the Session's own Cable endpoint, whether a command may run.
///
/// A request for a tool that is not a shell command (`read`, `edit`, `search`, ...) is
/// allowed: green ink governs the steps a command takes, as it does for a hook. An `execute`
/// request that names no command is refused.
#[derive(Debug)]
pub struct GateDecider {
    client: Option<GateClient>,
    cwd: PathBuf,
}

impl GateDecider {
    /// A decider for a Session started with `environment`, whose commands run in `cwd`. Without
    /// a Cable endpoint in the environment, a governed step is held.
    pub fn from_environment(
        environment: &BTreeMap<String, String>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            client: GateClient::from_environment(environment),
            cwd: cwd.into(),
        }
    }
}

/// The command line an `execute` request names: a string `command`, or an argument vector.
fn command_of(input: &Value) -> Option<String> {
    let command = input.get("command").or_else(|| input.get("cmd"))?;
    match command {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Array(parts) => {
            let parts: Option<Vec<&str>> = parts.iter().map(Value::as_str).collect();
            parts
                .filter(|parts| !parts.is_empty())
                .map(|parts| parts.join(" "))
        }
        _ => None,
    }
}

impl PermissionDecider for GateDecider {
    fn decide(&self, request: &PermissionRequest) -> PermissionDecision {
        if request.kind.as_deref() != Some("execute") {
            return PermissionDecision::Allow;
        }
        let Some(command) = request.raw_input.as_ref().and_then(command_of) else {
            return PermissionDecision::Deny(
                "Green ink: the request names no command, so it is held.".to_owned(),
            );
        };
        match gate(&command, Some(&self.cwd), self.client.as_ref()) {
            GateVerdict::Allow => PermissionDecision::Allow,
            GateVerdict::Hold(reason) => PermissionDecision::Deny(reason),
        }
    }
}
