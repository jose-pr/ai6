//! Text checks shared by views and routes.

/// The longest short text in a view, in bytes.
const MAX_SHORT: usize = 512;

/// At most [`MAX_SHORT`] bytes, without control characters but newlines.
pub(crate) fn short(what: &str, text: &str) -> Result<(), String> {
    if text.len() > MAX_SHORT || text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!(
            "{what} is longer than {MAX_SHORT} bytes or has control characters"
        ));
    }
    Ok(())
}

/// An identifier as AI6 records it: 1 to 256 bytes without control characters.
pub(crate) fn identifier(what: &str, text: &str) -> Result<(), String> {
    if ai6::directorates::section_00::assistant::valid_identity(text) {
        Ok(())
    } else {
        Err(format!("{what} is not an identifier"))
    }
}
