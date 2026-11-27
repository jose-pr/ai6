//! What the account-side usage readers share: one bounded HTTPS read of a JSON document and the reading of an
//! RFC 3339 time. A reader of one vendor's plan limits builds on these and on [`UsageReport`](crate::UsageReport);
//! none of it knows a harness, a session or a launch.
//!
//! No message here carries a request's address, a header or a body: a credential sent in one never comes back in an
//! error.

use core::fmt;
use core::time::Duration;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// The most of an answer that is read, in bytes.
pub const MAX_BODY_BYTES: u64 = 1 << 20;

/// Why a document could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadError {
    /// The address is not a plain `http` or `https` one.
    BadAddress,
    /// The server could not be reached, or did not answer in time.
    Unreachable,
    /// The server answered this status instead of a document. A redirect is one: it is never followed, since the
    /// credential would go with it.
    Status(u16),
    /// The answer was not a JSON document, or was larger than [`MAX_BODY_BYTES`].
    Malformed,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadAddress => f.write_str("the address is not an http or https one"),
            Self::Unreachable => f.write_str("the server could not be reached"),
            Self::Status(code) => write!(f, "the server answered status {code}"),
            Self::Malformed => f.write_str("the server's answer was not the expected document"),
        }
    }
}

impl core::error::Error for ReadError {}

/// What one `GET` answered: its status, its headers and, when the body was a JSON document, the document.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Exchange {
    /// The status code.
    pub status: u16,
    /// The response headers, names lowercased, in the order sent. A header whose value is not text is left out.
    pub headers: Vec<(String, String)>,
    /// The body when it was a JSON document no larger than [`MAX_BODY_BYTES`], else `None`.
    pub document: Option<Value>,
}

impl Exchange {
    /// The value of the header `name` (any case), the first if it was sent more than once.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(each, _)| each.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// `GET`s `url` carrying `headers`, within `timeout` as a whole, and returns whatever status answered, with its headers:
/// a refusal (a 429 for one) carries the figures a reader wants. Redirects are not followed. The proxy settings of the
/// environment apply, as they do to the program the account belongs to.
///
/// # Errors
///
/// [`ReadError::BadAddress`], [`ReadError::Unreachable`] or [`ReadError::Malformed`]; never a status, which is in the
/// [`Exchange`].
pub fn get_exchange(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<Exchange, ReadError> {
    if !crate::is_http_url(url) {
        return Err(ReadError::BadAddress);
    }
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_global(Some(timeout))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut request = agent.get(url).header("Accept", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.call().map_err(|error| match error {
        ureq::Error::BodyExceedsLimit(_) | ureq::Error::Protocol(_) | ureq::Error::Http(_) => {
            ReadError::Malformed
        }
        _ => ReadError::Unreachable,
    })?;
    let status = response.status().as_u16();
    let sent = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            Some((
                name.as_str().to_ascii_lowercase(),
                value.to_str().ok()?.to_owned(),
            ))
        })
        .collect();
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| ReadError::Unreachable)?;
    let document = if body.len() as u64 > MAX_BODY_BYTES {
        None
    } else {
        serde_json::from_slice(&body).ok()
    };
    Ok(Exchange {
        status,
        headers: sent,
        document,
    })
}

/// Reads the JSON document at `url` with a `GET` carrying `headers`, within `timeout` as a whole. Redirects are not
/// followed. The proxy settings of the environment apply, as they do to the program the account belongs to.
///
/// # Errors
///
/// [`ReadError`], saying nothing of the request but its status.
pub fn get_json(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<Value, ReadError> {
    let exchange = get_exchange(url, headers, timeout)?;
    if exchange.status != 200 {
        return Err(ReadError::Status(exchange.status));
    }
    exchange.document.ok_or(ReadError::Malformed)
}

/// The time now, in Unix seconds.
#[must_use]
pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// A time written as RFC 3339 (`2026-11-16T20:00:00Z`, `2026-11-16T20:00:00.5+00:00`), in Unix seconds; a fraction is
/// dropped. `None` for any other text.
#[must_use]
pub fn rfc3339_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let digits = |from: usize, len: usize| -> Option<i64> {
        let part = bytes.get(from..from + len)?;
        if part.iter().all(u8::is_ascii_digit) {
            core::str::from_utf8(part).ok()?.parse().ok()
        } else {
            None
        }
    };
    let (year, month, day) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    let (hour, minute, second) = (digits(11, 2)?, digits(14, 2)?, digits(17, 2)?);
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't' | b' '))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut at = 19;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return None;
        }
    }
    let offset = match bytes.get(at)? {
        b'Z' | b'z' if at + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') if at + 6 == bytes.len() && bytes.get(at + 3) == Some(&b':') => {
            let minutes = digits(at + 1, 2)? * 60 + digits(at + 4, 2)?;
            if *sign == b'+' { minutes } else { -minutes }
        }
        _ => return None,
    };
    // Days since 1970-01-01 of the civil date.
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rfc3339_times() {
        assert_eq!(rfc3339_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            rfc3339_seconds("2026-11-16T20:00:00+00:00"),
            Some(1_794_859_200)
        );
        assert_eq!(
            rfc3339_seconds("2026-11-16T20:00:00.943648+00:00"),
            Some(1_794_859_200)
        );
        assert_eq!(
            rfc3339_seconds("2026-11-16T22:00:00+02:00"),
            Some(1_794_859_200)
        );
        assert_eq!(rfc3339_seconds("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(rfc3339_seconds("2026-11-16"), None);
        assert_eq!(rfc3339_seconds("2026-13-16T20:00:00Z"), None);
        assert_eq!(rfc3339_seconds("2026-11-16T20:00:00"), None);
        assert_eq!(rfc3339_seconds("not a time at all.."), None);
    }
}
