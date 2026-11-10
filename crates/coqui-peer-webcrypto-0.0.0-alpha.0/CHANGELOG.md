# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- **Breaking: the builders and settings are derived by `coqui-builder`.** `WebCrypto::builder()` and
  `PairingMethod::builder()` take no arguments: the identity is the input `.identity(..)`, and `build()` returns
  `Result<_, Missing>`, an error naming `identity` until it is given. `WebCryptoBuilder` and `PairingMethodBuilder`
  are no longer `Clone`; `responders`, `initiators` and `random` take `impl Into<..>`. The settings' keys are
  setters of the builders (`max_message`, `session_limit`), `fill(Partial<..>)` and `fill_from(source)` layer a
  document or the environment under them, and `WebCryptoParts` and `PairingMethodParts` are what a builder
  collects. `WebCryptoSettings` and `PairingSettings` derive `coqui_builder::Describe` and are read as
  `Partial<..>`; their keys, defaults and the refusal of unknown keys are unchanged. The setter traits are in
  `prelude`.
- **Breaking: `NonDefault` is `coqui_builder::NonDefault`**, re-exported at the root. Its `value` and `default` are
  shown as `Type(text)`: `usize(4096)`, `u64(1000)`, and `Random(caller's)` against `Random(operating system's)`.
- `coqui-peer` is pinned at `984db3d`, and `guatiao` and `guatiao-serde` are no longer dependencies.
- **Breaking: a weakened setting is a non-default setting.** `WebCrypto::non_defaults` and `PairingMethod::non_defaults` return coqui-peer's `NonDefault { setting, value, default }`,
  one for every setting that differs from its default, named by key path (`max_message`, `session_limit`), and
  `random` when it is replaced.

### Removed

- **Breaking:** `WebCryptoBuilder::settings` and `PairingMethodBuilder::settings`: `fill(Partial::from(settings))`
  takes a whole settings value. The `ToValue`, `FromValue` and `Schema` derives of `WebCryptoSettings` and
  `PairingSettings`, and the writing of their text, with the `guatiao` and `guatiao-serde` dependencies.
- **Breaking:** the always-on `initiators` and `responders` = `any-with-key-proof` entries of a pairing method:
  admitting any proved key is what a pairing method is, not a setting left off its default.

### Added

- **Builders and settings.** `WebCrypto::builder()` and `PairingMethod::builder()` set a method up by the
  setting's name (`identity`, `responders`, `initiators`, `max_message`, `session_limit`, `random`), then `build()`;
  there is no `new`. `WebCryptoSettings { max_message, session_limit }` and `PairingSettings { max_message }` are
  values a file fills. `WebCryptoHandshake::bind` sets the outer-channel binding.
- `WebCrypto` and `WebCryptoHandshake`: the `coqui_peer::Method` named `webcrypto`, as initiator and
  responder, ending in a `Session`.
- `Session`, `Sealer`, `Opener`: an id both ends agree on, per-direction AES-256-GCM records with an inner
  type, `close` and `verify_end` so a cut stream is reported as `Error::Truncated`, `with_session_limit`
  per direction, and exports bound to their length.
- `Identity`: a P-256 key in PKCS#8, generated, read and written.
- `Trust`: fixed pins, or a live `TrustStore` with rotation and waiting entries, bounded by the store.
- `PairingMethod`, `PairingSession`, `PairDial`: a stranger's handshake ends in a type that runs a sealed
  `coqui_peer::pair` exchange, bound to the session, and carries nothing else.
- `FramedStream`: refuses a frame by its declared length before reading it.
- `WebCryptoHandshake::bind`: an optional outer binding for an endpoint that ends TLS itself,
  refused once the transcript is fixed.
- The JavaScript module in `bindings/js/coqui-peer-webcrypto`, with the same wire in both roles.
- `PROTOCOL.md` and the shared vectors in `tests/vectors/handshake.json`.
