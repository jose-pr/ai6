//! A message that carries images to a harness. An Agent Session's messages are text, so a
//! message with images is one text in this form, which only a handler that takes images reads:
//! [`CONTENT_PREFIX`] and then the JSON of a [`MessageContent`]. The Station sends the form only
//! to a handler whose [`AgentHandler::image_input`] says it takes images, and a message whose
//! own text begins with the prefix is always sent in it, so a person's words are never read as
//! the form.
//!
//! [`AgentHandler::image_input`]: super::agent::AgentHandler::image_input

use serde::{Deserialize, Serialize};

/// What begins a message in this form: a NUL, which the app's text never holds, and the form's
/// name.
pub const CONTENT_PREFIX: &str = "\u{0}ai6-content\n";

/// The image formats a message may carry.
pub const IMAGE_MEDIA_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// One image of a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageImage {
    /// One of [`IMAGE_MEDIA_TYPES`].
    pub media_type: String,
    /// The image's bytes as standard base64.
    pub data: String,
}

/// A message's text and the images that go with it, which come first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageContent {
    pub text: String,
    pub images: Vec<MessageImage>,
}

impl MessageContent {
    /// The message as an Agent Session's text.
    pub fn encode(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        format!("{CONTENT_PREFIX}{json}")
    }

    /// The content a message in this form holds; `None` for a plain text, which is a message
    /// of its own.
    pub fn decode(message: &str) -> Option<Self> {
        serde_json::from_str(message.strip_prefix(CONTENT_PREFIX)?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_with_images_reads_back_and_plain_text_is_no_content() {
        let content = MessageContent {
            text: "What is wrong here?".to_owned(),
            images: vec![MessageImage {
                media_type: "image/png".to_owned(),
                data: "iVBORw0KGgo=".to_owned(),
            }],
        };
        assert_eq!(MessageContent::decode(&content.encode()), Some(content));
        assert_eq!(MessageContent::decode("ai6-content\n{}"), None);
        assert_eq!(MessageContent::decode("What is wrong here?"), None);
    }
}
