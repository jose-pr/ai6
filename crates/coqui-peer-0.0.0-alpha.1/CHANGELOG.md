# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Builders and settings.** `TrustStore::builder(dir)` (`limits`, `ipv6_prefix`, `when_full`, `record_interval`, `rotation`, `clock`, `settings`, then `build()`), `WaitingQueue::builder(limits)` and `PairingDesk::builder(random)` (`options`, `limits`, `clock`, `settings`, then `build()`) set a thing up by the setting's name; `TrustStore::open`, `WaitingQueue::new` and `PairingDesk::new` stay as the defaults. `TrustStoreSettings`, `WaitingSettings` and `pair::PairingDeskSettings` hold what a builder takes whole, and `settings()` on the built thing returns them. `Pairing::random` sets the PAKE's random source.
- guatiao is a dependency in every build (the `guatiao` feature is gone, it was never released): `Digest` (its lowercase name), `WhenFull` (`refuse`, `replace-oldest`), `WaitingLimits`, `WaitingSettings`, `RotationOptions`, `TrustStoreSettings`, `pair::{CodeAlphabet, PairingOptions, DeskLimits, PairingDeskSettings}` derive guatiao's `ToValue`, `FromValue` and `Schema`; unknown keys are refused and an absent key is its default. `NonDefault::from_difference` renders a guatiao `Difference`; `NonDefault::of_settings` and `of_settings_against` list the keys of a settings value that differ from its schema's defaults or from another value, named by key path.
- Feature `coqui-transport` (off by default; it changes nothing else): module `transport` with `PeerFrame`, `write_frame`'s and `read_frame`'s wire as a `coqui_transport::frame` codec (`LengthPrefix::u32_be()`, empty messages allowed), held to the same vectors as the frame functions, and `ChannelTransport`, which drives a handshake over any `coqui_transport::Channel` as a `MessageTransport`, with the read and write timeouts the `_until` drivers set.
- `PairingOptions::pin_shown` refuses a joiner who asks to show its own PIN, for a responder with no operator to answer one; such a run would otherwise wait for `pin_ttl`.

### Changed

- **Breaking:** the `with_` setters are named for their setting and live on builders; the old names are gone, as nothing keeps an alias before a release without a pre-release tag. `TrustStore::with_limits` and `with_ipv6_prefix` are `TrustStore::builder(dir).limits(..)` / `.ipv6_prefix(..)`, or `TrustStore::waiting_queue(settings)` on a store that exists; `with_clock` and `with_rotation` are the builder's `clock` and `rotation`; `WaitingQueue::with_ipv6_prefix`, `PairingDesk::with_options`, `with_limits` and `with_clock` are the builders' setters; `Pairing::with_random` is `random`. `with_when_full` and `with_record_interval` are the builders' `when_full` and `record_interval`.
- **Breaking:** `WaitingLimits`, `RotationOptions`, `InvitationOptions`, `pair::{PairingOptions, DeskLimits, CodeAlphabet}`, `Paired`, `ShownPin`, `PendingSlot`, `PendingDevice`, `WaitingEntry`, `InvitationEntry`, `Invitation`, `Successor` and `Retiring` are `#[non_exhaustive]`: start from `Default` and assign the fields (`CodeAlphabet::new(chars, len)`).
- **Breaking:** `PairingOptions::non_defaults` and `DeskLimits::non_defaults` list every option that differs from its default, not only the looser ones, and name it by key path (`pin_alphabet.len`, was `pin_alphabet`, whose value was the number of combinations).
- **Breaking: a weakened setting is a non-default setting.** `Weakening { setting, value, strict }` is now
  `NonDefault { setting, value, default }`, and `PairingOptions::weakened` and `DeskLimits::weakened` are
  `non_defaults`; the old names are gone. A library lists
  settings and logs them at `debug` at most; the application that knows the use case decides what they mean.
- A source at `WaitingLimits::per_source` now refuses a newcomer instead of replacing its oldest
  entry, in `TrustStore::record_waiting` and `WaitingQueue::admit`: strangers sharing a source, as
  everyone behind one reverse proxy does, could flush a peer waiting for an operator.
  `Admission::evict` is empty unless the new `WhenFull::ReplaceOldest` is asked for through
  `WhenFull::ReplaceOldest` through the builder's `when_full`, which is the old behaviour.
- One source records a new peer at most once per second, each record being a synced write. Set it with
  `record_interval` on the `TrustStore` and `WaitingQueue` builders; zero is the old behaviour.

## [0.0.0-alpha.1] - 2026-10-29

### Changed

- The `_until` drivers bound writes as well as reads: they take a transport that implements the
  new `WriteTimeout` trait beside `ReadTimeout` (`drive_stream_until`, `complete_stream_until`,
  `drive_messages_until`, `complete_messages_until`), set its write timeout to what is left
  before every write, send and flush, and clear it before returning. A peer that stops reading
  ends the loop at its deadline with `DriveError::TimedOut` instead of holding it. A transport
  of your own implements `WriteTimeout` (a no-op where it cannot time out). A `TimedOut` or
  `WouldBlock` transport error inside such a loop is reported as `DriveError::TimedOut`.

### Added

- `ReadTimeout` and `WriteTimeout` for `&TcpStream`, so a socket another thread may shut down
  is driven through a shared reference.
- `WaitingQueue::prune`, which forgets the entries a host no longer stores once their rerecord
  time has passed, and `WaitingQueue::len` / `is_empty`.

### Fixed

- `TrustStore::record_waiting` prunes its in-memory waiting queue against the `waiting/`
  directory before deciding. Entries another process accepted or removed were remembered for
  the life of the process, so the queue grew with every source it had seen.

## [0.0.0-alpha.0] - 2026-10-27

The first pre-release, published to crates.io. The API may change in any release before 1.0.

### Added

- `PeerId` and `Digest`: canonical text `<alg>:<hash>`, parsing of bare hex, colon hex and
  grouped base32, a digest allow-list type and constant-time equality.
- `TrustStore`, `WaitingEntry { id, source }`, `WaitingLimits`: accepted and waiting peers on
  disk (format version 1), each file recording the method name beside the key bytes
  (`Method:` line), with per-source caps (IPv4 per address, an IPv4-mapped IPv6 address as its
  IPv4 address, IPv6 by a prefix of `with_ipv6_prefix` bits, /64 by default) and the rerecord
  interval. A peer is recorded only after it proved its key.
- `WaitingQueue` and `Admission`: the waiting queue's bounds without the storage, for a host
  that keeps waiting entries elsewhere (`admit`, `remember`, `forget`); the store uses it.
- `KeyMaterial`: the bytes, PEM label and method name a method gives the store.
- A sans-IO `Handshake` / `Method` pair that works over streams and message transports, with
  blocking drivers (`drive_stream`, `drive_messages`, and `complete_stream` /
  `complete_messages`, which return the completed handshake for a method-specific finish), their
  `_until` forms that end at a deadline with `DriveError::TimedOut` (reads bounded through
  `ReadTimeout`, implemented for `TcpStream`), and, behind `tokio`, the async forms of all of
  them.
- `read_frame` and `write_frame` (and their `asynch` forms): the length-prefixed frame format
  of the drivers, at most `MAX_MESSAGE`; the reader grows its buffer as bytes arrive.
- `file::{write_new, replace, Access}` and `pem::{encode, decode, Block}`: the store's atomic
  writes (no-replace and replacing, parent directories created, owner-only on request) and its
  PEM block format, for method crates. On Windows a rename, link or removal that another handle
  blocks is retried for about a second.
- Invitations in the `TrustStore`: `invite` (a 32-byte secret from a caller-supplied
  `Random`, `InvitationOptions` for validity and uses), `redeem` (atomic, one use at a time),
  `invitations` and `revoke_invitation`, which wins over redemptions in flight. The store keeps
  only the SHA-256 of the secret.
- Key rotation in the `TrustStore`: `announce_successor`, `successor_key`, `prove_successor`,
  `promote_successor`, `successors`, `retiring` and `take_rotation_events`, with
  `RotationOptions` (retire window, default 24 hours; `require_approval`; `allow_unproven`). A
  promoted key's predecessor is refused from its retirement time on; accepting a retired key
  makes it current again.
- Pairing, module `pair`: a sans-IO exchange over any channel by invitation, by PIN (SPAKE2
  with HKDF and HMAC key confirmation over both keys and an optional channel binding; feature
  `pin`) or by an RFC 8628-style device flow; `PairingDesk` for the responder's shown PIN,
  slots, user codes, approvals and throttles; `PairingOptions` and `DeskLimits` with defaults
  and `weakened()`; `CodeAlphabet`, `Pin`, `UserCode`; the `Rotation` announcement message;
  responder deadlines (`deadline`, `time_left`, `check_deadline`). Message vectors in
  `tests/vectors/pair/`.
- `Weakening { setting, value, strict }`: a setting looser than its strict default, the one type
  every crate of the family reports weakenings with.
- `Clock` (an injectable wall clock, `TrustStore::with_clock`) and `Random` (a caller-supplied
  secure random source; `Random::os` with the `pin` feature).
- Invitation secrets, PINs and the PIN exchange's keys are zeroed when dropped, and a
  `pair::Channel`'s `Debug` hides its binding. Error messages never quote input, key material
  or paths.

[Unreleased]: https://github.com/Boriken-Dev/coqui-peer/compare/v0.0.0-alpha.1...HEAD
[0.0.0-alpha.1]: https://github.com/Boriken-Dev/coqui-peer/compare/v0.0.0-alpha.0...v0.0.0-alpha.1
[0.0.0-alpha.0]: https://github.com/Boriken-Dev/coqui-peer/releases/tag/v0.0.0-alpha.0
