# `libpam-sys`: low-level bindings to Pluggable Authentication Modules

- Supports all known PAM implementations on Linux, Mac OS, BSD, and Illumos/Solaris
- Works with zero configuration for common use cases
- No need for system header files
- Depends only on `libc`

If you're looking for a nice, safe, Rusty API to PAM, may I recommend [nonstick]?

## PAM implementations

Supported PAM implementations are defined in the `pam_impl::PamImpl` enum.

This crate automatically chooses the appropriate PAM implementation you are most likely to need installed based on the target OS.
You can also explicitly specify the PAM implementation you want (if not detected correctly) by setting the `LIBPAMSYS_IMPL` environment variable **at build time**.
All build-time configuration is performed by the build script of the [`libpam-sys-impls` crate](https://crates.io/crates/libpam-sys-impls).

Normally, this crate exports all functionality available in the selected PAM library.
`XSso` exports only the subset of the [X/SSO specification][xsso] supported by both OpenPAM and Sun PAM.

### Changing behavior based on PAM implementation

Downstream crates can detect the current PAM implementation using custom `#[cfg]`s:

```rust
// Your package's build.rs:
use libpam_sys::pam_impl;

fn main() {
  pam_impl::enable_pam_impl_cfg();
  
  // the rest of your build script...
}
```

This will enable the use of `#[cfg]`s that look like this:

```rust
#[cfg(pam_impl = "Sun")]
fn some_func() { /* Sun-specific implementation */ }

#[cfg(any(pam_impl = "LinuxPam", pam_impl = "OpenPam"))]
fn some_func() { /* Linux-PAM / OpenPAM implementation */ }
```

Further documentation on this is available in `libpam-sys-impls`.

## Testing

Tests are mostly run through `libpam-sys-test`, which lives in the crate's workspace in its repository (along with [nonstick]).

- [`ctest`][ctest] verifies the correctness of the FFI bindings (function/struct alignment, etc.).
- A kind of scuffed homebrew thing also verifies that the constants are correct.

There are some unit tests of glue code and other type checks.

## Minimum Rust version

This crate supports **Rust 1.75**, the current version in Debian Trixie and Ubuntu 24.04.2 LTS.
There shouldn't be much that needs changing, since PAM's API is quite stable.

## References

- [X/SSO PAM specification][xsso]: This 1997 document laid out the original specification for PAM.
- [Linux-PAM repository][linux-pam]: The Linux-PAM implementation, used by most (all?) Linux distributions. Contains many extensions.
  - [Linux-PAM man page][man7]: Root man page for Linux-PAM, with links to additional PAM man pages.
  - [Linux-PAM guides][linux-guides]: Documentation for developers using PAM and sysadmins.
- [OpenPAM repository][openpam]: The OpenPAM implementation, used by many BSD varieties. This hews very close to the spec.
  - [OpenPAM man page][manbsd]: NetBSD's root man page for OpenPAM.
- [Illumos PAM repository][illumos-pam]: Illumos's implementation of PAM, based on Sun's Solaris. Even more basic than OpenPAM.
  - [Illumos PAM man page][manillumos]: Illumos's root man page for its PAM implementation.

[ctest]: https://github.com/rust-lang/libc/tree/ctest-v0.4.11/ctest
[nonstick]: https://crates.io/crates/nonstick
[xsso]: https://pubs.opengroup.org/onlinepubs/8329799/toc.htm
[linux-pam]: https://github.com/linux-pam/linux-pam
[man7]: https://www.man7.org/linux/man-pages/man8/pam.8.html
[linux-guides]: https://www.chiark.greenend.org.uk/doc/libpam-doc/html/
[openpam]: https://git.des.dev/OpenPAM/OpenPAM
[manbsd]: https://man.netbsd.org/pam.8
[illumos-pam]: https://code.illumos.org/plugins/gitiles/illumos-gate/+/refs/heads/master/usr/src/lib/libpam/
[manillumos]: https://illumos.org/man/3PAM/pam