//! Least-privilege Claude Code MCP configuration from validated loadouts.

use super::{Loadout, LoadoutCatalog, QError, QErrorKind};
use serde_json::{Map, Value, json};

/// Builds Claude Code's stdio MCP configuration for one exact Agent loadout.
///
/// The output contains only static command and argument declarations. It does not include
/// environment values, secret references, Mission IDs or provider Session IDs.
pub fn claude_mcp_config(catalog: &LoadoutCatalog, loadout: &Loadout) -> Result<Value, QError> {
    let servers = catalog.servers_for(loadout)?;
    let mut entries = Map::new();
    for server in servers {
        if !valid_claude_server_name(server.name())
            || !server.deny().is_empty()
            || server
                .command()
                .first()
                .is_none_or(|command| command.trim().is_empty())
        {
            return Err(QError::new(QErrorKind::Schema, None));
        }
        entries.insert(
            server.name().to_owned(),
            json!({
                "command": server.command()[0],
                "args": &server.command()[1..],
            }),
        );
    }
    let mut config = Map::new();
    config.insert("mcpServers".to_owned(), Value::Object(entries));
    Ok(Value::Object(config))
}

fn valid_claude_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::claude_mcp_config;
    use crate::directorates::q::LoadoutCatalog;
    use std::str::FromStr;

    #[test]
    fn config_contains_only_exact_loadout_servers_and_args() {
        let catalog = LoadoutCatalog::from_str(
            r#"
[loadout.007]
mcp = ["filesystem"]
secrets = ["github-token"]

[mcp.filesystem]
command = ["mcp-filesystem", "$WORKTREE"]

[mcp.unused]
command = ["mcp-unused", "--flag"]
"#,
        )
        .unwrap();
        let loadout = catalog.for_agent("007").unwrap();
        let config = claude_mcp_config(&catalog, loadout).unwrap();
        assert_eq!(
            config,
            serde_json::json!({
                "mcpServers": {
                    "filesystem": {
                        "command": "mcp-filesystem",
                        "args": ["$WORKTREE"]
                    }
                }
            })
        );
        assert!(!config.to_string().contains("github-token"));
        assert!(!config.to_string().contains("mcp-unused"));
    }

    #[test]
    fn config_rejects_hostnames_and_tool_denials_that_claude_cannot_enforce() {
        for document in [
            "[loadout.007]\nmcp=['server.name']\n[mcp.\"server.name\"]\ncommand=['mcp']",
            "[loadout.007]\nmcp=['server']\n[mcp.server]\ncommand=['mcp']\ndeny=['tool']",
        ] {
            let catalog = LoadoutCatalog::from_str(document).unwrap();
            let loadout = catalog.for_agent("007").unwrap();
            assert!(claude_mcp_config(&catalog, loadout).is_err());
        }
    }

    #[test]
    fn config_rejects_a_loadout_from_another_catalog() {
        let catalog = LoadoutCatalog::from_str("[loadout.007]").unwrap();
        let other = LoadoutCatalog::from_str("[loadout.007]\nsecrets=['x']").unwrap();
        assert!(claude_mcp_config(&catalog, other.for_agent("007").unwrap()).is_err());
    }
}
