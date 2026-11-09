//! PAM logging variables and macros.
//!
//! PAM implementations usually include the ability to log to syslog in a way
//! that is associated with the log entry itself. This module defines
//! the interface we use for logging.
//!
//! We can't use [the `log` crate](https://docs.rs/log) because that requires
//! that any `Log` implementors be `Sync` and `Send`, and a PAM handle
//! may be neither. Furthermore, PAM handles are passed to PAM modules in
//! dynamic libraries, and `log` doesn't work across dynamic linking boundaries.
//!
//! A `PamShared` implementation may still use the `log` crate on the backend,
//! and may even itself implement `log::Log`, but that interface is not exposed
//! to the generic PAM user.

use crate::_doc::{man7, manbsd};
use std::fmt;

/// A trait for logging.
pub trait Logger {
    /// Logs something via this PAM handle.
    ///
    /// You probably want to use one of the logging macros,
    /// like [`error!`](crate::error!),
    /// [`warn!`](crate::warn!),
    /// [`info!`](crate::info!),
    /// or [`debug!`](crate::debug!).
    ///
    /// In most PAM implementations, this will go to syslog.
    /// See [Linux-PAM's `pam_syslog`][man7] or
    /// [OpenPAM's `openpam_log`][manbsd] for more details.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nonstick::PamShared;
    /// use nonstick::location;
    /// use nonstick::logging::Level;
    /// # fn _test(pam_hdl: impl PamShared) {
    /// # let delay_ms = 100;
    /// # let url = "https://zombo.com";
    /// // Usually, instead of calling this manually, just use the macros.
    /// nonstick::error!(pam_hdl, "something bad happened!");
    /// nonstick::warn!(pam_hdl, "loading information took {delay_ms} ms");
    /// nonstick::info!(pam_hdl, "using network backend");
    /// nonstick::debug!(pam_hdl, "sending GET request to {url}");
    /// // But if you really want to, you can call this yourself:
    /// pam_hdl.log(
    ///     Level::Warn,
    ///     location!(),
    ///     format_args!("this is unnecessarily verbose"),
    /// );
    /// # }
    /// ```
    #[doc = man7!(3 pam_syslog)]
    #[doc = manbsd!(3 openpam_log)]
    fn log(&self, level: Level, loc: Location<'_>, entry: fmt::Arguments);
}

/// An entry to be added to the log.
///
/// The levels are in descending order of importance and correspond roughly
/// to the similarly-named levels in the `log` crate.
///
/// Their values are ordered monotonically, either increasing or decreasing,
/// depending upon the implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

/// The location of a log entry. Use [`location!`](crate::location!) to create this.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct Location<'a> {
    pub file: &'a str,
    pub line: u32,
    pub function: &'a str,
}

impl<'a> Location<'a> {
    /// Creates a new location. Just use [`location!`](crate::location!) instead.
    pub fn new(file: &'a str, line: u32, function: &'a str) -> Self {
        Self {
            file,
            line,
            function,
        }
    }
}

/// The [`Location`] where this macro is inserted.
#[doc(hidden)]
#[macro_export]
macro_rules! location {
    () => {
        $crate::logging::Location::new(file!(), line!(), $crate::__function!())
    };
}

/// Here's the guts of the logger thingy. You shouldn't be using this!
#[doc(hidden)]
#[macro_export]
macro_rules! __log_internal {
    ($handle:expr, $level:ident, $($arg:tt)+) => {
        $handle.log($crate::logging::Level::$level, $crate::location!(), format_args!($($arg)+));
    }
}

/// Ugly, hacky macro to get the current function name.
///
/// [Stolen from Stack Overflow.][https://stackoverflow.com/a/40234666/39808]
#[doc(hidden)]
#[macro_export]
macro_rules! __function {
    () => {{
        fn p() {}
        fn f<T>(_: T) -> &'static str {
            ::std::any::type_name::<T>()
        }
        f(p).trim_end_matches("::p")
    }};
}

/// Logs a message at error level via the given PAM handle.
///
/// This supports `format!`-style formatting.
///
/// # Example
///
/// ```no_run
/// # fn _test(pam_handle: impl nonstick::PamShared) {
/// # let load_error = "xxx";
/// nonstick::error!(
///     pam_handle,
///     "error loading data from data source: {load_error}"
/// );
/// // Will log a message like "error loading data from data source: timed out"
/// // at ERROR level on syslog.
/// # }
/// ```
#[macro_export]
macro_rules! error { ($handle:expr, $($arg:tt)+) => { $crate::__log_internal!($handle, Error, $($arg)+);}}

/// Logs a message at warning level via the given PAM handle.
///
/// This supports `format!`-style formatting.
///
/// # Example
///
/// ```no_run
/// # fn _test(pam_handle: impl nonstick::PamShared) {
/// # let (start, finish) = (0, 0);
/// nonstick::warn!(
///     pam_handle,
///     "loading took too long: {latency_ms} ms",
///     latency_ms = start - finish
/// );
/// // Will log a message like "loading took too long: 495 ms"
/// // at WARN level on syslog.
/// # }
/// ```
#[macro_export]
macro_rules! warn { ($handle:expr, $($arg:tt)+) => { $crate::__log_internal!($handle, Warn, $($arg)+);}}

/// Logs a message at info level via the given PAM handle.
///
/// This supports `format!`-style formatting.
///
/// # Example
///
/// ```no_run
/// # fn _test(pam_handle: impl nonstick::PamShared) {
/// nonstick::info!(pam_handle, "using remote backend to load user data");
/// // Will log a message like "using remote backend to load user data"
/// // at INFO level on syslog.
/// # }
/// ```
#[macro_export]
macro_rules! info { ($handle:expr, $($arg:tt)+) => { $crate::__log_internal!($handle, Info, $($arg)+);}}

/// Logs a message at debug level via the given PAM handle.
///
/// This level specially includes file/line/column information.
/// This supports `format!`-style formatting.
///
/// # Example
///
/// ```no_run
/// # fn _test(pam_handle: impl nonstick::PamShared) {
/// # let userinfo_url = "https://zombo.com/";
/// nonstick::debug!(pam_handle, "making HTTP GET request to {userinfo_url}");
/// // Will log a message like
/// //     "making HTTP GET request to https://zombo.com/"
/// // at DEBUG level on syslog.
/// # }
/// ```
#[macro_export]
macro_rules! debug { ($handle:expr, $($arg:tt)+) => { $crate::__log_internal!($handle, Debug, $($arg)+);}}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fmt;

    #[test]
    fn test_logging() {
        struct TestLog(RefCell<Vec<(Level, String)>>);

        impl Logger for TestLog {
            fn log(&self, level: Level, _: Location<'_>, text: fmt::Arguments) {
                self.0.borrow_mut().push((level, text.to_string()))
            }
        }

        let logger = TestLog(Default::default());

        let something = Level::Error;
        error!(logger, "here is another thing: {}", 99);
        warn!(logger, "watch out!");
        info!(logger, "here is some info: {info}", info = "information");
        debug!(logger, "here is something: {something:?}");

        let logged = logger.0.into_inner();

        assert_eq!(
            vec![
                (Level::Error, "here is another thing: 99".to_owned()),
                (Level::Warn, "watch out!".to_owned()),
                (Level::Info, "here is some info: information".to_owned()),
                (Level::Debug, "here is something: Error".to_owned()),
            ],
            logged
        );
    }
}
