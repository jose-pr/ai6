# coqui-agentic-peer

What an agent needs around a proven link: pair once, then ACP, A2A, MCP or anything else runs as a capability on the same link, with no second pairing. What crosses a stream is that protocol's own messages. The link itself (proof, pairing, streams, grants) is [`cabuya`](https://github.com/Boriken-Dev/cabuya), which this crate builds on and does not re-export.

## Features

- `home`: one identity and one trust store per user, shared by every program and capability, with each method's identity in a file of its own.
- `discovery`: DNS-SD records and per-user descriptors. What is found is a claim, and only the proof tests it.
- `companion`: native messaging for a browser extension: framing, registering a native host, and a discovery answer.
- `mcp_offer`: a peer's own MCP tools, offered to a program here through a stdio shim and a loopback port with a token.

## Installation

Not on crates.io yet:

```sh
cargo add --git https://github.com/Boriken-Dev/coqui-agentic-peer coqui-agentic-peer
```

## Quick start

```rust,ignore
use coqui_agentic_peer::{discovery, home};
use cabuya::auth::{Proof, Trust};

let identity = home::identity(None, None)?;
let store = home::store(None)?;
let proof = Proof::builder(identity).responders(Trust::Store(store)).build()?;
for candidate in discovery::discover(std::time::Duration::from_secs(2)) {
    println!("{} at {}", candidate.label, candidate.url);
}
```

The agent-specific parts of the wire are in [`docs/link.md`](https://boriken-dev.github.io/coqui-agentic-peer/link/), and the link's own in [`cabuya`](https://github.com/Boriken-Dev/cabuya/blob/main/docs/link.md).

## License

[MPL-2.0](../../LICENSE).
