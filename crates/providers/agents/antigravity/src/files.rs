//! The directory that gives one launch of agy its agent, MCP servers and hook.
//!
//! agy has no flag for any of the three: `--settings` refuses `mcpServers` and does not read hooks,
//! and the user's own files are shared by every Session of that OS user. It does read a workspace's
//! `.agents/`, and `--add-dir` makes this directory a workspace outside the checkout. agy keeps
//! workspaces in path order and runs commands in the first, so this directory is named after the
//! working directory with a suffix, which sorts after it (agy 1.4.12 on Linux).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ai6::guatiao::Status;
use ai6::guatiao::library::ProviderError;
use serde_json::{Map, Value, json};

use crate::{Plan, internal, refuse};

const AGENT_DESCRIPTION: &str = "An AI6 Agent's Session, with the tools AI6 chose for it.";

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One launch's directory, removed when dropped.
#[derive(Debug)]
pub(crate) struct SessionFiles {
    path: PathBuf,
    agent: String,
}

impl SessionFiles {
    /// Makes the directory beside `working_directory`, and in it the agent file (which a resumed
    /// conversation does not read, keeping the agent of its first launch), the MCP config with the
    /// plan's servers' URLs and tokens, and the hook.
    pub(crate) fn create(
        working_directory: &str,
        plan: &Plan,
        mcp_timeout_secs: i64,
        hooks: Option<&Value>,
        context: &str,
    ) -> Result<Self, ProviderError> {
        let unique = unique_suffix();
        let root = working_directory.trim_end_matches(['/', '\\']);
        let path = PathBuf::from(format!("{root}.ai6-agy-{unique}"));
        let agent = format!("ai6-{unique}");
        create_private_dir(&path).map_err(|error| {
            refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!("the Session's directory {}: {error}", path.display()),
            )
        })?;
        let files = Self { path, agent };
        let dot_agents = files.path.join(".agents");
        create_private_dir(&dot_agents).map_err(internal)?;
        let agent_dir = dot_agents.join("agents").join(&files.agent);
        std::fs::create_dir_all(&agent_dir).map_err(internal)?;
        write_private(
            &agent_dir.join("agent.md"),
            agent_file(&files.agent, plan, context).as_bytes(),
        )?;
        if !plan.servers.is_empty() {
            let config = mcp_config(plan, mcp_timeout_secs)?;
            write_private(
                &dot_agents.join("mcp_config.json"),
                config.to_string().as_bytes(),
            )?;
        }
        if let Some(hooks) = hooks {
            write_private(&dot_agents.join("hooks.json"), hooks.to_string().as_bytes())?;
        }
        Ok(files)
    }

    pub(crate) fn path_text(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    pub(crate) fn agent_name(&self) -> &str {
        &self.agent
    }
}

impl Drop for SessionFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The agent file: its frontmatter fixes the tools, its body is the system prompt. agy turns each
/// H1 into a tag of the system prompt and keeps the rest as written, so context with headings of
/// its own or `---` lines arrives whole.
fn agent_file(name: &str, plan: &Plan, context: &str) -> String {
    let tools = plan.tools.join(", ");
    format!(
        "---\nname: {name}\ndescription: {AGENT_DESCRIPTION}\ntools: [{tools}]\nmainAgent: true\n\
         subagent: false\ninheritMcp: {}\n---\n# System Prompt\n{context}\n",
        plan.inherit_mcp
    )
}

/// agy's `mcp_config.json`, which expands no `${VARIABLE}` in a URL or a header (agy 1.4.12 sent the
/// text as written), so it holds the values.
fn mcp_config(plan: &Plan, timeout_secs: i64) -> Result<Value, ProviderError> {
    let mut servers = Map::new();
    for (name, prefix) in &plan.servers {
        let url = plan_value(plan, prefix, "URL")?;
        let token = plan_value(plan, prefix, "TOKEN")?;
        let mut server = json!({
            "serverUrl": url,
            "headers": { "Authorization": format!("Bearer {token}") },
        });
        if timeout_secs != 0 {
            server["timeoutSeconds"] = json!(timeout_secs);
        }
        servers.insert(name.clone(), server);
    }
    Ok(json!({ "mcpServers": servers }))
}

fn plan_value(plan: &Plan, prefix: &str, part: &str) -> Result<String, ProviderError> {
    plan.endpoints
        .get(&format!("{prefix}_{part}"))
        .cloned()
        .ok_or_else(|| {
            refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!("the environment carries no {prefix}_{part}"),
            )
        })
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!(
        "{:x}-{:x}-{:x}",
        std::process::id(),
        nanos,
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ProviderError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(internal)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ProviderError> {
    std::fs::write(path, bytes).map_err(internal)
}
