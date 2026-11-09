use crate::constants::ReturnCode;
use crate::conv::{BinaryQAndA, RadioQAndA};
use crate::conv::{Conversation, ErrorMsg, Exchange, InfoMsg, MaskedQAndA, QAndA};
use crate::libpam::answer::BinaryAnswer;
use crate::libpam::answer::{Answer, Answers, TextAnswer};
use crate::libpam::question::Question;
use crate::ErrorCode;
use crate::Result;
use libpam_sys::aliases::ConversationCallback;
use libpam_sys_helpers::PtrPtrVec;
use std::ffi::{c_int, c_void};
use std::iter;
use std::ptr::NonNull;
use std::result::Result as StdResult;

/// The type used by PAM to call back into a conversation.
///
/// This has the same structure as a [`libpam_sys::pam_conv`].
#[derive(Debug)]
#[repr(C)]
pub struct OwnedConversation<C: Conversation> {
    callback: ConversationCallback,
    conv: Box<C>,
}

impl<C: Conversation> OwnedConversation<C> {
    pub fn new(conv: C) -> Self {
        Self {
            callback: Self::wrapper_callback,
            conv: Box::new(conv),
        }
    }

    /// Passed as the conversation function into PAM for an owned handle.
    ///
    /// PAM calls this, we compute answers, then send them back.
    unsafe extern "C" fn wrapper_callback(
        count: c_int,
        questions: *const *const libpam_sys::pam_message,
        answers: *mut *mut libpam_sys::pam_response,
        me: *mut c_void,
    ) -> c_int {
        let internal = || {
            // Collect all our pointers
            let conv = me
                .cast::<C>()
                .as_ref()
                .ok_or(ErrorCode::ConversationError)?;
            let q_iter = PtrPtrVec::<Question>::iter_over(questions, count as usize);
            let answers_ptr = answers.as_mut().ok_or(ErrorCode::ConversationError)?;

            // Build our owned list of Q&As from the questions we've been asked
            let messages: Vec<OwnedExchange> = q_iter
                .map(TryInto::try_into)
                .collect::<Result<_>>()
                .map_err(|_| ErrorCode::ConversationError)?;
            // Borrow all those Q&As and ask them.
            // If we got an invalid message type, bail before sending.
            let borrowed: Result<Vec<_>> = messages.iter().map(Exchange::try_from).collect();
            // TODO: Do we want to log something here?
            conv.communicate(&borrowed?);

            // Send our answers back.
            let owned = Answers::build(messages)?;
            *answers_ptr = owned.into_ptr();
            Ok(())
        };
        ReturnCode::from(internal()).into()
    }
}

/// A conversation owned by a PAM handle and lent to us.
#[derive(Debug)]
pub struct PamConv(libpam_sys::pam_conv);

impl Conversation for PamConv {
    fn communicate(&self, messages: &[Exchange]) {
        let internal = || {
            let questions: Result<_> = messages.iter().map(Question::try_from).collect();
            let questions = PtrPtrVec::new(questions?);
            let mut response_pointer = std::ptr::null_mut();
            // SAFETY: We're calling into PAM with valid everything.
            let result = unsafe {
                (self.0.conv)(
                    messages.len() as c_int,
                    questions.as_ptr(),
                    &mut response_pointer,
                    self.0.appdata_ptr,
                )
            };
            ErrorCode::result_from(result)?;
            // SAFETY: This is a pointer we just got back from PAM.
            // We have to trust that the responses from PAM match up
            // with the questions we sent.
            unsafe {
                let response_pointer =
                    NonNull::new(response_pointer).ok_or(ErrorCode::ConversationError)?;
                let mut owned_responses = Answers::from_c_heap(response_pointer, messages.len());
                for (msg, response) in iter::zip(messages, owned_responses.iter_mut()) {
                    convert(msg, response);
                }
            };
            Ok(())
        };
        if let Err(e) = internal() {
            messages.iter().for_each(|m| m.set_error(e))
        }
    }
}

/// Like [`Exchange`], but this time we own the contents.
#[derive(Debug)]
pub enum OwnedExchange<'a> {
    MaskedPrompt(MaskedQAndA<'a>),
    Prompt(QAndA<'a>),
    Info(InfoMsg<'a>),
    Error(ErrorMsg<'a>),
    RadioPrompt(RadioQAndA<'a>),
    BinaryPrompt(BinaryQAndA<'a>),
}

impl<'a> TryFrom<&'a OwnedExchange<'a>> for Exchange<'a> {
    type Error = ErrorCode;
    fn try_from(src: &'a OwnedExchange) -> StdResult<Self, ErrorCode> {
        match src {
            OwnedExchange::MaskedPrompt(m) => Ok(Exchange::MaskedPrompt(m)),
            OwnedExchange::Prompt(m) => Ok(Exchange::Prompt(m)),
            OwnedExchange::Info(m) => Ok(Exchange::Info(m)),
            OwnedExchange::Error(m) => Ok(Exchange::Error(m)),
            OwnedExchange::RadioPrompt(m) => Ok(Exchange::RadioPrompt(m)),
            OwnedExchange::BinaryPrompt(m) => Ok(Exchange::BinaryPrompt(m)),
        }
    }
}

/// Fills in the answer of the Message with the given response.
///
/// # Safety
///
/// You are responsible for ensuring that the src-dst pair matches.
unsafe fn convert(msg: &Exchange, resp: &mut Answer) {
    macro_rules! fill_text {
        ($dst:ident, $src:ident) => {{
            let text_resp = unsafe { TextAnswer::upcast($src) };
            $dst.set_answer(text_resp.contents().map(Into::into));
        }};
    }
    match *msg {
        Exchange::MaskedPrompt(qa) => fill_text!(qa, resp),
        Exchange::Prompt(qa) => fill_text!(qa, resp),
        Exchange::Error(m) => m.set_answer(Ok(())),
        Exchange::Info(m) => m.set_answer(Ok(())),
        Exchange::RadioPrompt(qa) => fill_text!(qa, resp),
        Exchange::BinaryPrompt(qa) => {
            let bin_resp = unsafe { BinaryAnswer::upcast(resp) };
            qa.set_answer(Ok(bin_resp
                .contents()
                .map(|d| d.into())
                .unwrap_or_default()));
            bin_resp.zero_contents()
        }
    }
}
