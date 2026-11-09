#![allow(clippy::needless_doctest_main)]
//! An enumeration of PAM implementations and tools to detect them.
//!
//! # Configuration
//!
//! When used at compile time, this crate uses the target OS by default,
//! but can be overridden with the `LIBPAMSYS_IMPL` environment variable.
//! See the documentation of [`build_target_impl`] for details.
//!
//! # Detecting PAM
//!
//! ## Build time
//!
//! Use [`enable_pam_impl_cfg`] in your `build.rs` to generate custom `#[cfg]`s
//! for conditional compilation based on PAM implementation.
//!
//! To detect the implementation that will be used at runtime, use the
//! [`build_target_impl`] function.
//!
//! ## Run time
//!
//! The implementation of PAM installed on the machine where the code is running
//! can be detected with [`currently_installed`], or you can use
//! [`os_default`] to see what implementation is used on a given target.

use std::env;
use std::env::VarError;
use std::ffi::c_void;
use std::ptr::NonNull;

/// An enum that knows its own values.
macro_rules! self_aware_enum {
    (
        $(#[$enumeta:meta])*
        $viz:vis enum $name:ident {
            $(
                $(#[$itemeta:meta])*
                $item:ident,
            )*
        }
    ) => {
        $(#[$enumeta])*
        $viz enum $name {
            $(
                $(#[$itemeta])*
                $item,
            )*
        }

        // The implementations in this block are private for now
        // to avoid putting a contract into the public API.
        #[allow(dead_code)]
        impl $name {
            /// Iterator over the items in the enum. For internal use.
            pub(crate) fn items() -> Vec<Self> {
                vec![$(Self::$item),*]
            }

            /// Attempts to parse the enum from the string. For internal use.
            pub(crate) fn try_from(value: &str) -> Result<Self, String> {
                match value {
                    $(stringify!($item) => Ok(Self::$item),)*
                    _ => Err(value.into()),
                }
            }
        }
    };
}

self_aware_enum! {
    /// The PAM implementations supported by `libpam-sys`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub enum PamImpl {
        /// [Linux-PAM] is provided by most Linux distributions.
        ///
        /// [Linux-PAM]: https://github.com/linux-pam/linux-pam
        LinuxPam,
        /// [OpenPAM] is used by most BSDs, including Mac OS X.
        ///
        /// [OpenPAM]: https://git.des.dev/OpenPAM/OpenPAM
        OpenPam,
        /// Illumos and Solaris use a derivative of [Sun's implementation][sun].
        ///
        /// [sun]: https://code.illumos.org/plugins/gitiles/illumos-gate/+/refs/heads/master/usr/src/lib/libpam
        Sun,
        /// Only the functionality and constants in [the PAM spec].
        ///
        /// [the PAM spec]: https://pubs.opengroup.org/onlinepubs/8329799/toc.htm
        XSso,
    }
}

#[allow(clippy::needless_doctest_main)]
/// Generates `cargo` directives for build scripts to enable `cfg(pam_impl)`.
///
/// Print this in your `build.rs` script to be able to use the custom `pam_impl`
/// configuration directive.
///
/// ```
/// // Your package's build.rs:
///
/// fn main() {
///     // Also available at libpam_sys::pam_impl::enable_pam_impl_cfg().
///     libpam_sys_impls::enable_pam_impl_cfg();
///     // whatever else you do in your build script.
/// }
/// ```
///
/// This will set the current `pam_impl` as well as registering all known
/// PAM implementations with `rustc-check-cfg` to get cfg-checking.
///
/// The names that appear in the `cfg` variables are the same as the values
/// in the [`PamImpl`] enum.
///
/// ```ignore
/// #[cfg(pam_impl = "OpenPam")]
/// fn openpam_specific_func(handle: *const libpam_sys::pam_handle) {
///     let environ = libpam_sys::pam_getenvlist(handle);
///     // ...
///     libpam_sys::openpam_free_envlist()
/// }
///
/// // This will give you a warning since "UnknownImpl" is not a known
/// // PAM implementation.
/// #[cfg(not(pam_impl = "UnknownImpl"))]
/// fn do_something() {
///     // ...
/// }
/// ```
pub fn enable_pam_impl_cfg() {
    println!("{}", pam_impl_cfg_string())
}

/// [`enable_pam_impl_cfg`], but returned as a string.
pub fn pam_impl_cfg_string() -> String {
    generate_cfg(build_target_impl())
}

fn generate_cfg(pam_impl: Option<PamImpl>) -> String {
    let impls: Vec<_> = PamImpl::items()
        .into_iter()
        .map(|i| format!(r#""{i:?}""#))
        .collect();
    let mut lines = vec![
        format!(
            "cargo:rustc-check-cfg=cfg(pam_impl, values({impls}))",
            impls = impls.join(",")
        ),
        "cargo:rustc-cfg=pam_impl".into(),
    ];
    if let Some(pam_impl) = pam_impl {
        lines.push("cargo:rustc-cfg=pam_impl".into());
        lines.push(format!("cargo:rustc-cfg=pam_impl=\"{pam_impl:?}\""));
    }
    lines.join("\n")
}

/// The strategy to use to detect PAM.
enum Detect {
    /// Use the default PAM implementation based on the target OS.
    TargetDefault,
    /// Detect the installed implementation.
    Installed,
    /// Use the named version of PAM.
    Specified(PamImpl),
}

const INSTALLED: &str = "__installed__";

/// For `build.rs` use: Detects the PAM implementation that should be used
/// for the target of the currently-running build script.
///
/// # Configuration
///
/// The PAM implementation selected depends upon the value of the
/// `LIBPAMSYS_IMPL` environment variable.
///
/// - Empty or unset (default): Use the default PAM implementation for the
///   Cargo target OS (as specified by `CARGO_CFG_TARGET_OS`).
///   - Linux: Linux-PAM
///   - BSD (and Mac): OpenPAM
///   - Illumos/Solaris: Sun PAM
/// - `__installed__`: Use the PAM implementation installed on the host system.
///   This opens the `libpam` library and looks for specific functions.
/// - The name of a [PamImpl] member: Use that PAM implementation.
///
/// # Panics
///
/// If an unknown PAM implementation is provided in `LIBPAMSYS_IMPL`.
pub fn build_target_impl() -> Option<PamImpl> {
    let detection = match env::var("LIBPAMSYS_IMPL").as_deref() {
        Ok("") | Err(VarError::NotPresent) => Detect::TargetDefault,
        Ok(INSTALLED) => Detect::Installed,
        Ok(val) => Detect::Specified(PamImpl::try_from(val).unwrap_or_else(|_| {
            panic!(
                "unknown PAM implementation {val:?}. \
                valid LIBPAMSYS_IMPL values are {:?}, \
                {INSTALLED:?} to use the currently-installed version, \
                or unset to use the OS default",
                PamImpl::items()
            )
        })),
        Err(other) => panic!("Couldn't detect PAM version: {other}"),
    };
    match detection {
        Detect::TargetDefault => env::var("CARGO_CFG_TARGET_OS")
            .ok()
            .as_deref()
            .and_then(os_default),
        Detect::Installed => currently_installed(),
        Detect::Specified(other) => Some(other),
    }
}

/// Gets the PAM version based on the target OS.
///
/// The target OS name passed in is one of the [Cargo target OS values][os].
///
/// [os]: https://doc.rust-lang.org/reference/conditional-compilation.html#r-cfg.target_os.values
pub fn os_default(target_os: &str) -> Option<PamImpl> {
    match target_os {
        "linux" => Some(PamImpl::LinuxPam),
        "macos" | "freebsd" | "netbsd" | "dragonfly" | "openbsd" => Some(PamImpl::OpenPam),
        "illumos" | "solaris" => Some(PamImpl::Sun),
        _ => None,
    }
}

/// The version of LibPAM installed on this machine (as found by `dlopen`).
pub fn currently_installed() -> Option<PamImpl> {
    LibPam::open().map(|lib| {
        if lib.has(b"pam_syslog\0") {
            PamImpl::LinuxPam
        } else if lib.has(b"_openpam_log\0") {
            PamImpl::OpenPam
        } else if lib.has(b"__pam_get_authtok\0") {
            PamImpl::Sun
        } else {
            PamImpl::XSso
        }
    })
}

struct LibPam(NonNull<c_void>);

impl LibPam {
    fn open() -> Option<Self> {
        let dlopen = |s: &[u8]| unsafe { libc::dlopen(s.as_ptr().cast(), libc::RTLD_LAZY) };
        NonNull::new(dlopen(b"libpam.so\0"))
            .or_else(|| NonNull::new(dlopen(b"libpam.dylib\0")))
            .map(Self)
    }

    fn has(&self, name: &[u8]) -> bool {
        let symbol = unsafe { libc::dlsym(self.0.as_ptr(), name.as_ptr().cast()) };
        !symbol.is_null()
    }
}

impl Drop for LibPam {
    fn drop(&mut self) {
        unsafe {
            libc::dlclose(self.0.as_ptr());
        }
    }
}
