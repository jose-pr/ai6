//! `mcp-shim` and `mcp-config`: the stdio MCP server `mcp-offer/1` declares to an agent, and the entry that gives an MCP client a
//! peer's tools.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

use std::path::Path;

use agentic_peer::mcp_offer;

use crate::helper::Helper;
use crate::standing;

/// The stdio MCP server, finding a peer's door in `home` (this user's own when `None`). With a `peer`, the one `config` prints, which finds that peer's standing stream; without, the one an
/// offer starts for an agent, which carries its lines to the peer that offered it.
///
/// # Errors
///
/// The server could not run.
pub fn run(peer: Option<&str>, home: Option<&Path>) -> Result<u8, String> {
    match peer {
        None => mcp_offer::shim(std::io::stdin(), std::io::stdout())
            .map(|()| 0)
            .map_err(|error| error.to_string()),
        Some(peer) => standing::shim(peer, home),
    }
}

/// The `mcpServers` entry that gives an MCP client on this machine `peer`'s tools, which holds nothing secret and stays good
/// across restarts of either side. `helper` is the program the client starts, and `home` the directory it finds the peer's door in,
/// when that is not this user's own.
///
/// # Errors
///
/// `peer` or `name` cannot be used, or the helper's path is not text.
pub fn config(
    helper: &Helper,
    home: Option<&Path>,
    peer: &str,
    name: &str,
) -> Result<String, String> {
    standing::config(helper, home, peer, name)
}
