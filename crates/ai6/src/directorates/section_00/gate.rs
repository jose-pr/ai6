//! The harness gate: before a shell command runs, it names the governed steps the command
//! would take ([`classify`]) and asks the Session's Station whether a live green ink approval
//! covers each one; a step that needs one and has none holds the command, with a message
//! telling the Agent to call `request_approval`.
//!
//! The Station is reached through the Session's Cable endpoint (`AI6_CABLE_MCP_URL` and
//! `AI6_CABLE_MCP_TOKEN` in the harness environment), so a gate works wherever the harness
//! runs. Claude Code runs `ai6 green-ink gate` as a `PreToolUse` hook; the Codex handler calls
//! [`gate`] for each command approval the app-server asks for; the Station's `theater_run`
//! tool applies [`decide`] itself. Best effort only: a command that hides its arguments is not
//! seen, so the guarantee is the credential, which Q leases only against an approval.
//!
//! [`classify`]: crate::directorates::m::approvals::classify
//! [`gate`]: crate::directorates::section_00::gate::gate

use crate::directorates::m::approvals::{ActionScope, GitContext, Repository};

/// The variable, in an Agent's roster environment, naming the program a harness hook runs as
/// the gate with `green-ink gate`. Unset, a handler names the `ai6` program hosting it: the
/// Station, or the field runner on the Theater. A Station drops it from a start or resume
/// request's environment and reports a roster value as a weakening.
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

/// What the Station answers for each step: its status (`approved`, `not_required`,
/// `needs_approval` or `refused`) and, for a refusal, why.
pub type Answers = Vec<(String, Option<String>)>;

/// Decides about a command from what [`classify`] made of it, asking `check` about the steps
/// it takes. It allows a command that takes no governed step, or one whose every step is
/// `approved` or `not_required`; anything else holds it, including a step `classify` could not
/// name and a `check` that fails.
///
/// [`classify`]: crate::directorates::m::approvals::classify
pub fn decide(
    classified: Result<Vec<ActionScope>, String>,
    check: impl FnOnce(&[ActionScope]) -> Result<Answers, String>,
) -> GateVerdict {
    let steps = match classified {
        Ok(steps) if steps.is_empty() => return GateVerdict::Allow,
        Ok(steps) => steps,
        Err(reason) => return GateVerdict::Hold(format!("Green ink: {reason}.")),
    };
    let named = steps
        .iter()
        .map(|step| format!("{} {}", step.kind, step.target))
        .collect::<Vec<_>>()
        .join(", ");
    let answers = match check(&steps) {
        Ok(answers) if answers.len() == steps.len() => answers,
        Ok(_) => {
            return GateVerdict::Hold(format!(
                "Green ink: this command would {named}; it is held because the Station answered \
                 for other steps than asked."
            ));
        }
        Err(error) => {
            return GateVerdict::Hold(format!(
                "Green ink: this command would {named}; it is held because green ink could not \
                 be checked: {error}."
            ));
        }
    };
    let mut held = Vec::new();
    for (step, (status, message)) in steps.iter().zip(answers) {
        match status.as_str() {
            "approved" | "not_required" => {}
            "needs_approval" => held.push(format!(
                "it would {kind} {target}, which needs green ink: call request_approval with kind \
                 \"{kind}\" and target \"{target}\", wait until it is approved, then run the \
                 command again",
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

/// The decision a Claude Code `PreToolUse` hook prints to deny a call, with `reason` for
/// Claude.
pub fn deny(reason: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason
        }
    })
    .to_string()
}

/// What `run` answers for one git invocation: whether it succeeded, and its standard output.
pub type GitOutput = (bool, String);

/// A [`Repository`] read by running `git`, wherever `run` runs it: in a process on this host,
/// or over a Theater's transport. `base` is where the command line runs; a [`GitContext`]'s
/// directories are followed from there, and one the gate cannot follow (`$VAR`, `~`, `-`) is
/// an error, so the command is held.
pub struct GitRepository<F> {
    base: Option<String>,
    run: F,
}

impl<F> GitRepository<F>
where
    F: Fn(&[String]) -> Result<GitOutput, String>,
{
    /// A repository found from `base` that runs git through `run`, which gets git's arguments
    /// (without the program).
    pub fn new(base: Option<String>, run: F) -> Self {
        Self { base, run }
    }

    /// `git` arguments that act where `at` says.
    fn place(&self, at: &GitContext) -> Result<Vec<String>, String> {
        let mut dir = self.base.clone();
        for step in &at.dirs {
            if step.contains(['$', '`', '%']) || step.starts_with('~') || step == "-" {
                return Err(format!(
                    "the command changes to {step:?}, which the gate cannot follow"
                ));
            }
            let absolute = step.starts_with('/')
                || step.starts_with('\\')
                || step.as_bytes().get(1) == Some(&b':');
            dir = Some(match dir {
                Some(dir) if !absolute => format!("{}/{step}", dir.trim_end_matches(['/', '\\'])),
                _ => step.clone(),
            });
        }
        let mut args = Vec::new();
        if let Some(dir) = dir {
            args.extend(["-C".to_owned(), dir]);
        }
        for (flag, value) in [("--git-dir", &at.git_dir), ("--work-tree", &at.work_tree)] {
            if let Some(value) = value {
                if value.contains(['$', '`', '%']) || value.starts_with('~') {
                    return Err(format!("{flag} {value:?} cannot be followed by the gate"));
                }
                args.push(format!("{flag}={value}"));
            }
        }
        Ok(args)
    }

    fn git(&self, at: &GitContext, command: &[&str]) -> Result<GitOutput, String> {
        let mut args = self.place(at)?;
        args.extend(command.iter().map(|arg| (*arg).to_owned()));
        (self.run)(&args)
    }
}

impl<F> Repository for GitRepository<F>
where
    F: Fn(&[String]) -> Result<GitOutput, String>,
{
    fn full_name(&self, at: &GitContext, name: &str) -> Result<Option<String>, String> {
        if name.starts_with('-') {
            return Ok(None);
        }
        let (named, output) = self.git(at, &["rev-parse", "--symbolic-full-name", name])?;
        let full = output.lines().next().unwrap_or_default().trim();
        if named && full.starts_with("refs/") {
            return Ok(Some(full.to_owned()));
        }
        // A name that is no ref fails the same way as a directory that is no repository.
        match self.git(at, &["rev-parse", "--git-dir"])? {
            (true, _) => Ok(None),
            (false, _) => Err("it is not a git repository".to_owned()),
        }
    }

    fn config(&self, at: &GitContext, key: &str) -> Result<Vec<String>, String> {
        let (found, output) = self.git(at, &["config", "--get-all", key])?;
        if found {
            return Ok(output.lines().map(str::to_owned).collect());
        }
        match self.git(at, &["rev-parse", "--git-dir"])? {
            (true, _) => Ok(Vec::new()),
            (false, _) => Err("it is not a git repository".to_owned()),
        }
    }
}

#[cfg(feature = "gate")]
pub use client::{GateClient, gate, local_git, pre_tool_use};

#[cfg(feature = "gate")]
mod client {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::{
        Answers, CABLE_TOKEN_ENV, CABLE_URL_ENV, GateVerdict, GitOutput, GitRepository, decide,
        deny,
    };
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

        /// The client for the endpoint this process's environment names, at `url` when given
        /// (the hook passes the URL the handler saw, so a project's settings cannot point the
        /// gate at another endpoint); `None` without a URL or a token.
        pub fn from_process_environment(url: Option<String>) -> Option<Self> {
            Some(Self::new(
                url.or_else(|| std::env::var(CABLE_URL_ENV).ok())?,
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
        pub fn check(&self, steps: &[ActionScope]) -> Result<Answers, String> {
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

    /// Runs `git` with `args` in a process on this host.
    ///
    /// # Errors
    ///
    /// Why git could not be started.
    pub fn local_git(args: &[String]) -> Result<GitOutput, String> {
        let output = Command::new("git")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|error| format!("git could not be run: {error}"))?;
        Ok((
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
    }

    /// Answers a Claude Code `PreToolUse` hook: `input` is the JSON the hook reads on standard
    /// input. For a shell command [`gate`] holds, and for input that is not a hook call with a
    /// command, the decision to print, which denies the call and gives Claude the reason;
    /// `None` (print nothing, let the call go on) otherwise.
    pub fn pre_tool_use(input: &str, client: Option<&GateClient>) -> Option<String> {
        let Ok(input) = serde_json::from_str::<Value>(input) else {
            return Some(deny(
                "Green ink: the hook input could not be read, so the command is held.",
            ));
        };
        let Some(command) = input.pointer("/tool_input/command").and_then(Value::as_str) else {
            return Some(deny(
                "Green ink: the hook input names no command, so the call is held.",
            ));
        };
        let cwd = input.get("cwd").and_then(Value::as_str).map(Path::new);
        match gate(command, cwd, client) {
            GateVerdict::Allow => None,
            GateVerdict::Hold(reason) => Some(deny(&reason)),
        }
    }

    /// Decides whether `command`, run in `cwd`, may go ahead, asking the repository there with
    /// `git` and the Station through `client`; see [`decide`]. Without a client a governed step
    /// is held.
    pub fn gate(command: &str, cwd: Option<&Path>, client: Option<&GateClient>) -> GateVerdict {
        let repository =
            GitRepository::new(cwd.map(|cwd| cwd.to_string_lossy().into_owned()), local_git);
        decide(classify(command, &repository), |steps| match client {
            Some(client) => client.check(steps),
            None => Err("this Session has no Station to check green ink with".to_owned()),
        })
    }

    #[cfg(test)]
    mod tests {
        use std::path::PathBuf;

        use super::*;
        use crate::directorates::m::approvals::classify;

        /// A repository with a tag `v1` and a branch `work`, made with the local git.
        fn repository() -> tempfile::TempDir {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().to_string_lossy().into_owned();
            for args in [
                vec!["init", "-q", "-b", "work"],
                vec![
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "first",
                ],
                vec!["tag", "v1"],
            ] {
                let mut full = vec!["-C".to_owned(), path.clone()];
                full.extend(args.into_iter().map(str::to_owned));
                assert!(local_git(&full).unwrap().0, "git {full:?}");
            }
            dir
        }

        fn steps(command: &str, cwd: &Path) -> Result<Vec<String>, String> {
            let repository =
                GitRepository::new(Some(cwd.to_string_lossy().into_owned()), local_git);
            classify(command, &repository).map(|steps| {
                steps
                    .into_iter()
                    .map(|step| format!("{} {}", step.kind, step.target))
                    .collect()
            })
        }

        #[test]
        fn refs_are_resolved_in_the_repository_the_command_moves_to() {
            let repo = repository();
            let elsewhere = tempfile::tempdir().unwrap();
            let path = repo.path().to_string_lossy().replace('\\', "/");
            for command in [
                format!("cd '{path}' && git push origin v1"),
                format!("git -C '{path}' push origin v1"),
                format!("git --git-dir='{path}/.git' push origin v1"),
                format!("GIT_DIR='{path}/.git' git push origin v1"),
            ] {
                assert_eq!(
                    steps(&command, elsewhere.path()).unwrap(),
                    ["push origin:refs/tags/v1"],
                    "{command}"
                );
            }
            // Asked where the hook runs instead, `v1` is no ref there and the push is held.
            assert!(steps("git push origin v1", elsewhere.path()).is_err());
            assert_eq!(
                steps("git push", repo.path()).unwrap(),
                ["push origin:refs/heads/work"]
            );
            let error = steps("cd $HOME && git push origin v1", repo.path()).unwrap_err();
            assert!(error.contains("cannot follow"), "{error}");
        }

        #[test]
        fn input_the_hook_cannot_read_is_denied() {
            for input in ["not json", "{\"tool_input\": {}}", ""] {
                let output: Value =
                    serde_json::from_str(&pre_tool_use(input, None).expect("denied")).unwrap();
                assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
            }
            let quiet = json!({ "cwd": PathBuf::from("."), "tool_input": { "command": "ls" } });
            assert_eq!(pre_tool_use(&quiet.to_string(), None), None);
        }
    }
}
