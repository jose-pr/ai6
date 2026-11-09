//! PEM blocks as the trust store writes them: header lines, then one block with
//! 64-character base64 lines and LF line endings.

/// One PEM block and the header lines before it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Block<'a> {
    /// The lines before `-----BEGIN`, trimmed, in order.
    pub headers: Vec<&'a str>,
    /// The label: `CERTIFICATE` in `-----BEGIN CERTIFICATE-----`.
    pub label: String,
    /// The decoded bytes.
    pub bytes: Vec<u8>,
}

/// Encodes `bytes` as a PEM block labelled `label`, with 64-character lines and LF line
/// endings, ending in a newline.
pub fn encode(label: &str, bytes: &[u8]) -> String {
    let encoded = data_encoding::BASE64.encode(bytes);
    let mut pem = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(core::str::from_utf8(chunk).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str(&format!("-----END {label}-----\n"));
    pem
}

/// The first PEM block in `text` and the header lines before it; `None` when there is no
/// complete block or its body is not base64.
pub fn decode(text: &str) -> Option<Block<'_>> {
    let mut headers = Vec::new();
    let mut lines = text.lines();
    let label = loop {
        let line = lines.next()?.trim();
        if let Some(label) = line
            .strip_prefix("-----BEGIN ")
            .and_then(|rest| rest.strip_suffix("-----"))
        {
            break label.to_owned();
        }
        headers.push(line);
    };
    let end = format!("-----END {label}-----");
    let mut body = String::new();
    loop {
        let line = lines.next()?.trim();
        if line == end {
            break;
        }
        body.push_str(line);
    }
    let bytes = data_encoding::BASE64.decode(body.as_bytes()).ok()?;
    Some(Block {
        headers,
        label,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_round_trips_with_its_headers() {
        let pem = format!("Method: tls\n{}", encode("CERTIFICATE", &[7; 100]));
        let block = decode(&pem).expect("a block");
        assert_eq!(block.headers, ["Method: tls"]);
        assert_eq!(block.label, "CERTIFICATE");
        assert_eq!(block.bytes, vec![7; 100]);
        assert!(pem.lines().all(|line| line.len() <= 64));
        assert!(decode("-----BEGIN X-----\nAAAA\n").is_none());
    }
}
