# `libpam-sys-impls`: LibPAM library detection

This crate detects what implementation of LibPAM should be used, as part of the build script, and exports that information to downstream crates.
It can also be used at runtime, but is primarily intended for build scripts.

Its main use is as a backend for [libpam-sys](https://crates.io/crates/libpam-sys/).
That crate re-exports pretty much everything we provide.
In most cases, you can just use that instead of depending upon this directly.

## MSRV

This library supports **Rust 1.75**, as the version currently (July 2025) available in Debian Trixie and Ubuntu 24.04 LTS.