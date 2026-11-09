//! `libpam-sys` provides low-level access to LibPAM.
//!
//! Everything in here is directly as exported from the LibPAM library or
//! its header files, with two exceptions:
//!
//! - The [`pam_impl`] submodule (and the associated [`pam_impl_name!`] macro),
//!   which can be used to detect the current PAM implementation.
//! - The [`aliases`] submodule, which contains convenient aliases
//!   for callback types used in libpam, so you don't have to type
//!   `unsafe extern "C" fn(this is so long)` all the time.
#![doc = ""]
#![doc = concat!("This documentation was built for the **", env!("LIBPAMSYS_IMPL"), "** implementation.")]
//!
//! You can override this **at build time** by setting the `LIBPAMSYS_IMPL`
//! environment variable to one of the values of the [`pam_impl::PamImpl`] enum.
//! For more information about configuration, see [the documentation of
//! libpam-sys-impls](libpam_sys_impls::build_target_impl).
#![allow(non_camel_case_types)]
#![allow(unused_imports)]

pub mod aliases;
mod constants;
mod ffi;
#[doc(inline)]
pub use crate::{constants::*, ffi::*};

/// Information about the current PAM implementation (or the implementation
/// that is being built for).
pub mod pam_impl {
    #[doc(inline)]
    pub use libpam_sys_impls::{enable_pam_impl_cfg, pam_impl_cfg_string, PamImpl};

    include!(concat!(env!("OUT_DIR"), "/pam_impl_consts.rs"));
}
