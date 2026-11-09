//! The PAM conversation and associated Stuff.

// Temporarily allowed until we get the actual conversation functions hooked up.
#![allow(dead_code)]

use crate::constants::{ErrorCode, Result};
use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fmt::Debug;
use std::result::Result as StdResult;

/// An individual pair of request/response to be sent to the user.
#[derive(Debug)]
#[non_exhaustive]
pub enum Exchange<'a> {
    Prompt(&'a QAndA<'a>),
    MaskedPrompt(&'a MaskedQAndA<'a>),
    Error(&'a ErrorMsg<'a>),
    Info(&'a InfoMsg<'a>),
    RadioPrompt(&'a RadioQAndA<'a>),
    BinaryPrompt(&'a BinaryQAndA<'a>),
}

impl Exchange<'_> {
    /// Sets an error answer on this question, without having to inspect it.
    ///
    /// Use this as a default match case:
    ///
    /// ```
    /// use nonstick::conv::{Exchange, QAndA};
    /// use nonstick::ErrorCode;
    ///
    /// fn cant_respond(message: Exchange) {
    ///     // "question" is kind of a bad name in the context of
    ///     // a one-way message, but it's for consistency.
    ///     match message {
    ///         Exchange::Info(i) => {
    ///             eprintln!("fyi, {:?}", i.question());
    ///             i.set_answer(Ok(()))
    ///         }
    ///         Exchange::Error(e) => {
    ///             eprintln!("ERROR: {:?}", e.question());
    ///             e.set_answer(Ok(()))
    ///         }
    ///         // We can't answer any questions.
    ///         other => other.set_error(ErrorCode::ConversationError),
    ///     }
    /// }
    pub fn set_error(&self, err: ErrorCode) {
        match *self {
            Exchange::Prompt(m) => m.set_answer(Err(err)),
            Exchange::MaskedPrompt(m) => m.set_answer(Err(err)),
            Exchange::Error(m) => m.set_answer(Err(err)),
            Exchange::Info(m) => m.set_answer(Err(err)),
            Exchange::RadioPrompt(m) => m.set_answer(Err(err)),
            Exchange::BinaryPrompt(m) => m.set_answer(Err(err)),
        }
    }
}

macro_rules! q_and_a {
    ($(#[$m:meta])* $name:ident<'a, Q=$qt:ty, A=$at:ty>, $val:path) => {
        $(#[$m])*
        pub struct $name<'a> {
            q: $qt,
            a: Cell<Result<$at>>,
        }

        $(#[$m])*
        impl<'a> $name<'a> {
            #[doc = concat!("Creates a `", stringify!($t), "` to be sent to the user.")]
            pub fn new(question: $qt) -> Self {
                Self {
                    q: question,
                    a: Cell::new(Err(ErrorCode::ConversationError)),
                }
            }

            /// Converts this Q&A into a [`Exchange`] for the [`Conversation`].
            pub fn exchange(&self) -> Exchange<'_> {
                $val(self)
            }

            /// The contents of the question being asked.
            ///
            /// For instance, this might say `"Username:"` to prompt the user
            /// for their name, or the text of an error message.
            pub fn question(&self) -> $qt {
                self.q
            }

            /// Sets the answer to the question.
            ///
            /// The [`Conversation`] implementation calls this to set the answer.
            /// The conversation should *always call this function*,
            /// even for Q&A messages that don't have "an answer"
            /// (like error or info messages).
            pub fn set_answer(&self, answer: Result<$at>) {
                self.a.set(answer)
            }

            /// Gets the answer to the question.
            pub fn answer(self) -> Result<$at> {
                self.a.into_inner()
            }
        }

        // shout out to stackoverflow user ballpointben for this lazy impl:
        // https://stackoverflow.com/a/78871280/39808
        $(#[$m])*
        impl fmt::Debug for $name<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> StdResult<(), fmt::Error> {
                f.debug_struct(stringify!($name)).field("q", &self.q).finish_non_exhaustive()
            }
        }
    };
}

q_and_a!(
    /// A Q&A that asks the user for text and does not show it while typing.
    ///
    /// In other words, a password entry prompt.
    MaskedQAndA<'a, Q=&'a OsStr, A=OsString>,
    Exchange::MaskedPrompt
);

q_and_a!(
    /// A standard Q&A prompt that asks the user for text.
    ///
    /// This is the normal "ask a person a question" prompt.
    /// When the user types, their input will be shown to them.
    /// It can be used for things like usernames.
    QAndA<'a, Q=&'a OsStr, A=OsString>,
    Exchange::Prompt
);

q_and_a!(
    /// A Q&A for "radio button"–style data. (Linux-PAM extension)
    ///
    /// This message type is theoretically useful for "yes/no/maybe"
    /// questions, but nowhere in the documentation is it specified
    /// what the format of the answer will be, or how this should be shown.
    RadioQAndA<'a, Q=&'a OsStr, A=OsString>,
    Exchange::RadioPrompt
);

q_and_a!(
    /// Asks for binary data. (Linux-PAM extension)
    ///
    /// This sends a binary message to the client application.
    /// It can be used to communicate with non-human logins,
    /// or to enable things like security keys.
    ///
    /// The `data_type` tag is a value that is simply passed through
    /// to the application. PAM does not define any meaning for it.
    BinaryQAndA<'a, Q=(&'a [u8], u8), A=BinaryData>,
    Exchange::BinaryPrompt
);

/// Owned binary data.
#[derive(Debug, Default, PartialEq)]
pub struct BinaryData {
    /// The data.
    pub data: Vec<u8>,
    /// A tag describing the type of the data, to use how you please.
    pub data_type: u8,
}

impl BinaryData {
    /// Creates a `BinaryData` with the given contents and type.
    pub fn new(data: impl Into<Vec<u8>>, data_type: u8) -> Self {
        Self {
            data: data.into(),
            data_type,
        }
    }
}

impl<IV: Into<Vec<u8>>> From<(IV, u8)> for BinaryData {
    /// Makes a new BinaryData from borrowed data.
    fn from((data, data_type): (IV, u8)) -> Self {
        Self {
            data: data.into(),
            data_type,
        }
    }
}

impl From<BinaryData> for (Vec<u8>, u8) {
    /// Easy destructuring.
    fn from(value: BinaryData) -> Self {
        (value.data, value.data_type)
    }
}

impl<'a> From<&'a BinaryData> for (&'a [u8], u8) {
    fn from(value: &'a BinaryData) -> Self {
        (&value.data, value.data_type)
    }
}

q_and_a!(
    /// A message containing information to be passed to the user.
    ///
    /// While this does not have an answer, [`Conversation`] implementations
    /// should still call [`set_answer`][`QAndA::set_answer`] to verify that
    /// the message has been displayed (or actively discarded).
    InfoMsg<'a, Q = &'a OsStr, A = ()>,
    Exchange::Info
);

q_and_a!(
    /// An error message to be passed to the user.
    ///
    /// While this does not have an answer, [`Conversation`] implementations
    /// should still call [`set_answer`][`QAndA::set_answer`] to verify that
    /// the message has been displayed (or actively discarded).
    ErrorMsg<'a, Q = &'a OsStr, A = ()>,
    Exchange::Error
);

/// A channel for PAM modules to request information from the user.
///
/// This trait is used by both applications and PAM modules:
///
/// - Applications implement Conversation and provide a user interface
///   to allow the user to respond to PAM questions.
/// - Modules call a Conversation implementation to request information
///   or send information to the user.
pub trait Conversation {
    /// Sends messages to the user.
    ///
    /// The returned Vec of messages always contains exactly as many entries
    /// as there were messages in the request; one corresponding to each.
    ///
    /// TODO: write detailed documentation about how to use this.
    fn communicate(&self, messages: &[Exchange]);
}

/// Turns a simple function into a [`Conversation`].
///
/// This can be used to wrap a free-floating function for use as a
/// Conversation:
///
/// ```
/// use nonstick::conv::{conversation_func, Conversation, Exchange};
/// mod some_library {
/// #    use nonstick::Conversation;
///     pub fn get_auth_data(conv: &mut impl Conversation) {
///         /* ... */
///     }
/// }
///
/// fn my_terminal_prompt(messages: &[Exchange]) {
///     // ...
/// #    unimplemented!()
/// }
///
/// fn main() {
///     some_library::get_auth_data(&mut conversation_func(my_terminal_prompt));
/// }
/// ```
pub fn conversation_func(func: impl Fn(&[Exchange])) -> impl Conversation {
    FunctionConvo(func)
}

struct FunctionConvo<C: Fn(&[Exchange])>(C);

impl<C: Fn(&[Exchange])> Conversation for FunctionConvo<C> {
    fn communicate(&self, messages: &[Exchange]) {
        self.0(messages)
    }
}

/// A Conversation
struct UsernamePasswordConvo {
    username: String,
    password: String,
}

/// A conversation trait for asking or answering one question at a time.
///
/// An implementation of this is provided for any [`Conversation`],
/// or a PAM application can implement this trait and handle messages
/// one at a time.
///
/// For example, to use a `Conversation` as a `ConversationAdapter`:
///
/// ```
/// # use nonstick::{Conversation, Result};
/// # use std::ffi::OsString;
/// // Bring this trait into scope to get `masked_prompt`, among others.
/// use nonstick::ConversationAdapter;
///
/// fn ask_for_token(convo: &impl Conversation) -> Result<OsString> {
///     convo.masked_prompt("enter your one-time token")
/// }
/// ```
///
/// or to use a `ConversationAdapter` as a `Conversation`:
///
/// ```
/// use nonstick::{Conversation, ConversationAdapter};
/// # use nonstick::{BinaryData, Result};
/// # use std::ffi::{OsStr, OsString};
/// mod some_library {
/// #    use nonstick::Conversation;
///     pub fn get_auth_data(conv: &impl Conversation) { /* ... */
///     }
/// }
///
/// struct MySimpleConvo {/* ... */}
/// # impl MySimpleConvo { fn new() -> Self { Self{} } }
///
/// impl ConversationAdapter for MySimpleConvo {
///     // ...
/// # fn prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString> {
/// #     unimplemented!()
/// # }
/// #
/// # fn masked_prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString> {
/// #     unimplemented!()
/// # }
/// #
/// # fn error_msg(&self, message: impl AsRef<OsStr>) {
/// #     unimplemented!()
/// # }
/// #
/// # fn info_msg(&self, message: impl AsRef<OsStr>) {
/// #     unimplemented!()
/// # }
/// #
/// # fn radio_prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString> {
/// #     unimplemented!()
/// # }
/// #
/// # fn binary_prompt(&self, (data, data_type): (&[u8], u8)) -> Result<BinaryData> {
/// #     unimplemented!()
/// # }
/// }
///
/// fn main() {
///     let mut simple = MySimpleConvo::new();
///     some_library::get_auth_data(&mut simple.into_conversation())
/// }
/// ```
pub trait ConversationAdapter {
    /// Lets you use this simple conversation as a full [Conversation].
    ///
    /// The wrapper takes each message received in [`Conversation::communicate`]
    /// and passes them one-by-one to the appropriate method,
    /// then collects responses to return.
    fn into_conversation(self) -> Demux<Self>
    where
        Self: Sized,
    {
        Demux(self)
    }
    /// Prompts the user for something.
    fn prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString>;
    /// Prompts the user for something, but hides what the user types.
    fn masked_prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString>;
    /// Alerts the user to an error.
    fn error_msg(&self, message: impl AsRef<OsStr>);
    /// Sends an informational message to the user.
    fn info_msg(&self, message: impl AsRef<OsStr>);
    /// \[Linux extension] Prompts the user for a yes/no/maybe conditional.
    ///
    /// PAM documentation doesn't define the format of the response.
    ///
    /// When called on an implementation that doesn't support radio prompts,
    /// this will return [`ErrorCode::ConversationError`].
    /// If implemented on an implementation that doesn't support radio prompts,
    /// this will never be called.
    fn radio_prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString> {
        let _ = request;
        Err(ErrorCode::ConversationError)
    }
    /// \[Linux extension] Requests binary data from the user.
    ///
    /// When called on an implementation that doesn't support radio prompts,
    /// this will return [`ErrorCode::ConversationError`].
    /// If implemented on an implementation that doesn't support radio prompts,
    /// this will never be called.
    fn binary_prompt(&self, data_and_type: (&[u8], u8)) -> Result<BinaryData> {
        let _ = data_and_type;
        Err(ErrorCode::ConversationError)
    }
}

impl<CA: ConversationAdapter> From<CA> for Demux<CA> {
    fn from(value: CA) -> Self {
        Demux(value)
    }
}

macro_rules! conv_fn {
    ($(#[$m:meta])* $fn_name:ident($param:tt: $pt:ty) -> $resp_type:ty { $msg:ty }) => {
        $(#[$m])*
        fn $fn_name(&self, $param: impl AsRef<$pt>) -> Result<$resp_type> {
            let prompt = <$msg>::new($param.as_ref());
            self.communicate(&[prompt.exchange()]);
            prompt.answer()
        }
    };
    ($(#[$m:meta])*$fn_name:ident($param:tt: $pt:ty) { $msg:ty }) => {
        $(#[$m])*
        fn $fn_name(&self, $param: impl AsRef<$pt>) {
            self.communicate(&[<$msg>::new($param.as_ref()).exchange()]);
        }
    };
}

impl<C: Conversation + ?Sized> ConversationAdapter for C {
    conv_fn!(prompt(message: OsStr) -> OsString { QAndA });
    conv_fn!(masked_prompt(message: OsStr) -> OsString { MaskedQAndA } );
    conv_fn!(error_msg(message: OsStr) { ErrorMsg });
    conv_fn!(info_msg(message: OsStr) { InfoMsg });
    conv_fn!(radio_prompt(message: OsStr) -> OsString { RadioQAndA });
    fn binary_prompt(&self, (data, typ): (&[u8], u8)) -> Result<BinaryData> {
        let prompt = BinaryQAndA::new((data, typ));
        self.communicate(&[prompt.exchange()]);
        prompt.answer()
    }
}

/// A [`Conversation`] which asks the questions one at a time.
///
/// This is automatically created by [`ConversationAdapter::into_conversation`].
pub struct Demux<CA: ConversationAdapter>(CA);

impl<CA: ConversationAdapter> Demux<CA> {
    /// Gets the original Conversation out of this wrapper.
    fn into_inner(self) -> CA {
        self.0
    }
}

impl<CA: ConversationAdapter> Conversation for Demux<CA> {
    fn communicate(&self, messages: &[Exchange]) {
        for msg in messages {
            match msg {
                Exchange::Prompt(prompt) => prompt.set_answer(self.0.prompt(prompt.question())),
                Exchange::MaskedPrompt(prompt) => {
                    prompt.set_answer(self.0.masked_prompt(prompt.question()))
                }
                Exchange::RadioPrompt(prompt) => {
                    prompt.set_answer(self.0.radio_prompt(prompt.question()))
                }
                Exchange::Info(prompt) => {
                    self.0.info_msg(prompt.question());
                    prompt.set_answer(Ok(()))
                }
                Exchange::Error(prompt) => {
                    self.0.error_msg(prompt.question());
                    prompt.set_answer(Ok(()))
                }
                Exchange::BinaryPrompt(prompt) => {
                    let q = prompt.question();
                    prompt.set_answer(self.0.binary_prompt(q))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demux() {
        #[derive(Default)]
        struct DemuxTester {
            error_ran: Cell<bool>,
            info_ran: Cell<bool>,
        }

        impl ConversationAdapter for DemuxTester {
            fn prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString> {
                match request.as_ref().to_str().unwrap() {
                    "what" => Ok("whatwhat".into()),
                    "give_err" => Err(ErrorCode::PermissionDenied),
                    _ => panic!("unexpected prompt!"),
                }
            }
            fn masked_prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString> {
                assert_eq!("reveal", request.as_ref());
                Ok("my secrets".into())
            }
            fn error_msg(&self, message: impl AsRef<OsStr>) {
                self.error_ran.set(true);
                assert_eq!("whoopsie", message.as_ref());
            }
            fn info_msg(&self, message: impl AsRef<OsStr>) {
                self.info_ran.set(true);
                assert_eq!("did you know", message.as_ref());
            }
            fn radio_prompt(&self, request: impl AsRef<OsStr>) -> Result<OsString> {
                assert_eq!("channel?", request.as_ref());
                Ok("zero".into())
            }
            fn binary_prompt(&self, data_and_type: (&[u8], u8)) -> Result<BinaryData> {
                assert_eq!((&[10, 9, 8][..], 66), data_and_type);
                Ok(BinaryData::new(vec![5, 5, 5], 5))
            }
        }

        let tester = DemuxTester::default();

        let what = QAndA::new("what".as_ref());
        let pass = MaskedQAndA::new("reveal".as_ref());
        let err = ErrorMsg::new("whoopsie".as_ref());
        let info = InfoMsg::new("did you know".as_ref());
        let has_err = QAndA::new("give_err".as_ref());

        let conv = tester.into_conversation();

        // Basic tests.

        conv.communicate(&[
            what.exchange(),
            pass.exchange(),
            err.exchange(),
            info.exchange(),
            has_err.exchange(),
        ]);

        assert_eq!("whatwhat", what.answer().unwrap());
        assert_eq!("my secrets", pass.answer().unwrap());
        assert_eq!(Ok(()), err.answer());
        assert_eq!(Ok(()), info.answer());
        assert_eq!(ErrorCode::PermissionDenied, has_err.answer().unwrap_err());
        let tester = conv.into_inner();
        assert!(tester.error_ran.get());
        assert!(tester.info_ran.get());

        // Test the Linux extensions separately.
        {
            let conv = tester.into_conversation();

            let radio = RadioQAndA::new("channel?".as_ref());
            let bin = BinaryQAndA::new((&[10, 9, 8], 66));
            conv.communicate(&[radio.exchange(), bin.exchange()]);

            assert_eq!("zero", radio.answer().unwrap());
            assert_eq!(BinaryData::from(([5, 5, 5], 5)), bin.answer().unwrap());
        }
    }

    fn test_mux() {
        struct MuxTester;

        impl Conversation for MuxTester {
            fn communicate(&self, messages: &[Exchange]) {
                if let [msg] = messages {
                    match *msg {
                        Exchange::Info(info) => {
                            assert_eq!("let me tell you", info.question());
                            info.set_answer(Ok(()))
                        }
                        Exchange::Error(error) => {
                            assert_eq!("oh no", error.question());
                            error.set_answer(Ok(()))
                        }
                        Exchange::Prompt(prompt) => {
                            prompt.set_answer(match prompt.question().to_str().unwrap() {
                                "should_err" => Err(ErrorCode::PermissionDenied),
                                "question" => Ok("answer".into()),
                                other => panic!("unexpected question {other:?}"),
                            })
                        }
                        Exchange::MaskedPrompt(ask) => {
                            assert_eq!("password!", ask.question());
                            ask.set_answer(Ok("open sesame".into()))
                        }
                        Exchange::BinaryPrompt(prompt) => {
                            assert_eq!((&[1, 2, 3][..], 69), prompt.question());
                            prompt.set_answer(Ok(BinaryData::from((&[3, 2, 1], 42))))
                        }
                        Exchange::RadioPrompt(ask) => {
                            assert_eq!("radio?", ask.question());
                            ask.set_answer(Ok("yes".into()))
                        }
                    }
                } else {
                    panic!(
                        "there should only be one message, not {len}",
                        len = messages.len()
                    )
                }
            }
        }

        let tester = MuxTester;

        assert_eq!("answer", tester.prompt("question").unwrap());
        assert_eq!("open sesame", tester.masked_prompt("password!").unwrap());
        tester.error_msg("oh no");
        tester.info_msg("let me tell you");
        // Linux-PAM extensions. Always implemented, but separate for clarity.
        {
            assert_eq!("yes", tester.radio_prompt("radio?").unwrap());
            assert_eq!(
                BinaryData::new(vec![3, 2, 1], 42),
                tester.binary_prompt((&[1, 2, 3], 69)).unwrap(),
            )
        }
        assert_eq!(
            ErrorCode::BufferError,
            tester.prompt("should_error").unwrap_err(),
        );
        assert_eq!(
            ErrorCode::ConversationError,
            tester.masked_prompt("return_wrong_type").unwrap_err()
        )
    }
}
