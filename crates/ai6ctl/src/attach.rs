//! `ai6ctl charge attach` and `ai6ctl mission attach`: a line-based client of the gateway's
//! `GET /v1/user/attach` WebSocket. Each frame the Run shows is printed as one line; each line
//! read from the input is a prompt, or a `/` command. The access token travels in the first
//! WebSocket frame only, never in the URL, and is printed nowhere.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, client_async};

/// The time to connect and complete the WebSocket handshake.
const CONNECT: Duration = Duration::from_secs(10);
/// The time the gateway has to answer the hello; it holds its own deadline for receiving one.
const HELLO_REPLY: Duration = Duration::from_secs(15);
/// The time a closing handshake may take once the person has left.
const LEAVE: Duration = Duration::from_secs(2);

const USAGE: &str = "type text to prompt the Run; //text prompts with a leading slash; /answer N OPTION, \
/allow N, /refuse N, /interrupt, /detach (or end of input) to leave";

/// What to attach to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A charge's standing Run: `m`, `operation:CODENAME` or `station:ID`.
    Charge(String),
    /// A Mission's live Run.
    Mission { operation: String, mission: String },
}

impl Target {
    pub(crate) fn hello(&self, token: &str, after: Option<u64>) -> Value {
        let mut hello = json!({"type": "hello", "token": token});
        match self {
            Self::Charge(scope) => hello["charge"] = json!(scope),
            Self::Mission { operation, mission } => {
                hello["mission"] = json!({"operation": operation, "mission": mission});
            }
        }
        if let Some(after) = after {
            hello["after"] = json!(after);
        }
        hello
    }
}

/// Attaches through the gateway at `server` (an `http` or `https` origin) with `hello` (see
/// [`Target::hello`]), reading
/// the person's lines from `input` and printing what the Run shows to `out`; the gateway's
/// refusals of a request go to `err`. `authorities` are DER certificates trusted besides the
/// public roots for `https`.
///
/// Returns when the person detaches or the Run closes; an error when the gateway refuses the
/// attachment or the connection ends without either.
pub(crate) async fn attach<I, O, E>(
    server: &Url,
    authorities: &[Vec<u8>],
    hello: Value,
    input: I,
    out: &mut O,
    err: &mut E,
) -> Result<(), String>
where
    I: AsyncBufRead + Unpin,
    O: Write,
    E: Write,
{
    let secure = server.scheme() == "https";
    let host = server
        .host_str()
        .map(|host| host.trim_matches(['[', ']']).to_owned())
        .ok_or_else(|| "Universal Exports URL has no host".to_owned())?;
    let port = server
        .port_or_known_default()
        .ok_or_else(|| "Universal Exports URL has no port".to_owned())?;
    let address = format!(
        "{}://{}/v1/user/attach",
        if secure { "wss" } else { "ws" },
        server.authority()
    );
    let unreachable = || "could not contact Universal Exports".to_owned();

    let tcp = tokio::time::timeout(CONNECT, TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| unreachable())?
        .map_err(|_| unreachable())?;
    if secure {
        let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host)
            .map_err(|_| "Universal Exports URL has an invalid host name".to_owned())?;
        let connector = tokio_rustls::TlsConnector::from(tls_config(authorities)?);
        let stream = tokio::time::timeout(CONNECT, connector.connect(name, tcp))
            .await
            .map_err(|_| unreachable())?
            .map_err(|_| "could not establish a TLS connection to Universal Exports".to_owned())?;
        let (socket, _) = tokio::time::timeout(CONNECT, client_async(address.as_str(), stream))
            .await
            .map_err(|_| unreachable())?
            .map_err(|_| unreachable())?;
        session(socket, hello, input, out, err).await
    } else {
        let (socket, _) = tokio::time::timeout(CONNECT, client_async(address.as_str(), tcp))
            .await
            .map_err(|_| unreachable())?
            .map_err(|_| unreachable())?;
        session(socket, hello, input, out, err).await
    }
}

fn tls_config(authorities: &[Vec<u8>]) -> Result<Arc<tokio_rustls::rustls::ClientConfig>, String> {
    use tokio_rustls::rustls::pki_types::CertificateDer;
    use tokio_rustls::rustls::{ClientConfig, RootCertStore, crypto};

    let mut roots = RootCertStore::empty();
    for certificate in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        roots
            .add(certificate.clone())
            .map_err(|_| "a public root certificate is not usable".to_owned())?;
    }
    for der in authorities {
        roots
            .add(CertificateDer::from(der.clone()))
            .map_err(|_| "a certificate of the CA file is not usable".to_owned())?;
    }
    let config = ClientConfig::builder_with_provider(Arc::new(crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|_| "could not initialize TLS".to_owned())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The conversation over an open WebSocket: the hello, then frames out and lines in.
pub(crate) async fn session<S, I, O, E>(
    mut socket: WebSocketStream<S>,
    hello: Value,
    input: I,
    out: &mut O,
    err: &mut E,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: AsyncBufRead + Unpin,
    O: Write,
    E: Write,
{
    send(&mut socket, &hello).await?;
    let first = tokio::time::timeout(HELLO_REPLY, next(&mut socket))
        .await
        .map_err(|_| "the gateway did not answer".to_owned())??;
    match first {
        None => return Err("the connection ended before the gateway answered".to_owned()),
        Some(reply) => match reply["type"].as_str() {
            Some("attached") => emit(
                out,
                &format!("attached as {}", text(&reply["client"]).unwrap_or_default()),
            )?,
            Some("error") => return Err(format!("refused: {}", problem(&reply))),
            _ => return Err("the gateway sent something other than an answer".to_owned()),
        },
    }

    let mut lines = input.lines();
    loop {
        tokio::select! {
            message = next(&mut socket) => {
                let Some(message) = message? else {
                    return Err("the connection ended".to_owned());
                };
                match message["type"].as_str() {
                    Some("frame") => {
                        emit(out, &frame_line(&message["frame"]))?;
                        if message["frame"]["kind"] == "closed" {
                            leave(&mut socket, false).await;
                            return Ok(());
                        }
                    }
                    Some("error") => {
                        writeln!(err, "gateway: {}", problem(&message))
                            .and_then(|()| err.flush())
                            .map_err(|_| "could not write output".to_owned())?;
                    }
                    _ => {}
                }
            }
            line = lines.next_line() => {
                let line = line.map_err(|_| "could not read input".to_owned())?;
                let Some(line) = line else {
                    leave(&mut socket, true).await;
                    return Ok(());
                };
                match parse_line(&line) {
                    Input::Blank => {}
                    Input::Send(request) => send(&mut socket, &request).await?,
                    Input::Detach => {
                        leave(&mut socket, true).await;
                        return Ok(());
                    }
                    Input::Bad(why) => {
                        writeln!(err, "ai6ctl: {why}")
                            .and_then(|()| err.flush())
                            .map_err(|_| "could not write output".to_owned())?;
                    }
                }
            }
        }
    }
}

/// Leaves the Run, which goes on; `say` tells the gateway first.
async fn leave<S>(socket: &mut WebSocketStream<S>, say: bool)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if say {
        let _ = send(socket, &json!({"type": "detach"})).await;
    }
    let _ = tokio::time::timeout(LEAVE, socket.close(None)).await;
}

async fn send<S>(socket: &mut WebSocketStream<S>, value: &Value) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|_| "the connection ended".to_owned())
}

/// The next JSON text frame; `None` when the socket closed.
async fn next<S>(socket: &mut WebSocketStream<S>) -> Result<Option<Value>, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        match socket.next().await {
            None | Some(Ok(Message::Close(_))) => return Ok(None),
            Some(Ok(Message::Text(body))) => {
                return serde_json::from_str(body.as_str())
                    .map(Some)
                    .map_err(|_| "the gateway sent a frame that is not JSON".to_owned());
            }
            Some(Ok(_)) => {}
            Some(Err(_)) => return Err("the connection failed".to_owned()),
        }
    }
}

fn emit<O: Write>(out: &mut O, line: &str) -> Result<(), String> {
    writeln!(out, "{line}")
        .and_then(|()| out.flush())
        .map_err(|_| "could not write output".to_owned())
}

/// What a typed line asks.
#[derive(Debug, PartialEq)]
enum Input {
    Blank,
    Send(Value),
    Detach,
    Bad(String),
}

fn parse_line(line: &str) -> Input {
    let line = line.trim();
    if line.is_empty() {
        return Input::Blank;
    }
    if let Some(escaped) = line.strip_prefix("//") {
        return Input::Send(json!({"type": "prompt", "text": format!("/{escaped}")}));
    }
    let Some(command) = line.strip_prefix('/') else {
        return Input::Send(json!({"type": "prompt", "text": line}));
    };
    let words: Vec<&str> = command.split_whitespace().collect();
    let request = |text: &str| {
        text.parse::<u64>()
            .map_err(|_| format!("{text:?} is not a request number; {USAGE}"))
    };
    let answer = |request: Result<u64, String>, option: &str| {
        request.map(|request| {
            Input::Send(json!({"type": "answer", "request": request, "option": option}))
        })
    };
    let parsed = match words.as_slice() {
        ["answer", number, option] => answer(request(number), option),
        ["allow", number] => answer(request(number), "allow"),
        ["refuse", number] => answer(request(number), "refuse"),
        ["interrupt"] => Ok(Input::Send(json!({"type": "interrupt"}))),
        ["detach"] => Ok(Input::Detach),
        _ => Err(USAGE.to_owned()),
    };
    parsed.unwrap_or_else(Input::Bad)
}

/// One printable line for a frame of the Run's history.
fn frame_line(frame: &Value) -> String {
    let seq = frame["seq"]
        .as_u64()
        .filter(|seq| *seq > 0)
        .map_or_else(|| "-".to_owned(), |seq| seq.to_string());
    let data = &frame["data"];
    let kind = frame["kind"].as_str().unwrap_or("?");
    let said = |key: &str| text(&data[key]).unwrap_or_default();
    let body = match kind {
        "event" => {
            let what = data["kind"].as_str().unwrap_or("event");
            let mut body = format!("{what}: {}", said("text"));
            if let Some(tools) = data["tools"].as_array().filter(|tools| !tools.is_empty()) {
                let names: Vec<String> = tools.iter().filter_map(text).collect();
                body.push_str(&format!(" [tools: {}]", names.join(", ")));
            }
            body
        }
        "prompt" => format!("prompt from {}: {}", said("from"), said("text")),
        "turn" => match data["state"].as_str() {
            Some("begun") => format!("turn begun by {}", said("by")),
            Some("ended") => format!("turn ended by {}", said("by")),
            _ => format!("turn: {data}"),
        },
        "permission" => {
            let options: Vec<String> = data["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|option| {
                    let id = text(&option["id"]).unwrap_or_default();
                    match text(&option["label"]) {
                        Some(label) if label != id => format!("{id} ({label})"),
                        _ => id,
                    }
                })
                .collect();
            let mut body = format!(
                "permission {}: {}",
                text(&data["request"]).unwrap_or_default(),
                said("tool")
            );
            if let Some(detail) = text(&data["detail"]).filter(|detail| !detail.is_empty()) {
                body.push_str(&format!(" ({detail})"));
            }
            body.push_str(&format!(" options: {}", options.join(", ")));
            body
        }
        "answered" => {
            let mut body = format!(
                "answered {} by {}: {}",
                text(&data["request"]).unwrap_or_default(),
                said("by"),
                said("option")
            );
            if let Some(reason) = text(&data["reason"]).filter(|reason| !reason.is_empty()) {
                body.push_str(&format!(" ({reason})"));
            }
            body
        }
        "attached" | "detached" => format!("{} {kind}", said("person")),
        "interrupted" => format!("interrupted by {}", said("by")),
        "ready" => format!(
            "ready: busy {}, attached {}, last {}",
            data["busy"], data["attached"], data["last"]
        ),
        "closed" => format!("closed: {}", said("reason")),
        "error" => format!("error: {}", said("reason")),
        _ => format!("{kind}: {data}"),
    };
    format!("[{seq}] {}", plain(&body))
}

/// A JSON string as text, a number or boolean as its digits or word, anything else compactly.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// An error frame as `code: message`.
fn problem(frame: &Value) -> String {
    plain(&format!(
        "{}: {}",
        text(&frame["code"]).unwrap_or_else(|| "error".to_owned()),
        text(&frame["message"]).unwrap_or_default()
    ))
}

/// One line with nothing a terminal would act on: a newline shows as `\n`, any other control
/// character as U+FFFD.
fn plain(value: &str) -> String {
    value
        .chars()
        .fold(String::with_capacity(value.len()), |mut line, character| {
            match character {
                '\n' => line.push_str("\\n"),
                character if character.is_control() => line.push('\u{fffd}'),
                character => line.push(character),
            }
            line
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_lines_become_requests() {
        assert_eq!(
            parse_line("  what is G-1?  "),
            Input::Send(json!({"type": "prompt", "text": "what is G-1?"}))
        );
        assert_eq!(
            parse_line("//usr is a path"),
            Input::Send(json!({"type": "prompt", "text": "/usr is a path"}))
        );
        assert_eq!(
            parse_line("/answer 7 once"),
            Input::Send(json!({"type": "answer", "request": 7, "option": "once"}))
        );
        assert_eq!(
            parse_line("/allow 3"),
            Input::Send(json!({"type": "answer", "request": 3, "option": "allow"}))
        );
        assert_eq!(
            parse_line("/refuse 3"),
            Input::Send(json!({"type": "answer", "request": 3, "option": "refuse"}))
        );
        assert_eq!(
            parse_line("/interrupt"),
            Input::Send(json!({"type": "interrupt"}))
        );
        assert_eq!(parse_line("/detach"), Input::Detach);
        assert_eq!(parse_line("   "), Input::Blank);
        for bad in ["/allow", "/allow x", "/nope", "/answer 1"] {
            assert!(matches!(parse_line(bad), Input::Bad(_)), "{bad}");
        }
    }

    #[test]
    fn frames_print_as_one_line_each() {
        let line = |kind: &str, data: Value| {
            frame_line(&json!({"seq": 4, "at": 1.0, "kind": kind, "data": data}))
        };
        assert_eq!(
            line("event", json!({"kind": "tool", "text": "Run a\ncommand"})),
            r"[4] tool: Run a\ncommand"
        );
        assert_eq!(
            line("prompt", json!({"from": "ada", "text": "hi"})),
            "[4] prompt from ada: hi"
        );
        assert_eq!(
            line("turn", json!({"state": "begun", "by": "ada"})),
            "[4] turn begun by ada"
        );
        assert_eq!(
            line(
                "permission",
                json!({"request": 7, "tool": "Bash", "detail": "ls",
                       "options": [{"id": "allow", "label": "Allow"}, {"id": "refuse", "label": "refuse"}]})
            ),
            "[4] permission 7: Bash (ls) options: allow (Allow), refuse"
        );
        assert_eq!(
            frame_line(
                &json!({"kind": "ready", "data": {"busy": false, "attached": 2, "last": 9}})
            ),
            "[-] ready: busy false, attached 2, last 9"
        );
        assert_eq!(line("future", json!({"a": 1})), r#"[4] future: {"a":1}"#);
    }

    #[test]
    fn the_hello_carries_the_token_and_the_target() {
        assert_eq!(
            Target::Charge("m".to_owned()).hello("t", Some(5)),
            json!({"type": "hello", "token": "t", "charge": "m", "after": 5})
        );
        assert_eq!(
            Target::Mission {
                operation: "G".to_owned(),
                mission: "x".to_owned()
            }
            .hello("t", None),
            json!({"type": "hello", "token": "t", "mission": {"operation": "G", "mission": "x"}})
        );
    }
}
