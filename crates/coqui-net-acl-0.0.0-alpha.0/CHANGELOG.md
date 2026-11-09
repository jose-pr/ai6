# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Feature `guatiao`: `Acl`, `Rule` and `Action` derive guatiao's `ToValue`, `FromValue` and `Schema`, in the same
  document shape as the serde text form (`default`, `[[rule]]`), so a configuration type that holds an `Acl`
  derives its own without a conversion module.

### Changed

- **Breaking: a weakening is a non-default setting.** `Weakening { setting, value, strict }` is now
  `NonDefault { setting, value, default }` and `Acl::weakened` is `Acl::non_defaults`. The old names are
  gone; nothing keeps an alias before a release without a pre-release tag.

## [0.0.0-alpha.0] - 2026-10-27

The first pre-release, published to crates.io. The API may change in any release before 1.0.

### Added

- `coqui-net-acl`: an ordered access control list for network listeners. `Acl` holds `Rule`s
  (action, source, destination, port, peer fingerprint, log hint) read top to bottom, then a
  default (`Deny` unless set). `decide` answers at accept time and defers a rule that names a
  fingerprint; `decide_peer` finishes the decision after the handshake; `decide_anonymous`
  decides a peer that proves no identity by address alone; `permits_bind` is advisory;
  `weakened` reports a `permit` default.
- `Acl::canonicalize_fingerprints`: rewrites every rule's fingerprint with the host's parser
  into its canonical spelling, all or nothing, so `decide_peer` matches a rule written in any
  spelling the host accepts.
- `IpNet` and `PortRange` with `FromStr`/`Display`; an IPv4-mapped IPv6 address is the IPv4
  address it maps; a prefix above the address width matches as the full width. `ParseError`
  names what was expected and never quotes the input. `Display` on `Acl` prints a router-style
  listing.
- `Outcome` is `#[non_exhaustive]`; `Action`, `IpNet`, `PortRange`, `Rule` and `Outcome`
  implement `Hash`.
- The `serde` feature: a TOML-friendly text form (`default`, `[[rule]]`) that refuses unknown
  keys. A default build has no dependencies and no `unsafe` code.

[Unreleased]: https://github.com/Boriken-Dev/coqui-net-acl/compare/v0.0.0-alpha.0...HEAD
[0.0.0-alpha.0]: https://github.com/Boriken-Dev/coqui-net-acl/releases/tag/v0.0.0-alpha.0
