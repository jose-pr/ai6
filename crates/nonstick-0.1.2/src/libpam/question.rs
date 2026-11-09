//! Data and types dealing with PAM messages.

use crate::conv::{ErrorMsg, Exchange, InfoMsg, MaskedQAndA, QAndA};
use crate::libpam::conversation::OwnedExchange;
use crate::libpam::memory;
use crate::ErrorCode;
use crate::Result;
use libpam_sys_helpers;
use std::ffi::{c_int, c_void, CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::ptr::NonNull;

memory::num_enum! {
    /// The C enum values for messages shown to the user.
    enum Style {
        /// Requests information from the user; will be masked when typing.
        PromptEchoOff = libpam_sys::PAM_PROMPT_ECHO_OFF,
        /// Requests information from the user; will not be masked.
        PromptEchoOn = libpam_sys::PAM_PROMPT_ECHO_ON,
        /// An error message.
        ErrorMsg = libpam_sys::PAM_ERROR_MSG,
        /// An informational message.
        TextInfo = libpam_sys::PAM_TEXT_INFO,
        /// Yes/No/Maybe conditionals. A Linux-PAM extension.
        #[cfg(feature = "linux-pam-ext")]
        RadioType = libpam_sys::PAM_RADIO_TYPE,
        /// For server–client non-human interaction.
        ///
        /// NOT part of the X/Open PAM specification.
        /// A Linux-PAM extension.
        #[cfg(feature = "linux-pam-ext")]
        BinaryPrompt = libpam_sys::PAM_BINARY_PROMPT,
    }
}

/// A question sent by PAM or a module to an application.
///
/// PAM refers to this as a "message", but we call it a question
/// to avoid confusion.
///
/// This question, and its internal data, is owned by its creator
/// (either the module or PAM itself).
#[repr(C)]
#[derive(Debug)]
pub struct Question {
    /// The style of message to request.
    pub style: c_int,
    /// A description of the data requested.
    ///
    /// For most requests, this will be an owned [`CStr`],
    /// but for requests with style `PAM_BINARY_PROMPT`,
    /// this will be `CBinaryData` (a Linux-PAM extension).
    pub data: Option<NonNull<c_void>>,
}

impl Question {
    /// Gets this message's data pointer as a string.
    ///
    /// # Safety
    ///
    /// It's up to you to pass this only on types with a string value.
    unsafe fn string_data(&self) -> &OsStr {
        match self.data.as_ref() {
            None => "".as_ref(),
            Some(data) => OsStr::from_bytes(CStr::from_ptr(data.as_ptr().cast()).to_bytes()),
        }
    }

    /// Gets this message's data pointer as borrowed binary data.
    unsafe fn binary_data(&self) -> (&[u8], u8) {
        self.data
            .as_ref()
            .map(|data| libpam_sys_helpers::BinaryPayload::contents(data.as_ptr().cast()))
            .unwrap_or_default()
    }
}

impl TryFrom<&Exchange<'_>> for Question {
    type Error = ErrorCode;
    fn try_from(msg: &Exchange) -> Result<Self> {
        let alloc = |style, text: &OsStr| -> Result<_> {
            Ok((style, unsafe {
                memory::CHeapBox::cast(memory::CHeapString::new(text.as_bytes()).into_box())
            }))
        };
        // We will only allocate heap data if we have a valid input.
        let (style, data): (_, memory::CHeapBox<c_void>) = match *msg {
            Exchange::MaskedPrompt(p) => alloc(Style::PromptEchoOff, p.question()),
            Exchange::Prompt(p) => alloc(Style::PromptEchoOn, p.question()),
            Exchange::Error(p) => alloc(Style::ErrorMsg, p.question()),
            Exchange::Info(p) => alloc(Style::TextInfo, p.question()),
            #[cfg(feature = "linux-pam-ext")]
            Exchange::RadioPrompt(p) => alloc(Style::RadioType, p.question()),
            #[cfg(feature = "linux-pam-ext")]
            Exchange::BinaryPrompt(p) => {
                let (data, typ) = p.question();
                let payload = memory::CHeapPayload::new(data, typ)?.into_inner();
                Ok((Style::BinaryPrompt, unsafe {
                    memory::CHeapBox::cast(payload)
                }))
            }
            #[cfg(not(feature = "linux-pam-ext"))]
            Exchange::RadioPrompt(_) | Exchange::BinaryPrompt(_) => {
                Err(ErrorCode::ConversationError)
            }
        }?;
        Ok(Self {
            style: style.into(),
            data: Some(memory::CHeapBox::into_ptr(data)),
        })
    }
}

impl Drop for Question {
    fn drop(&mut self) {
        // SAFETY: We either created this data or we got it from PAM.
        // After this function is done, it will be zeroed out.
        unsafe {
            // This is nice-to-have. We'll try to zero out the data
            // in the Question. If it's not a supported format, we skip it.
            if let Ok(style) = Style::try_from(self.style) {
                let _ = match style {
                    #[cfg(feature = "linux-pam-ext")]
                    Style::BinaryPrompt => self
                        .data
                        .as_mut()
                        .map(|p| libpam_sys_helpers::BinaryPayload::zero(p.as_ptr().cast())),
                    #[cfg(feature = "linux-pam-ext")]
                    Style::RadioType => self
                        .data
                        .as_mut()
                        .map(|p| memory::CHeapString::zero(p.cast())),
                    Style::TextInfo
                    | Style::ErrorMsg
                    | Style::PromptEchoOff
                    | Style::PromptEchoOn => self
                        .data
                        .as_mut()
                        .map(|p| memory::CHeapString::zero(p.cast())),
                };
            };
            let _ = self.data.map(|p| memory::CHeapBox::from_ptr(p));
        }
    }
}

impl<'a> TryFrom<&'a Question> for OwnedExchange<'a> {
    type Error = ErrorCode;
    fn try_from(question: &'a Question) -> Result<Self> {
        let style: Style = question
            .style
            .try_into()
            .map_err(|_| ErrorCode::ConversationError)?;
        // SAFETY: In all cases below, we're creating questions based on
        // known types that we get from PAM and the inner types it should have.
        let prompt = unsafe {
            match style {
                Style::PromptEchoOff => {
                    Self::MaskedPrompt(MaskedQAndA::new(question.string_data()))
                }
                Style::PromptEchoOn => Self::Prompt(QAndA::new(question.string_data())),
                Style::ErrorMsg => Self::Error(ErrorMsg::new(question.string_data())),
                Style::TextInfo => Self::Info(InfoMsg::new(question.string_data())),
                #[cfg(feature = "linux-pam-ext")]
                Style::RadioType => {
                    Self::RadioPrompt(crate::conv::RadioQAndA::new(question.string_data()))
                }
                #[cfg(feature = "linux-pam-ext")]
                Style::BinaryPrompt => {
                    Self::BinaryPrompt(crate::conv::BinaryQAndA::new(question.binary_data()))
                }
            }
        };
        Ok(prompt)
    }
}

#[cfg(feature = "linux-pam-ext")]
impl From<libpam_sys_helpers::TooBigError> for ErrorCode {
    fn from(_: libpam_sys_helpers::TooBigError) -> Self {
        ErrorCode::BufferError
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_matches {
        (($variant:path, $q:expr), $input:expr) => {
            let input = $input;
            let exc = input.exchange();
            if let $variant(msg) = exc {
                assert_eq!($q, msg.question());
            } else {
                panic!(
                    "want enum variant {v}, got {exc:?}",
                    v = stringify!($variant)
                );
            }
        };
    }

    // TODO: Test TryFrom<Exchange> for Question/OwnedQuestion.

    #[test]
    fn standard() {
        assert_matches!(
            (Exchange::MaskedPrompt, "hocus pocus"),
            MaskedQAndA::new("hocus pocus".as_ref())
        );
        assert_matches!((Exchange::Prompt, "what"), QAndA::new("what".as_ref()));
        assert_matches!((Exchange::Prompt, "who"), QAndA::new("who".as_ref()));
        assert_matches!((Exchange::Info, "hey"), InfoMsg::new("hey".as_ref()));
        assert_matches!((Exchange::Error, "gasp"), ErrorMsg::new("gasp".as_ref()));
    }

    #[test]
    #[cfg(feature = "linux-pam-ext")]
    fn linux_extensions() {
        use crate::conv::{BinaryQAndA, RadioQAndA};
        assert_matches!(
            (Exchange::BinaryPrompt, (&[5, 4, 3, 2, 1][..], 66)),
            BinaryQAndA::new((&[5, 4, 3, 2, 1], 66))
        );
        assert_matches!(
            (Exchange::RadioPrompt, "you must choose"),
            RadioQAndA::new("you must choose".as_ref())
        );
    }
}
