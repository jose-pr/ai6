//! The client's requests read into the harness's terms. Everything here is the client's, untrusted.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::path::{Path, PathBuf};

#[cfg(test)]
use serde_json::Map;
use serde_json::Value;

use super::{McpServer, McpTransport, Open, OpenRequest, Takes};
use crate::event::Content;

/// Why a request cannot be served as asked: answered invalid params.
pub(crate) type Invalid = String;

fn text_of<'a>(value: &'a Value, key: &str) -> Result<&'a str, Invalid> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is missing or not text"))
}

/// An absolute path by this platform's rules: a drive or UNC path on Windows, `/…` elsewhere.
pub(crate) fn absolute(path: &str, what: &str) -> Result<PathBuf, Invalid> {
    if path.contains('\0') {
        return Err(format!("{what} holds a NUL character"));
    }
    if Path::new(path).is_absolute() {
        Ok(PathBuf::from(path))
    } else {
        Err(format!("{what} must be an absolute path"))
    }
}

/// Whether `path` names a network share or a device: `\\host\share`, `//host/share`, `\\?\…` or `\\.\…`.
pub(crate) fn network(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    bytes.len() >= 2 && matches!(bytes[0], b'\\' | b'/') && matches!(bytes[1], b'\\' | b'/')
}

/// Whether a path a link led to is on the network. A resolved path on Windows is written `\\?\C:\…` for a local disk and
/// `\\?\UNC\…` for a share.
pub(crate) fn network_resolved(path: &Path) -> bool {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => rest.starts_with("UNC\\"),
        None => network(path),
    }
}

fn pairs(value: Option<&Value>, what: &str) -> Result<Vec<(String, String)>, Invalid> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value
        .as_array()
        .ok_or_else(|| format!("{what} is not a list"))?;
    list.iter()
        .map(|pair| {
            Ok((
                text_of(pair, "name")?.to_string(),
                text_of(pair, "value")?.to_string(),
            ))
        })
        .collect()
}

fn mcp_server(value: &Value, takes: Takes, version: u8) -> Result<McpServer, Invalid> {
    let name = text_of(value, "name")?.to_string();
    if name.is_empty() {
        return Err("an MCP server has an empty name".to_string());
    }
    let kind = value.get("type").and_then(Value::as_str);
    let transport = match kind {
        None | Some("stdio") => {
            let command = text_of(value, "command")?;
            absolute(command, "an MCP server's command")?;
            let args = match value.get("args") {
                None => Vec::new(),
                Some(args) => args
                    .as_array()
                    .ok_or("an MCP server's args are not a list")?
                    .iter()
                    .map(|arg| {
                        arg.as_str()
                            .map(str::to_string)
                            .ok_or("an MCP server's argument is not text")
                    })
                    .collect::<Result<_, _>>()?,
            };
            McpTransport::Stdio {
                command: command.to_string(),
                args,
                env: pairs(value.get("env"), "an MCP server's env")?,
            }
        }
        Some("http") if takes.mcp_http => McpTransport::Http {
            url: text_of(value, "url")?.to_string(),
            headers: pairs(value.get("headers"), "an MCP server's headers")?,
        },
        Some("sse") if takes.mcp_sse && version == 1 => McpTransport::Sse {
            url: text_of(value, "url")?.to_string(),
            headers: pairs(value.get("headers"), "an MCP server's headers")?,
        },
        Some(other) => return Err(format!("MCP servers of type {other} are not supported")),
    };
    Ok(McpServer::new(name, transport))
}

/// The fields of `session/new`, `session/load` and `session/resume` a harness opens a session with.
pub(crate) fn open_request(
    params: &Value,
    open: Open,
    takes: Takes,
    version: u8,
) -> Result<OpenRequest, Invalid> {
    let cwd = absolute(text_of(params, "cwd")?, "cwd")?;
    let mut request = OpenRequest::new(cwd);
    request.open = open;
    if let Some(servers) = params.get("mcpServers").filter(|value| !value.is_null()) {
        let servers = servers.as_array().ok_or("mcpServers is not a list")?;
        let mut names = Vec::new();
        for server in servers {
            let server = mcp_server(server, takes, version)?;
            if names.contains(&server.name) {
                return Err(format!("two MCP servers are named {}", server.name));
            }
            names.push(server.name.clone());
            request.mcp_servers.push(server);
        }
    }
    if let Some(prompt) = params.get("systemPrompt").filter(|value| !value.is_null()) {
        if !takes.system_prompt {
            return Err("this agent does not take a system prompt".to_string());
        }
        let mode = prompt
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("append");
        if mode != "append" {
            return Err(format!("a system prompt in {mode} mode is not supported"));
        }
        let blocks = prompt
            .get("content")
            .and_then(Value::as_array)
            .ok_or("systemPrompt.content is not a list")?;
        let mut text = Vec::new();
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("text") {
                return Err("a system prompt may hold only text".to_string());
            }
            text.push(text_of(block, "text")?.to_string());
        }
        request.system_prompt = Some(text.join("\n\n"));
    }
    if let Some(directories) = params
        .get("additionalDirectories")
        .filter(|value| !value.is_null())
    {
        let directories = directories
            .as_array()
            .ok_or("additionalDirectories is not a list")?;
        if !directories.is_empty() && !takes.additional_directories {
            return Err("this agent does not take more directories".to_string());
        }
        for directory in directories {
            let directory = directory
                .as_str()
                .ok_or("an additional directory is not text")?;
            request
                .additional_directories
                .push(absolute(directory, "an additional directory")?);
        }
    }
    if let Some(meta) = params.get("_meta").and_then(Value::as_object) {
        request.meta = meta.clone();
    }
    Ok(request)
}

/// A prompt's blocks.
pub(crate) fn prompt(params: &Value, takes: Takes) -> Result<Vec<Content>, Invalid> {
    let blocks = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or("prompt is not a list")?;
    let mut content = Vec::new();
    for block in blocks {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        content.push(match kind {
            "text" => Content::Text(text_of(block, "text")?.to_string()),
            "image" if takes.images => Content::Image {
                mime_type: text_of(block, "mimeType")?.to_string(),
                data: text_of(block, "data")?.to_string(),
            },
            "resource_link" => Content::ResourceLink {
                uri: text_of(block, "uri")?.to_string(),
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            "resource" if takes.embedded_context => {
                let resource = block
                    .get("resource")
                    .ok_or("a resource block has no resource")?;
                let string = |key: &str| {
                    resource
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                };
                if string("text").is_none() && string("blob").is_none() {
                    return Err("a resource has neither text nor blob".to_string());
                }
                Content::Resource {
                    uri: text_of(resource, "uri")?.to_string(),
                    mime_type: string("mimeType"),
                    text: string("text"),
                    blob: string("blob"),
                }
            }
            other => return Err(format!("content blocks of type {other} are not supported")),
        });
    }
    if content.is_empty() {
        return Err("the prompt is empty".to_string());
    }
    Ok(content)
}

/// A request's `sessionId`.
pub(crate) fn session_id(params: &Value) -> Result<String, Invalid> {
    Ok(text_of(params, "sessionId")?.to_string())
}

/// `{"sessionId", "configId", "value"}`, the value an id in either version.
pub(crate) fn config_choice(params: &Value) -> Result<(String, String), Invalid> {
    let id = text_of(params, "configId")?.to_string();
    let value = match params.get("value") {
        Some(Value::String(value)) => value.clone(),
        _ => return Err("value is not an option's id".to_string()),
    };
    if let Some(kind) = params.get("type").and_then(Value::as_str)
        && kind != "id"
    {
        return Err(format!(
            "config option values of type {kind} are not supported"
        ));
    }
    Ok((id, value))
}

/// The meta object of a request, or an empty one.
#[cfg(test)]
pub(crate) fn meta(params: &Value) -> Map<String, Value> {
    params
        .get("_meta")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An absolute path on this platform.
    fn abs(rest: &str) -> String {
        if cfg!(windows) {
            format!("C:\\{rest}")
        } else {
            format!("/{rest}")
        }
    }

    const ALL: Takes = Takes::new()
        .images(true)
        .embedded_context(true)
        .mcp_http(true)
        .mcp_sse(true)
        .additional_directories(true)
        .system_prompt(true);

    #[test]
    fn every_part_of_a_session_request_is_read_in_either_version() {
        for (version, stdio) in [
            (
                1,
                json!({"name": "files", "command": abs("bin/files"), "args": ["-v"], "env": [{"name": "TOKEN", "value": "t"}]}),
            ),
            (
                2,
                json!({"type": "stdio", "name": "files", "command": abs("bin/files"), "args": ["-v"], "env": [{"name": "TOKEN", "value": "t"}]}),
            ),
        ] {
            let params = json!({
                "cwd": abs("work"),
                "mcpServers": [stdio, {"type": "http", "name": "docs", "url": "https://docs.example/mcp", "headers": []}],
                "systemPrompt": {"mode": "append", "content": [{"type": "text", "text": "Be brief."}]},
                "additionalDirectories": [abs("skills")],
                "_meta": {"claudeCode": {"options": {}}}
            });
            let request = open_request(&params, Open::New, ALL, version).unwrap();
            assert_eq!(request.cwd, PathBuf::from(abs("work")));
            assert_eq!(request.mcp_servers.len(), 2);
            assert_eq!(request.system_prompt.as_deref(), Some("Be brief."));
            assert_eq!(
                request.additional_directories,
                [PathBuf::from(abs("skills"))]
            );
            assert!(request.meta.contains_key("claudeCode"));
            assert!(
                !format!("{:?}", request.mcp_servers[0]).contains("\"t\""),
                "an env value never shows"
            );
        }
    }

    #[test]
    fn what_the_agent_does_not_take_is_refused_before_anything_starts() {
        let none = Takes::new();
        for params in [
            json!({"cwd": "relative"}),
            json!({"cwd": abs("w"), "systemPrompt": {"mode": "append", "content": []}}),
            json!({"cwd": abs("w"), "additionalDirectories": [abs("x")]}),
            json!({"cwd": abs("w"), "mcpServers": [{"type": "http", "name": "a", "url": "u"}]}),
            json!({"cwd": abs("w"), "mcpServers": [{"name": "a", "command": "relative"}]}),
            json!({"cwd": abs("w"), "mcpServers": [{"name": "a", "command": abs("a")}, {"name": "a", "command": abs("b")}]}),
        ] {
            assert!(
                open_request(&params, Open::New, none, 1).is_err(),
                "{params}"
            );
        }
        let sse =
            json!({"cwd": abs("w"), "mcpServers": [{"type": "sse", "name": "a", "url": "u"}]});
        assert!(open_request(&sse, Open::New, ALL, 1).is_ok());
        assert!(
            open_request(&sse, Open::New, ALL, 2).is_err(),
            "version 2 has no SSE"
        );
        let overriding =
            json!({"cwd": abs("w"), "systemPrompt": {"mode": "override", "content": []}});
        assert!(open_request(&overriding, Open::New, ALL, 2).is_err());
    }

    #[test]
    fn a_path_is_absolute_by_this_platforms_rules() {
        if cfg!(windows) {
            assert!(absolute(r"C:\w", "cwd").is_ok());
            assert!(absolute("C:/w", "cwd").is_ok());
            assert!(absolute("/w", "cwd").is_err(), "no drive");
        } else {
            assert!(absolute("/w", "cwd").is_ok());
            assert!(absolute("C:/w", "cwd").is_err(), "relative here");
            assert!(absolute(r"C:\w", "cwd").is_err(), "relative here");
        }
    }

    #[test]
    fn a_prompt_takes_text_images_links_and_resources_and_nothing_else() {
        let params = json!({"prompt": [
            {"type": "text", "text": "Look"},
            {"type": "image", "mimeType": "image/png", "data": "AAAA"},
            {"type": "resource_link", "uri": "file:///a.txt", "name": "a.txt"},
            {"type": "resource", "resource": {"uri": "file:///b.txt", "text": "bee"}}
        ]});
        assert_eq!(prompt(&params, ALL).unwrap().len(), 4);
        assert!(
            prompt(&params, Takes::new()).is_err(),
            "an image the agent does not take"
        );
        assert!(prompt(&json!({"prompt": [{"type": "audio", "data": "x"}]}), ALL).is_err());
        assert!(prompt(&json!({"prompt": []}), ALL).is_err());
        assert_eq!(meta(&json!({"_meta": {"a": 1}}))["a"], 1);
    }
}
