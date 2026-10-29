//! Text checks shared by views and routes.

/// At most `max` bytes, without control characters but newlines.
pub(crate) fn short_within(what: &str, text: &str, max: usize) -> Result<(), String> {
    if text.len() > max || text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!(
            "{what} is longer than {max} bytes or has control characters"
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
