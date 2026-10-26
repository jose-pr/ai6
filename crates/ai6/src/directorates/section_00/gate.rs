//! The harness gate: before a shell command runs, it names the governed steps the command
//! would take ([`classify`]) and asks the Session's Station whether a live green ink approval
//! covers each one; a step that needs one and has none holds the command, with a message
//! telling the Agent to call `request_approval`.
//!
//! The Station is reached through the Session's Cable endpoint (`AI6_CABLE_MCP_URL` and
//! `AI6_CABLE_MCP_TOKEN` in the harness environment), so a gate works wherever the harness
//! runs. Claude Code runs `ai6 green-ink gate` as a `PreToolUse` hook; the Codex handler calls
//! [`gate`] for each command approval the app-server asks for. Best effort only: a command that
//! hides its arguments is not seen, so the guarantee is the credential, which Q leases only
//! against an approval.
//!
//! [`classify`]: crate::directorates::m::approvals::classify

/// The variable, in an Agent's environment, naming the program a harness hook runs as the
/// gate with `green-ink gate`. Unset, a handler names the `ai6` program hosting it: the
/// Station, or the field runner on the Theater.
pub const GATE_PROGRAM_ENV: &str = "AI6_GREEN_INK_GATE";
/// The variable carrying the Session's Cable endpoint URL.
pub const CABLE_URL_ENV: &str = "AI6_CABLE_MCP_URL";
/// The variable carrying the Session's Cable endpoint bearer token.
pub const CABLE_TOKEN_ENV: &str = "AI6_CABLE_MCP_TOKEN";

/// What the gate decides about a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
    /// The command takes no governed step, or every one is covered or needs no green ink.
    Allow,
    /// The command is held; the text tells the Agent why and what to do.
    Hold(String),
}

#[cfg(feature = "gate")]
pub use client::{GateClient, gate, git_resolver, pre_tool_use};

#[cfg(feature = "gate")]
mod client {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::{CABLE_TOKEN_ENV, CABLE_URL_ENV, GateVerdict};
    use crate::directorates::m::approvals::{ActionScope, classify};

    /// The MCP revision the client speaks.
    const PROTOCOL_VERSION: &str = "2025-06-18";
    /// How long one exchange with the Station may take.
    const TIMEOUT: Duration = Duration::from_secs(30);

    /// A client of the Station's `check_approval` tool on the Session's Cable endpoint.
    pub struct GateClient {
        url: String,
        token: String,
        agent: ureq::Agent,
    }

    impl std::fmt::Debug for GateClient {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("GateClient")
                .field("url", &self.url)
                .finish_non_exhaustive()
        }
    }

    impl GateClient {
        /// The client for the endpoint `environment` names; `None` without one.
        pub fn from_environment(environment: &BTreeMap<String, String>) -> Option<Self> {
            Some(Self::new(
                environment.get(CABLE_URL_ENV)?.clone(),
                environment.get(CABLE_TOKEN_ENV)?.clone(),
            ))
        }

        /// The client for the endpoint this process's environment names.
        pub fn from_process_environment() -> Option<Self> {
            Some(Self::new(
                std::env::var(CABLE_URL_ENV).ok()?,
                std::env::var(CABLE_TOKEN_ENV).ok()?,
            ))
        }

        fn new(url: String, token: String) -> Self {
            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(TIMEOUT))
                .max_redirects(0)
                .http_status_as_error(false)
                .build()
                .into();
            Self { url, token, agent }
        }

        /// Asks the Station about each step: the statuses it answers, in order (`approved`,
        /// `not_required`, `needs_approval` or `refused`), each with the Station's message.
        ///
        /// # Errors
        ///
        /// Why the Station could not be asked or did not answer.
        pub fn check(
            &self,
            steps: &[ActionScope],
        ) -> Result<Vec<(String, Option<String>)>, String> {
            let (session, version) = self.initialize()?;
            let answer = self.call(&session, &version, steps);
            // Ending the MCP session is a courtesy; the Station drops idle ones itself.
            let _ = self
                .agent
                .delete(&self.url)
                .header("Authorization", &format!("Bearer {}", self.token))
                .header("Mcp-Session-Id", &session)
                .header("MCP-Protocol-Version", &version)
                .call();
            answer
        }

        fn initialize(&self) -> Result<(String, String), String> {
            let (session, reply) = self.post(
                None,
                PROTOCOL_VERSION,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {},
                        "clientInfo": { "name": "ai6-green-ink-gate", "version": env!("CARGO_PKG_VERSION") }
                    }
                }),
            )?;
            let session = session.ok_or("the Station's endpoint opened no MCP session")?;
            let version = reply
                .and_then(|reply| {
                    reply
                        .pointer("/result/protocolVersion")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| PROTOCOL_VERSION.to_owned());
            self.post(
                Some(&session),
                &version,
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            )?;
            Ok((session, version))
        }

        fn call(
            &self,
            session: &str,
            version: &str,
            steps: &[ActionScope],
        ) -> Result<Vec<(String, Option<String>)>, String> {
            let steps: Vec<Value> = steps
                .iter()
                .map(|step| json!({ "kind": step.kind, "target": step.target }))
                .collect();
            let (_, reply) = self.post(
                Some(session),
                version,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": { "name": "check_approval", "arguments": { "steps": steps } }
                }),
            )?;
            let reply = reply.ok_or("the Station did not answer the check")?;
            if let Some(error) = reply.get("error") {
                return Err(format!("the Station refused the check: {error}"));
            }
            let text = reply
                .pointer("/result/content/0/text")
                .and_then(Value::as_str)
                .ok_or("the Station's answer has no text")?;
            let answers: Vec<Value> =
                serde_json::from_str(text).map_err(|_| text.trim().to_owned())?;
            if answers.len() != steps.len() {
                return Err("the Station answered for other steps than asked".to_owned());
            }
            answers
                .iter()
                .map(|answer| {
                    let status = answer
                        .get("status")
                        .and_then(Value::as_str)
                        .ok_or("the Station's answer has no status")?;
                    let message = answer
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    Ok((status.to_owned(), message))
                })
                .collect()
        }

        /// Posts one JSON-RPC message; returns the session the Station named and the reply
        /// with the message's `id`, which arrives as JSON or as a server-sent event.
        fn post(
            &self,
            session: Option<&str>,
            version: &str,
            message: &Value,
        ) -> Result<(Option<String>, Option<Value>), String> {
            let mut request = self
                .agent
                .post(&self.url)
                .header("Authorization", &format!("Bearer {}", self.token))
                .header("Accept", "application/json, text/event-stream")
                .header("MCP-Protocol-Version", version);
            if let Some(session) = session {
                request = request.header("Mcp-Session-Id", session);
            }
            let mut response = request
                .send_json(message)
                .map_err(|error| format!("the Station could not be reached: {error}"))?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!("the Station's endpoint answered {status}"));
            }
            let session = response
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let Some(id) = message.get("id") else {
                return Ok((session, None));
            };
            let body = response
                .body_mut()
                .read_to_string()
                .map_err(|error| format!("the Station's answer could not be read: {error}"))?;
            let reply = std::iter::once(body.as_str())
                .chain(body.lines().filter_map(|line| line.strip_prefix("data:")))
                .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
                .find(|value| value.get("id") == Some(id));
            Ok((session, reply))
        }
    }

    /// Names refs the way `git rev-parse --symbolic-full-name` does in `dir`.
    pub fn git_resolver(dir: Option<PathBuf>) -> impl Fn(&str) -> Option<String> {
        move |name: &str| {
            if name.starts_with('-') {
                return None;
            }
            let mut command = Command::new("git");
            command
                .args(["rev-parse", "--symbolic-full-name", name])
                .stdin(Stdio::null())
                .stderr(Stdio::null());
            if let Some(dir) = &dir {
                command.current_dir(dir);
            }
            let output = command.output().ok()?;
            let full = String::from_utf8(output.stdout).ok()?;
            let full = full.lines().next()?.trim();
            (output.status.success() && full.starts_with("refs/")).then(|| full.to_owned())
        }
    }

    /// Answers a Claude Code `PreToolUse` hook: `input` is the JSON the hook reads on standard
    /// input. For a shell command [`gate`] holds, the decision to print, which denies the call
    /// and gives Claude the reason; `None` (print nothing, let the call go on) otherwise,
    /// including for input that names no command.
    pub fn pre_tool_use(input: &str, client: Option<&GateClient>) -> Option<String> {
        let input: Value = serde_json::from_str(input).ok()?;
        let command = input.pointer("/tool_input/command")?.as_str()?;
        let cwd = input.get("cwd").and_then(Value::as_str).map(Path::new);
        match gate(command, cwd, client) {
            GateVerdict::Allow => None,
            GateVerdict::Hold(reason) => Some(
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason
                    }
                })
                .to_string(),
            ),
        }
    }

    /// Decides whether `command`, run in `cwd`, may go ahead: it may when it takes no governed
    /// step, or when the Station says each one is approved or needs no green ink. Anything
    /// else holds it, including a Station that cannot be asked.
    pub fn gate(command: &str, cwd: Option<&Path>, client: Option<&GateClient>) -> GateVerdict {
        let resolve = git_resolver(cwd.map(Path::to_path_buf));
        let steps = match classify(command, &resolve) {
            Ok(steps) if steps.is_empty() => return GateVerdict::Allow,
            Ok(steps) => steps,
            Err(reason) => return GateVerdict::Hold(format!("Green ink: {reason}.")),
        };
        let named = steps
            .iter()
            .map(|step| format!("{} {}", step.kind, step.target))
            .collect::<Vec<_>>()
            .join(", ");
        let Some(client) = client else {
            return GateVerdict::Hold(format!(
                "Green ink: this command would {named}, and this Session has no Station to check \
                 green ink with, so it is held."
            ));
        };
        let answers = match client.check(&steps) {
            Ok(answers) => answers,
            Err(error) => {
                return GateVerdict::Hold(format!(
                    "Green ink: this command would {named}; it is held because green ink could \
                     not be checked: {error}."
                ));
            }
        };
        let mut held = Vec::new();
        for (step, (status, message)) in steps.iter().zip(answers) {
            match status.as_str() {
                "approved" | "not_required" => {}
                "needs_approval" => held.push(format!(
                    "it would {kind} {target}, which needs green ink: call request_approval with \
                     kind \"{kind}\" and target \"{target}\", wait until it is approved, then run \
                     the command again",
                    kind = step.kind,
                    target = step.target
                )),
                _ => held.push(format!(
                    "it would {} {}: {}",
                    step.kind,
                    step.target,
                    message.unwrap_or_else(|| format!("the Station answered {status}"))
                )),
            }
        }
        if held.is_empty() {
            GateVerdict::Allow
        } else {
            GateVerdict::Hold(format!("Green ink: {}.", held.join("; ")))
        }
    }
}
