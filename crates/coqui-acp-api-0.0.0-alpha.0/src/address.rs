//! The one check of a plain web address every kind and reader makes.

/// Whether `text` is an `http` or `https` address a provider may be given: the scheme, then something, and nothing in
/// it a header or a log line could be broken by (no whitespace, no control character). Nothing else is judged: the
/// address may be of any length, and where it points is the person's to say.
#[must_use]
pub fn is_http_url(text: &str) -> bool {
    text.strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .is_some_and(|rest| {
            !rest.is_empty() && !rest.chars().any(|c| c.is_whitespace() || c.is_control())
        })
}

#[cfg(test)]
mod tests {
    use super::is_http_url;

    #[test]
    fn an_address_of_any_length_is_taken_and_a_broken_one_is_not() {
        for good in [
            "https://example.com",
            "http://localhost:8080/v1",
            "http://a",
            &format!("https://example.com/{}", "a".repeat(5000)),
        ] {
            assert!(is_http_url(good), "{}", good.len());
        }
        for bad in [
            "",
            "https://",
            "http://",
            "ftp://example.com",
            "example.com",
            "https://exa mple.com",
            "https://example.com/\n",
            "https://example.com/\u{7}",
            " https://example.com",
        ] {
            assert!(!is_http_url(bad), "{bad:?}");
        }
    }
}
