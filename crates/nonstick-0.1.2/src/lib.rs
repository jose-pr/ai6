//! 🍳 A safe, nonstick interface to
//! the Pluggable Authentication Modules framework.
//!
//! Nonstick provides a fully type- and memory-safe interface to
//! all implementations of PAM, both for PAM modules and PAM applications.
//!
//! # Usage
//!
//! nonstick can be used on either side of a PAM transaction,
//! both to implement an application which calls into PAM,
//! or a module which implements a PAM backend.
//!
//! For more information about how PAM works in general, or more pointers
//! on how to implement a PAM module or application, see the
//! [References](#references) section.
//!
//! ## PAM Application
//!
//! To implement a PAM application, first implement a [`Conversation`],
//! then build a [`Transaction`] with the [`TransactionBuilder`].
//! This can be built into any standard Rust library or binary.
//!
//! ```
//! use nonstick::{
//!     AuthnFlags, Conversation, ConversationAdapter, Result as PamResult, Transaction,
//!     TransactionBuilder,
//! };
//! use std::ffi::{OsStr, OsString};
//!
//! /// A basic Conversation that assumes that any "regular" prompt is for
//! /// the username, and that any "masked" prompt is for the password.
//! ///
//! /// A typical Conversation will provide the user with an interface
//! /// to interact with PAM, e.g. a dialogue box or a terminal prompt.
//! struct UsernamePassConvo {
//!     username: String,
//!     password: String,
//! }
//!
//! // ConversationAdapter is a convenience wrapper for the common case
//! // of only handling one request at a time.
//! impl ConversationAdapter for UsernamePassConvo {
//!     fn prompt(&self, request: impl AsRef<OsStr>) -> PamResult<OsString> {
//!         Ok(OsString::from(&self.username))
//!     }
//!
//!     fn masked_prompt(&self, request: impl AsRef<OsStr>) -> PamResult<OsString> {
//!         Ok(OsString::from(&self.password))
//!     }
//!
//!     fn error_msg(&self, message: impl AsRef<OsStr>) {
//!         // Normally you would want to display this to the user somehow.
//!         // In this case, we're just ignoring it.
//!     }
//!
//!     fn info_msg(&self, message: impl AsRef<OsStr>) {
//!         // ibid.
//!     }
//! }
//!
//! fn authenticate(username: &str, password: &str) -> PamResult<()> {
//!     let user_pass = UsernamePassConvo {
//!         username: username.into(),
//!         password: password.into(),
//!     };
//!
//!     let mut txn = TransactionBuilder::new_with_service("cortex-sso")
//!         .username(username)
//!         .build(user_pass.into_conversation())?;
//!     // If authentication fails, this will return an error.
//!     // We immediately give up rather than re-prompting the user.
//!     txn.authenticate(AuthnFlags::empty())?;
//!     txn.account_management(AuthnFlags::empty())?;
//!     Ok(())
//! }
//! ```
//!
//! PAM just tells you that the user is, in fact, who they say they are.
//! It is up to your application to choose what to do with that information.
//!
//! ## PAM module
//!
//! PAM modules are implemented as dynamic libraries loaded into
//! the address space of the calling application. To implement a module,
//! create a `dylib` crate and implement a [`PamModule`], and export it
//! using the [`pam_export!`] macro.
//!
//! ```toml
//! # Your Cargo.toml
//! [package]
//! name = "samename"
//! description = "Checks that username and password are the same"
//! # ...
//!
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! nonstick = "0.1"
//! # ...
//! ```
//!
//! Once you've set up the dylib crate and added `nonstick` as a dependency,
//! you can write the code itself:
//!
//! ```
//! // Your lib.rs
//!
//! use nonstick::{
//!     pam_export, AuthnFlags, ErrorCode, ModuleClient, PamModule, Result as PamResult,
//! };
//! use std::ffi::CStr;
//!
//! # // This needs to be here to make this doc example work.
//! # fn main() {}
//!
//! /// A module that only allows you to log in if your username
//! /// is the same as your password.
//! struct SameName;
//! pam_export!(SameName);
//!
//! impl<M: ModuleClient> PamModule<M> for SameName {
//!     fn authenticate(handle: &mut M, _args: Vec<&CStr>, _flags: AuthnFlags) -> PamResult<()> {
//!         // Using `None` as the prompt parameter here will tell PAM
//!         // to use the default prompt.
//!         let username = handle.username(None)?;
//!         let password = handle.authtok(None)?;
//!         if username == password {
//!             Ok(())
//!         } else {
//!             Err(ErrorCode::AuthenticationError)
//!         }
//!     }
//!
//!     // You can implement other methods of PamModule to provide additional
//!     // features.
//! }
//! ```
//!
//! This gets built into a shared object. By installing this into the PAM
//! library directory at a place like `pam_samename.so` and configuring PAM
//! to use it in the authentication stack (beyond the scope of this
//! documentation), it will be used to authenticate users.
//!
//! # Configuration
//!
//! There are a few different PAM implementations available. By default,
//! nonstick detects which implementation it should use for the current target.
//! If you need to choose a different implementation, set the `LIBPAMSYS_IMPL`
//! environment variable at build time. See the [`libpam_sys`] documentation.
#![doc = ""]
#![doc = concat!("This documentation was built for **", pam_impl_name!(), "**.")]
//!
//! # Cargo features
//!
//! This crate provides the following Cargo features:
//!
//! - **link** (enabled by default): Actually link against PAM,
//!   rather than just providing an abstract PAM interface.
//!   Enabling this will fail if extensions incompatible with the
//!   PAM implementation you're linking against are also enabled.
//! - Extensions beyond the PAM specification provided by various PAM
//!   implementations:
//!   - **basic-ext**: Enable extensions provided by both Linux-PAM and OpenPAM.
//!     This is limited to a few return enums.
//!   - **linux-pam-ext** (includes basic-ext): Enable extensions provided by
//!     Linux-PAM. This includes enum values and the ability to send
//!     binary messages between the PAM module and the application.
//!   - **openpam-ext** (includes basic-ext): Enable extensions provided by
//!     OpenPAM. This includes enum values.
//!   - **sun-ext**: Enable extensions provided by Sun PAM.
//!     This includes enum values.
//!
//! # Design
//!
//! This library consists of two parts:
//!
//! - The generic PAM interface, a set of traits describing the behavior of PAM
//!   and the API we export. It is independent of the PAM library itself and
//!   could be implemented by any crate to provide PAM-like services.
//!   This is primarily intended to allow a developer to test their PAM modules
//!   and applications by writing mock implementations to verify their
//!   application (or module) code's interactions with PAM itself.
//!
//! - The bindings to LibPAM itself. This part is included only when **link**
//!   is enabled. These live in the `libpam` submodule (with a few exceptions
//!   for constant-related code).
//!
//! # References
//!
//! These documents were used when authoring this library and will probably be
//! of value if you want to implement a PAM module or a PAM application.
//!
//! - The Linux-PAM guides provide information for a variety of audiences.
//!   While some of it is specific to Linux-PAM, much of it applies to other
//!   PAM implementations:
//!   - [Application Developers' Guide][adg]
//!   - [Module Writers' Guide][mwg]
//!   - [System Administrators' Guide][sag]
//! - PAM framework man pages for developers:
//!   - [Linux-PAM developer man page][man7]
//!   - [OpenPAM developer man page][manbsd]
//!   - [Illumos PAM developer man page][mansun]
//! - PAM framework man pages for system administrators:
//!   - [Linux-PAM admin documentation][man7pam8]
//!   - [OpenPAM admin documentation][bsdpam8]
//!   - [Illumos pam.conf documentation][sunpam5]
//! - [The original PAM specification][spec] (mostly of historical interest)
//! - [Wikipedia: Cooking spray][spray]
//!
//! [spray]: https://en.wikipedia.org/wiki/Cooking_spray
#![doc = _doc::man7!(man7pam8: 8 pam)]
#![doc = _doc::manbsd!(bsdpam8: 8 pam)]
#![doc = _doc::mansun!(sunpam5: 5 "pam.conf")]
#![doc = _doc::stdlinks!(3 pam)]
#![doc = _doc::guide!(adg: "Linux-PAM_ADG.html")]
#![doc = _doc::guide!(mwg: "Linux-PAM_MWG.html")]
#![doc = _doc::guide!(sag: "Linux-PAM_SAG.html")]
#![doc = _doc::xsso!(spec: "toc.htm")]

#[cfg(feature = "link")]
mod _compat_checker {
    macro_rules! feature_check {
        ($feature:literal, pam_impl = ($($pimpl:literal),*)) => {
            #[cfg(all(feature = $feature, not(any($(pam_impl = $pimpl),*))))]
            compile_error!(
                concat!(
                    "The feature '", $feature, "' is only available ",
                    "with these PAM implementations:\n",
                    $("- ", $pimpl, "\n"),*,
                    "The current PAM implementation is:\n\n",
                    "  ", libpam_sys::pam_impl_name!(), "\n\n",
                    "Set the 'LIBPAMSYS_IMPL' environment variable to one of ",
                    "the above PAM implementation names to build ",
                    "for that implementation of PAM."
                )
            );
        }
    }
    feature_check!("linux-pam-ext", pam_impl = ("LinuxPam"));
    feature_check!("basic-ext", pam_impl = ("LinuxPam", "OpenPam"));
    feature_check!("openpam-ext", pam_impl = ("OpenPam"));
    feature_check!("sun-ext", pam_impl = ("Sun"));
}

pub mod constants;
pub mod conv;
pub mod module;

pub mod handle;

mod _doc;
mod environ;
pub mod items;
#[cfg(feature = "link")]
pub mod libpam;
pub mod logging;

#[cfg(feature = "link")]
#[doc(inline)]
pub use crate::libpam::{LibPamHandle, LibPamTransaction, TransactionBuilder};
#[doc(inline)]
pub use crate::{
    constants::{
        AuthnFlags, AuthtokAction, AuthtokFlags, BaseFlags, CredAction, ErrorCode, Result,
    },
    conv::{BinaryData, Conversation, ConversationAdapter},
    environ::{EnvironMap, EnvironMapMut},
    handle::{ModuleClient, PamShared, Transaction},
    module::PamModule,
};
#[doc(inline)]
pub use libpam_sys::pam_impl;
use libpam_sys::pam_impl_name;
