# 🍳 nonstick

Nonstick lets you use PAM (Pluggable Authentication Modules) from Rust.
Never worry about getting stuck on unsafe code.

You can use nonstick for interacting with PAM from both sides:

- PAM applications: call into PAM to authenticate users.
- PAM modules: write a backend that PAM uses for authentication.

It supports all known PAM implementations:

- Linux-PAM, used on most (all?) Linux distributions.
- OpenPAM, used on most BSDs, including Mac OS.
- Sun's PAM, used on Solaris and derivatives like Illumos.

Further documentation can be found in the crate's rustdoc.

## Why use nonstick?

- Safe, idiomatic Rust API.
- Supports both modules and client applications.
- Works with all PAM implementations.
- Does not require system headers.
- Few dependencies, fast builds (no `syn`!).
- Well-tested on multiple platforms (AMD64 Linux, FreeBSD, and Illumos).

## Status

- **Modules**: full support for all calls by PAM into modules.
  You can use nonstick to implement a PAM module that performs any stage of the PAM lifecycle.

- **Applications**: supports only a subset of PAM features:
  
  - Authentication
  - Account management
  - Password change
  
  The remaining features (credential and session management) are coming in a future release.
  (It needs work on a safe and ergonomic API.)

The overall shape of the API is largely complete and its general shape should not change significantly.
While there may still be minor source incompatibilities pre-1.0 (e.g., moving methods around), it is unlikely that the library will be radically reworked (and I will try to avoid them unless needed).

## Testing

Nonstick is tested against all supported PAM implementations.
In addition to doctests and unit tests in the source itself, the (non-public) `testharness` sub-package performs end-to-end tests against the whole authentication process.

## Configuration

By default, nonstick uses `libpam-sys` to detect which implementation of PAM it should build against.
You can also select to build your library or application against a specific PAM implementation by setting the `LIBPAMSYS_IMPL` environment variable.
See [the documentation for `libpam-sys`](https://docs.rs/libpam-sys/) for more details.

## Cargo features

- `link` (enabled by default): Link against your system's PAM library.
  If disabled, you can still use the PAM traits and enum types to build and test your own PAM code independent of your system PAM.
- `basic-ext` (enabled by default): Include enum values provided by both OpenPAM and Linux-PAM.
- `linux-pam-ext`: Include features specific to Linux-PAM, including enum values and the ability to send binary messages.
- `openpam-ext`: Include features specific to OpenPAM (just enum values).
- `sun-ext`: Include features specific to Sun PAM (just enum values).

When `link` is enabled, you can only use the PAM features available on the configured PAM implementation.
For instance, when building with Linux-PAM, `link` and `openpam-ext` cannot be used together.

However, when `link` is disabled, you could develop and test a crate with `sun-ext` enabled using any device.

## Credits

This is a direct fork of [Anthony Nowell](http://anowell.com/)’s [`pam-rs`/`pam-bindings` crate](https://crates.io/crates/pam-bindings).
`pam-rs` was in turn inspired by:

- [`rust-pam` by tozny](https://github.com/tozny/rust-pam)
- [`pam_groupmap` by ndenev](https://github.com/ndenev/pam_groupmap)
- [`pam-http` by beatgammit](https://github.com/beatgammit/pam-http)
