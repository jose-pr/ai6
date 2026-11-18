//! A request that a Station read an account's usage now, as it waits in the Registry: the gateway
//! reaches no Station, so a person's ask is a record a Station holding the account answers.

use std::io;

use crate::directorates::registry::accounts::{AccountRef, UsageRequest};
use crate::directorates::registry::backend::RegistryBackend;

/// The most characters of a result kept in a request.
const RESULT_CHARS: usize = 300;

/// Records that `by` asked for `account`'s usage at `now`. A request no Station has answered is
/// kept as it is, so a burst of asks is one; the request now on record.
///
/// # Errors
///
/// When the Registry cannot be written.
pub fn request(
    registry: &dyn RegistryBackend,
    account: &AccountRef,
    by: &str,
    now: f64,
) -> io::Result<UsageRequest> {
    let held = registry.change_usage_request(account, &mut |request| {
        if !request.as_ref().is_some_and(UsageRequest::waits) {
            *request = Some(UsageRequest::new(by, now));
        }
    })?;
    held.ok_or_else(|| io::Error::other("the request was not recorded"))
}

/// Records that a Station answered the request made at `asked_at`, with `result`, a line that
/// names no credential. A request made again since is left waiting; times match to the millisecond,
/// since the Registry keeps them as JSON numbers.
///
/// # Errors
///
/// When the Registry cannot be written.
pub fn answer(
    registry: &dyn RegistryBackend,
    account: &AccountRef,
    asked_at: f64,
    done_at: f64,
    result: &str,
) -> io::Result<()> {
    let result: String = result
        .chars()
        .filter(|each| !each.is_control())
        .take(RESULT_CHARS)
        .collect();
    registry.change_usage_request(account, &mut |request| {
        if let Some(request) = request
            && request.waits()
            && (request.at - asked_at).abs() < 0.001
        {
            request.done_at = Some(done_at);
            request.result = Some(result.clone());
        }
    })?;
    Ok(())
}
