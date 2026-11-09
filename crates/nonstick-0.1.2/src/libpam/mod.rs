//! The implementation of the PAM interface that wraps `libpam`.
//!
//! While you're going to want to write PAM modules and applications against
//! the interfaces in [the `handle` module](crate::handle) for testability,
//! this is (probably) what will be used behind the scenes.

#![allow(dead_code)]

mod answer;
mod conversation;
mod environ;
mod handle;
mod items;
mod memory;
#[doc(hidden)]
pub mod module;
mod question;

#[doc(inline)]
pub use handle::{LibPamHandle, LibPamTransaction, TransactionBuilder};
