# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `link::serve_with_terminal`: `serve_with` with a `terminal` closure called once when the mux reaches a terminal state, before the call waits for the stream handlers still running, so a host wakes work that does not read its stream and the wait is bounded. `serve_with` is it with a closure that does nothing; its behavior is unchanged.
- The `cabuya` program, behind the new feature `cli` (`sealed`, `forward` and `clap`): `pair` (a PIN, one joiner; `--join` on the other side), `serve` (listens, takes pairings with `--pair`, serves `tcp/1` to each peer by a grants file of its own), `grant` and `revoke`, `connect` (one stream on standard input and output, so it is an `ssh` `ProxyCommand`), `forward` (local ports onto targets the host reaches), `id`, `hosts` and `forget`. Identity, trust, grants and host names live in one per-user folder (`--home`, `CABUYA_HOME`). A link is length-prefixed messages (`u32` little-endian) over TCP. Nothing on the wire changes.
- `Limits::open_wait` (default 250 ms): `Mux::open` and `open_with` wait that long for a stream to end when this side holds `max_streams`, or `max_per_protocol` of that protocol, before answering `NoProgress`, so a caller that opens one short stream after another is not refused because its own closes are still in flight. Zero refuses at once, as before. Rust only; no frame changes.
- `pairing::answer`, `join_shared`, `MARKER`, `starts_pairing`, `Answered`, `Unanswered` and `PairingHost`: one responder for a listener that takes proofs and pairings. A pairing and a proof begin with the same handshake message under both methods, so a joiner of `join_shared` sends `MARKER` first and `answer` reads the first message and runs `respond` or `prove` by it. `join`, `respond` and `prove` are unchanged.
- `pairing::desk_with(options, limits)`: a desk with the PIN's lifetime, attempts, alphabet and the pending-run bounds set; `desk()` is `desk_with` of the defaults.
- A channel whose `receive` answers `Failure::Permission`, as a failed `Sealed::with_check` now does without closing the channel, makes the mux close every open stream with `CloseReason::Revoked` before the link is lost, so a peer revoked at the link reads `Revoked` and not a lost link. The owner closes the channel after the mux ends. Rust only: no frame changes.
- `mux::Stream::grant` and `Stream::window`, `Limits::max_window` and `Limits::auto_window`: a receiver widens one stream beyond the initial window with one credit frame, bounded by `max_window` (default: `window`, so nothing changes), and `auto_window` (off) doubles a stream that received over two thirds of its window between a `ping` and its `pong`. The `broken` check uses the stream's own window. No frame changes: senders already accept credit past what was read. `LinkStream::grant` has a default that answers `Unsupported`. Dart and JavaScript gain `grant`, `window` and `maxWindow`.
- `link/1` documents carry `window` and `max_frame`, and `link::Capabilities` has them as `Option`s, absent from an older peer, so a host can report that the two sides' initial windows differ.
- `forward::Rule`: a grant token `tcp/1:RULE` may match targets by pattern (an exact or `*.suffix` name, `*`, an address or a CIDR network, and a port, a range or `*`); a token naming one target behaves as before. Nothing resolves when a rule is checked, so a name target never matches an address or network rule. `grant_rule`, `Rule::matches`, `grant_token` and `from_grant_token`. `forward` now depends on `coqui-net-acl` for networks and port ranges.
- `forward::Options`, `pump_with`, `listen_with`, `serving_with` and `Target::connect_within`: the connect timeout, message size, stall time and polling intervals are settings with today's values as defaults; zero durations and chunk are refused by `Options::validate`.
- `auth::Trust::Asked` and `Trust::asked(closure)`: the host decides whether a proven key is admitted, under either method, once per handshake and never for a key whose proof failed, with the key, whether this side answered, and the source. It records nothing itself. coqui-peer-noise and coqui-peer-webcrypto are pinned at the revisions that carry the same variant.
- `mux`: named streams inside one link, with credit per stream, metadata on open, a distinct cause for every end, and an optional heartbeat, in Rust, Dart and JavaScript, on one set of vectors.
- `peers` and `link`: claims and grants per peer behind a `PeerStore` trait with audit events, and serving a link under them, failing closed, with a capability document and per-protocol admission.
- `auth` and `pairing` (feature `sealed`): Noise and WebCrypto identities, proof, the sealed channel, and PIN pairing over any channel.
- `forward` (feature `forward`): `tcp/1`, one TCP connection per stream in either direction, each target its own grant.
- The wire is the one `coqui-agentic-peer` speaks (`link/1`, `tcp/1`, every frame byte), so a link of either library talks to a link of the other; `tests/interop.rs` runs both ends under both proof methods.

### Changed

- The project is renamed from `coqui-link` to `cabuya`: the repository is `Boriken-Dev/cabuya`, the crate and the JavaScript package are `cabuya` (`cabuya::` in Rust), the Dart package is `cabuya`, and `pairing::MARKER` is `cabuya/pairing/1`. A joiner and a listener of different names do not recognise each other's marker, so both sides move together. Frames, protocol names (`link/1`, `tcp/1`) and the vectors are unchanged. The test environment variables are `CABUYA_TEST_NODE`, `CABUYA_TEST_NOISE` and `CABUYA_SOURCE_VECTORS`.
