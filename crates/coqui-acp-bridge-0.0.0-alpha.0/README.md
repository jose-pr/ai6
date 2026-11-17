# coqui-acp-bridge

[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-acp/test.yml)](https://github.com/Boriken-Dev/coqui-acp/actions/workflows/test.yml)

**Put a stdio [Agent Client Protocol](https://agentclientprotocol.com) agent on the network, under a proven peer identity.** No byte reaches the agent until the peer is proven. The
bridge passes complete messages and reads none of them, so what the agent may do stays with the agent. Documentation: <https://boriken-dev.github.io/coqui-acp/>.

## Features

- `serve`: wait for peers, each with an agent of its own. WebSocket, and WebTransport beside it.
- `serve --linger SECONDS`: keep a link's agent for a time, and give it back to the same proven peer on a new connection, so `session/resume` reaches a live session. Output while nobody is connected is held up to a limit, and what is dropped past it is said on the next link.
- `serve --service mcp=PROGRAM`: another protocol on the same proven link. `grant PEER TOKEN` adds one to a peer already paired, with no second pairing.
- `serve --forward`, `forward`: TCP over the link in either direction, each target its own grant.
- `serve --mcp-offers`: a peer can give its agent the peer's own MCP tools, through a stdio server every agent can start.
- `mcp-config`: the same tools for an MCP client with no ACP session, such as an IDE, from one stable `mcpServers` entry: a peer keeps a stream standing, and each program gets its own.
- `serve --advertise`, `discover`: find agents on this machine and the link. What is found is a claim, and only the proof at connect tests it. `--tls-short-lived` lets it open WebTransport with no certificate authority.
- `dial`: connect out from behind NAT, reconnecting with the agent still running; ends with the agent's exit status.
- Nothing is trusted on first use. A peer proves a key in-band (`--allow`, or a paired key with `--allow-store`), by Noise or by WebCrypto, chosen per endpoint and never negotiated, and everything after that is inside an encrypted session, so a reverse proxy that ends TLS can carry it and can neither read nor inject. Or by a pinned client certificate (`--allow-cert`), which ends where TLS ends.
- `pair-accept` and `pair-dial`: a PIN makes a full key, inside the same kind of session, so a reverse proxy can carry it. One wrong try ends the attempt.
- Mutual TLS by pin, store or CA, TLS that checks only the server, or `--no-peer-proof` for a trusted local setup, where that is the choice to make. One proof per endpoint, never negotiated.
- `--permit` and `--deny` drop a source network before its handshake. `--origin` limits browsers.
- A key, a PIN or a token is never an argument.

## Installation

Not published: `publish = false` until a release is chosen. A library: a program that people run exposes its commands (`serve`, `dial`, `forward`, `pairing`, `grant`, `discover`, `keygen`, `mcp_shim`), each a module with its arguments and a `run`. `cargo run -p coqui-acp-bridge --example bridge -- <command>` is a command line over them, for trying them from a checkout; the tests use it.

## Quick start

With the example command line (`cargo run -p coqui-acp-bridge --example bridge --`, written `bridge` below). A browser, on this machine:

```sh
bridge keygen bridge.pem
bridge serve --identity bridge.pem --allow sha256:<browser peer id> -- gemini --acp
```

Two machines, paired by a PIN, with the agent behind NAT:

```sh
# On the server: shows a PIN
bridge pair-accept --listen 127.0.0.1:7443 --identity server.pem --store peers   # behind a proxy

# On the agent's machine: asks for it on standard input
bridge pair-dial wss://server.example/v1/agent/pair --identity agent.pem --store peers

# Then, on each side
bridge serve --listen 127.0.0.1:7444 --identity server.pem --allow-store peers -- my-gateway
bridge dial wss://server.example/v1/agent --identity agent.pem --trust-store peers -- gemini --acp
```

## Command line

| Command | Does |
| --- | --- |
| `keygen PATH [--method noise\|webcrypto]` | creates an identity of the method, Noise unless named, and prints its peer id; never replaces a file |
| `serve` | one agent process per proven connection, up to `--max-peers` (default 4) |
| `dial URL` | `wss://` only; a wrong remote certificate stops it |
| `pair-accept`, `pair-dial` | one PIN exchange, recorded in a trust store |

`dial --identity` proves in-band against `--peer` or `--trust-store`. `dial --tls-identity` is mutual TLS, trusting `--expect` (a fingerprint), `--ca-file` (a private CA, with the name checked), or either. `--bearer-file` adds `Authorization: Bearer`, and needs a checked certificate.

Exit status: the agent's own when it ends the bridge; 2 for a bad configuration or a refused pairing.

## What it does not protect

- An agent is a program with its own permissions. Allowing a peer gives it whatever that agent will do when asked.
- Client certificates end at TLS, so through a proxy they mean what the proxy vouches for.
- A proxy that also serves a browser client's JavaScript can replace it, so the session protects nothing against that proxy. Ship the client's code with the application or extension.
- A slow peer is dropped past 64 unproven connections at once; `--permit` is what keeps strangers out of that count.
- The agent's own children are not stopped with it.
- Key file permissions are the caller's to keep, on Windows and when reading on any platform.
- Everything else about the session, what it reads and what it sends to its model provider, is the agent's.

## Development

```sh
COQUI_ACP_TEST_METHOD=noise COQUI_ACP_TEST_NODE=/path/to/node \
COQUI_ACP_TEST_NOISE=/path/to/coqui-peer-noise/bindings/js/coqui-peer-noise/src/index.mjs \
  cargo test -p coqui-acp-bridge
COQUI_ACP_TEST_METHOD=webcrypto COQUI_ACP_TEST_NODE=/path/to/node \
COQUI_ACP_TEST_WEBCRYPTO=/path/to/coqui-peer-webcrypto/bindings/js/coqui-peer-webcrypto/src/index.mjs \
  cargo test -p coqui-acp-bridge
```

The suite runs once per method, `COQUI_ACP_TEST_METHOD` naming it (the default method when unset). The JavaScript peer's test needs Node 22 or later and that method's module, and says so when it skips.

## License

MPL-2.0; see [LICENSE](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE).
