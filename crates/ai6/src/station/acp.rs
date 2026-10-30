//! Starts and resumes the Sessions this Station supervises itself over ACP: each handler is
//! served by an in-process [`AcpFacade`], and the Station holds the [`AcpSession`] that drives
//! it, so every Session, local or remote, is controlled through one protocol.

use std::sync::{Arc, Mutex};

use guatiao::Status;
use guatiao::library::{Object, ProviderError};

use crate::directorates::q::terminal::AgentTerminal;
use crate::directorates::section_00::acp::{
    AcpFacade, AcpOptions, AcpSession, LAUNCH_META_KEY, LaunchMeta, ResumeMeta, UNBOUNDED_MS,
    duplex,
};
use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentSession, AgentStart};

/// A façade over `handler` whose operation spawns through `terminal`, once.
fn facade(handler: Box<dyn AgentHandler>, terminal: Object<dyn AgentTerminal>) -> AcpFacade {
    let slot = Mutex::new(Some(terminal));
    AcpFacade::new(
        Arc::from(handler),
        Arc::new(move || {
            slot.lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .ok_or_else(|| {
                    ProviderError::new(
                        Status::GUATIAO_ERR_INTERNAL,
                        "the terminal was already used",
                    )
                })
        }),
    )
}

/// What the Station waits for: a handler's start, resume and close have never had a limit
/// here, and the façade answers at once.
fn options(cwd: String, restricted: bool) -> AcpOptions {
    AcpOptions {
        cwd,
        restricted,
        request_timeout_ms: UNBOUNDED_MS,
        start_timeout_ms: UNBOUNDED_MS,
        close_grace_ms: UNBOUNDED_MS,
        ..AcpOptions::default()
    }
}

/// Starts `request` on `handler` and answers the Session driving it.
pub(crate) fn start(
    handler: Box<dyn AgentHandler>,
    terminal: Object<dyn AgentTerminal>,
    request: AgentStart,
) -> Result<Object<dyn AgentSession>, ProviderError> {
    let facade = facade(handler, terminal);
    let (agent_end, client_end) = duplex();
    facade.serve(agent_end);
    let mut options = options(
        request.working_directory.clone(),
        request.answer_only || request.assistant.is_some(),
    );
    options.prompt = Some(request.prompt.clone());
    options.meta.insert(
        LAUNCH_META_KEY.to_owned(),
        LaunchMeta::from_start(&request).to_value(),
    );
    AcpSession::connect(client_end, options, None).map(AcpSession::into_handle)
}

/// Resumes `request` on `handler` and answers the Session driving it.
pub(crate) fn resume(
    handler: Box<dyn AgentHandler>,
    terminal: Object<dyn AgentTerminal>,
    request: AgentResume,
) -> Result<Object<dyn AgentSession>, ProviderError> {
    let facade = facade(handler, terminal);
    let (agent_end, client_end) = duplex();
    facade.serve(agent_end);
    let mut options = options(
        request.working_directory.clone(),
        request.answer_only || request.assistant.is_some(),
    );
    options.resume = Some(request.session_id.to_string());
    options.meta.insert(
        LAUNCH_META_KEY.to_owned(),
        ResumeMeta::from_resume(&request).to_value(),
    );
    AcpSession::connect(client_end, options, None).map(AcpSession::into_handle)
}

/// Operator typing for a Session whose harness speaks JSON on plain pipes: raw terminal bytes
/// would corrupt its stream, so typed text becomes messages instead, one per line, and
/// Ctrl-C interrupts. Anything else a terminal sends (escape sequences, other control bytes)
/// is refused with a reason.
#[cfg(feature = "publisher")]
pub(crate) struct MessageInput {
    session: Arc<Mutex<Object<dyn AgentSession>>>,
    line: Mutex<Vec<u8>>,
}

#[cfg(feature = "publisher")]
impl MessageInput {
    pub(crate) fn new(session: Arc<Mutex<Object<dyn AgentSession>>>) -> Self {
        Self {
            session,
            line: Mutex::new(Vec::new()),
        }
    }
}

#[cfg(feature = "publisher")]
impl crate::directorates::q::terminal::TerminalInputTarget for MessageInput {
    fn write(&self, data: &[u8]) -> Result<(), &'static str> {
        const FAILED: &str = "terminal input failed";
        let mut line = self.line.lock().map_err(|_| FAILED)?;
        for &byte in data {
            match byte {
                b'\r' | b'\n' => {
                    if line.is_empty() {
                        continue;
                    }
                    let text = String::from_utf8(std::mem::take(&mut *line))
                        .map_err(|_| "this Session takes typed text, not raw bytes")?;
                    self.session
                        .lock()
                        .map_err(|_| FAILED)?
                        .send_message(crate::directorates::section_00::agent::AgentMessage { text })
                        .map_err(|_| FAILED)?;
                }
                0x03 => {
                    line.clear();
                    self.session
                        .lock()
                        .map_err(|_| FAILED)?
                        .interrupt()
                        .map_err(|_| FAILED)?;
                }
                0x08 | 0x7f => while line.pop().is_some_and(|last| (0x80..0xc0).contains(&last)) {},
                b'\t' | 0x20..=0x7e | 0x80..=0xff => line.push(byte),
                _ => {
                    line.clear();
                    return Err(
                        "this Session takes typed text messages; raw terminal input is not accepted",
                    );
                }
            }
        }
        Ok(())
    }

    fn resize(&self, _rows: i64, _cols: i64) -> Result<(), &'static str> {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A fake external ACP agent for Station tests: it asks leave to run one shell command and
    //! reports what it was answered.

    use std::sync::{Arc, Mutex};

    use agent_client_protocol::{
        Agent, Channel, Client, ConnectionTo, Responder, UntypedMessage, on_receive_request,
    };
    use serde_json::{Value, json};

    /// The client end of a connection to an agent that, on its first prompt, asks leave to run
    /// `command` and ends its turn; the answer it got is left in `outcome`.
    pub(crate) fn command_agent(
        command: &'static str,
        outcome: Arc<Mutex<Option<Value>>>,
    ) -> Channel {
        let (agent_end, client_end) = crate::directorates::section_00::acp::duplex();
        std::thread::spawn(move || {
            let _ = futures::executor::block_on(
                Agent
                    .builder()
                    .on_receive_request(
                        async move |message: UntypedMessage,
                                    responder: Responder<Value>,
                                    cx: ConnectionTo<Client>| {
                            let outcome = outcome.clone();
                            match message.method() {
                                "initialize" => responder.respond(
                                    json!({"protocolVersion": 1, "agentCapabilities": {}}),
                                ),
                                "session/new" => responder.respond(json!({"sessionId": "ext-1"})),
                                "session/prompt" => {
                                    std::thread::spawn(move || {
                                        let asked = UntypedMessage::new(
                                            "session/request_permission",
                                            json!({
                                                "sessionId": "ext-1",
                                                "toolCall": {
                                                    "toolCallId": "t1",
                                                    "title": "Run a command",
                                                    "kind": "execute",
                                                    "status": "pending",
                                                    "rawInput": {"command": command},
                                                },
                                                "options": [
                                                    {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                                                    {"optionId": "no", "name": "Reject", "kind": "reject_once"},
                                                ],
                                            }),
                                        )
                                        .expect("a well-formed request");
                                        let answer = futures::executor::block_on(
                                            cx.send_request(asked).block_task(),
                                        );
                                        *outcome.lock().unwrap() = answer.ok();
                                        let _ = responder.respond(json!({"stopReason": "end_turn"}));
                                    });
                                    Ok(())
                                }
                                _ => responder
                                    .respond_with_error(agent_client_protocol::Error::method_not_found()),
                            }
                        },
                        on_receive_request!(),
                    )
                    .connect_to(agent_end),
            );
        });
        client_end
    }
}

#[cfg(all(test, feature = "publisher"))]
mod tests {
    use super::*;
    use crate::SessionId;
    use crate::directorates::q::terminal::TerminalInputTarget;
    use crate::directorates::section_00::agent::{AgentEvent, AgentMessage};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Recorder {
        messages: Mutex<Vec<String>>,
        interrupts: AtomicUsize,
    }

    struct Recording(Arc<Recorder>);

    impl AgentSession for Recording {
        fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
            Ok(None)
        }

        fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
            self.0.messages.lock().unwrap().push(message.text);
            Ok(())
        }

        fn next_event(&self, _timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
            Ok(None)
        }

        fn interrupt(&self) -> Result<(), ProviderError> {
            self.0.interrupts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn close(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    #[test]
    fn operator_typing_for_a_json_harness_becomes_messages_and_never_raw_bytes() {
        let recorder = Arc::new(Recorder::default());
        let session: Object<dyn AgentSession> = Recording(recorder.clone()).into_object();
        let input = MessageInput::new(Arc::new(Mutex::new(session)));
        // Typed in pieces, with a correction, then Enter.
        input.write(b"hel").unwrap();
        input.write(b"lo!\x7f world\r").unwrap();
        input.write("caf\u{e9}\n".as_bytes()).unwrap();
        assert_eq!(
            *recorder.messages.lock().unwrap(),
            vec!["hello world".to_owned(), "caf\u{e9}".to_owned()]
        );
        // Enter on an empty line sends nothing; Ctrl-C interrupts.
        input.write(b"\r\r").unwrap();
        input.write(b"\x03").unwrap();
        assert_eq!(recorder.messages.lock().unwrap().len(), 2);
        assert_eq!(recorder.interrupts.load(Ordering::SeqCst), 1);
        // An escape sequence (an arrow key) or another control byte is refused, and what was
        // typed before it is dropped, not sent.
        input.write(b"half").unwrap();
        let error = input.write(b"\x1b[A").unwrap_err();
        assert!(error.contains("typed text"), "{error}");
        input.write(b"\r").unwrap();
        assert_eq!(recorder.messages.lock().unwrap().len(), 2);
        assert!(input.resize(24, 80).is_ok());
    }
}
