//! Finishing `codex login` from the address the person's browser ended on.
//!
//! `codex login` listens on a port of this machine's loopback and the sign-in page sends the browser back to it. When
//! the browser is on another machine that redirect reaches nothing, so the person pastes the address it ended on and
//! this module requests it on the loopback here, as the browser would have. Only the callback of the login that is
//! waiting is taken: its port, its path and its `state`. The code in the address is single use and bound to the waiting
//! process, and it is in no error, log or answer.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::net::{Ipv4Addr, SocketAddr};
use core::time::Duration;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

/// The path Codex's login server answers the sign-in page's redirect on.
const PATH: &str = "/auth/callback";

/// How long the connection and each read of the answer may take.
const WAIT: Duration = Duration::from_secs(10);

/// The most of an address that is read, in bytes.
const LONGEST: usize = 8192;

/// What the waiting `codex login` expects of its callback, as its sign-in page names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expected {
    host: String,
    port: u16,
    state: String,
}

impl Expected {
    /// Reads the callback out of the sign-in page's address: its `redirect_uri` and `state`. `None` when the page names
    /// no callback on this machine, which is then not one this module relays.
    pub(crate) fn from_page(page: &str) -> Option<Self> {
        let query = page.split_once('?')?.1.split('#').next()?;
        let redirect = value_of(query, "redirect_uri")?;
        let state = value_of(query, "state")?;
        let (host, port, path, _) = split(&redirect)?;
        (path == PATH && !state.is_empty()).then_some(Self { host, port, state })
    }

    /// The port Codex listens on.
    pub(crate) const fn port(&self) -> u16 {
        self.port
    }

    /// Checks that `address` is this login's callback and answers what to request of it, the path and the query.
    ///
    /// # Errors
    ///
    /// Why the address is not the callback, in words that do not repeat it.
    pub(crate) fn accept(&self, address: &str) -> Result<String, String> {
        let address = address.trim();
        if address.is_empty() {
            return Err("no address was given".to_string());
        }
        if address.len() > LONGEST {
            return Err("the address is too long to be the callback".to_string());
        }
        let wanted = format!("http://127.0.0.1:{}{PATH}", self.port);
        let Some((host, port, path, query)) = split(address) else {
            return Err(format!(
                "that is not the address the browser ended on, which starts {wanted}"
            ));
        };
        if host != "127.0.0.1" && host != "localhost" {
            return Err(format!("the address must be on 127.0.0.1, as {wanted}"));
        }
        if port != self.port {
            return Err(format!(
                "the address must be on port {}, the one the sign-in listens on",
                self.port
            ));
        }
        if path != PATH {
            return Err(format!("the address must be the callback, {wanted}"));
        }
        let query = query.unwrap_or_default();
        let mut states = pairs(&query).filter(|(name, _)| name == "state");
        match (states.next(), states.next()) {
            (Some((_, state)), None) if state == self.state => {}
            (None, _) => return Err("the address carries no state".to_string()),
            _ => return Err("the address is from another sign-in (its state differs)".to_string()),
        }
        if !pairs(&query)
            .any(|(name, value)| (name == "code" || name == "error") && !value.is_empty())
        {
            return Err("the address carries no code: sign in on the page first".to_string());
        }
        Ok(format!("{PATH}?{query}"))
    }

    /// Requests `target` (what [`accept`](Self::accept) answered) on this machine's loopback, as the browser would.
    ///
    /// # Errors
    ///
    /// Why it was not delivered; never the address or the code.
    pub(crate) fn relay(&self, target: &str) -> Result<(), String> {
        let at = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let mut stream = TcpStream::connect_timeout(&at, WAIT).map_err(|_| {
            format!(
                "nothing listens on 127.0.0.1:{} here: the sign-in has ended or timed out, so start it again",
                self.port
            )
        })?;
        let _ = stream.set_read_timeout(Some(WAIT));
        let _ = stream.set_write_timeout(Some(WAIT));
        let request = format!(
            "GET {target} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
            self.host, self.port
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|_| "the sign-in did not take the callback".to_string())?;
        let mut line = String::new();
        BufReader::new(stream.take(256))
            .read_line(&mut line)
            .map_err(|_| "the sign-in did not answer the callback in time".to_string())?;
        let status = line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| "the sign-in answered the callback with something else".to_string())?;
        if (200..400).contains(&status) {
            Ok(())
        } else {
            Err(format!("the sign-in refused the callback (HTTP {status})"))
        }
    }
}

/// The scheme-less parts of an `http://` address: host (lower case), port, path and query. `None` for another scheme,
/// a missing port, user information or an address that is not whole.
fn split(address: &str) -> Option<(String, u16, String, Option<String>)> {
    let rest = strip_scheme(address)?;
    let rest = rest.split('#').next()?;
    let (authority, tail) = rest
        .find(['/', '?'])
        .map_or((rest, ""), |at| rest.split_at(at));
    let (host, port) = authority.rsplit_once(':')?;
    if host.is_empty()
        || host.contains(['@', '[', ']'])
        || !port.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    let (path, query) = match tail.split_once('?') {
        Some((path, query)) => (path, Some(query.to_string())),
        None => (tail, None),
    };
    if path.is_empty()
        || query
            .as_deref()
            .is_some_and(|q| q.contains(char::is_whitespace))
    {
        return None;
    }
    Some((host.to_ascii_lowercase(), port, path.to_string(), query))
}

fn strip_scheme(address: &str) -> Option<&str> {
    let head = address.get(..7)?;
    head.eq_ignore_ascii_case("http://")
        .then(|| address.get(7..))
        .flatten()
}

/// The `name=value` pairs of a query, percent-decoded.
fn pairs(query: &str) -> impl Iterator<Item = (String, String)> + '_ {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            (decode(name), decode(value))
        })
}

fn value_of(query: &str, name: &str) -> Option<String> {
    pairs(query).find(|(each, _)| each == name).map(|(_, v)| v)
}

/// Percent-decoding, with `+` as a space, as a query is read.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' if at + 2 < bytes.len()
                && hex(bytes[at + 1]).is_some()
                && hex(bytes[at + 2]).is_some() =>
            {
                out.push(hex(bytes[at + 1]).unwrap_or(0) << 4 | hex(bytes[at + 2]).unwrap_or(0));
                at += 3;
            }
            b'+' => {
                out.push(b' ');
                at += 1;
            }
            other => {
                out.push(other);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://auth.openai.com/oauth/authorize?response_type=code&client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&state=s-1_a";

    fn expected() -> Expected {
        Expected::from_page(PAGE).expect("a callback")
    }

    #[test]
    fn the_page_names_the_port_and_the_state() {
        let e = expected();
        assert_eq!((e.port(), e.state.as_str()), (1455, "s-1_a"));
        assert!(Expected::from_page("https://auth.openai.com/x?state=a").is_none());
        let elsewhere = "https://a/x?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fother&state=a";
        assert!(Expected::from_page(elsewhere).is_none());
    }

    #[test]
    fn the_callback_of_the_waiting_login_is_taken() {
        let e = expected();
        let target = e
            .accept("http://127.0.0.1:1455/auth/callback?code=c&scope=a+b&state=s-1_a")
            .unwrap();
        assert_eq!(target, "/auth/callback?code=c&scope=a+b&state=s-1_a");
        assert!(
            e.accept("  http://localhost:1455/auth/callback?code=c&state=s-1_a\n")
                .is_ok()
        );
    }

    #[test]
    fn anything_else_is_refused_without_repeating_it() {
        let e = expected();
        for bad in [
            "",
            "not a link",
            "https://127.0.0.1:1455/auth/callback?code=c&state=s-1_a",
            "http://evil.example:1455/auth/callback?code=c&state=s-1_a",
            "http://127.0.0.1.evil.example:1455/auth/callback?code=c&state=s-1_a",
            "http://user@127.0.0.1:1455/auth/callback?code=c&state=s-1_a",
            "http://127.0.0.1:1456/auth/callback?code=c&state=s-1_a",
            "http://127.0.0.1/auth/callback?code=c&state=s-1_a",
            "http://127.0.0.1:1455/auth/other?code=c&state=s-1_a",
            "http://127.0.0.1:1455/auth/callback/x?code=c&state=s-1_a",
            "http://127.0.0.1:1455/auth/callback?code=c&state=other",
            "http://127.0.0.1:1455/auth/callback?code=c",
            "http://127.0.0.1:1455/auth/callback?code=c&state=s-1_a&state=s-1_a",
            "http://127.0.0.1:1455/auth/callback?state=s-1_a",
        ] {
            let why = e.accept(bad).expect_err(bad);
            assert!(!why.is_empty());
            assert!(!why.contains("code=c"), "{why}");
        }
    }
}
