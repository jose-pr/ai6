//! `mcp-offer/1` for a program nobody declared it to, such as an IDE on this machine: a peer keeps a stream open with no session on it,
//! and `mcp-shim --peer PEER` finds it through a file only this user can read. One stream carries one client, so the peer keeps one more
//! open than are in use, and each program gets a stream of its own under the one grant.
//!
//! The entry `mcp-config` prints names the peer and nothing else, so it stays good however often the browser, its port or its token
//! change. A shim reaches only the peer it names: another peer of this bridge has a file of its own.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{self, ErrorKind, Read as _, Write as _};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use agentic_peer::home;
use cabuya::link::{Accepted, Handler};
use cabuya::mux::{CloseReason, LinkStream, StreamLife};
use coqui_transport::Failure;
use serde_json::{Value, json};

/// What a peer opens `mcp-offer/1` with to keep a stream standing. An older bridge refuses it, as it is no server name.
pub const METADATA: &[u8] = b"standing/1";
/// The stream's first message: this bridge keeps it for whichever program comes next.
const READY: &[u8] = br#"{"standing":1}"#;
/// A program took it, so the peer should open another. Every message after this is MCP's own.
const CLAIMED: &[u8] = br#"{"claimed":true}"#;
/// The server's name in what `mcp-config` prints, unless it is told another.
pub const DEFAULT_NAME: &str = "canoa-browser";
/// As long as a line may be: the mux's own default for a message.
const MAX_LINE: usize = 16 * 1024 * 1024;
const TOKEN_BYTES: usize = 32;
/// What a door says back to a shim: its answer in hex, a space, and `ok` or `no`.
const ANSWER_LENGTH: usize = TOKEN_BYTES * 2 + 3;
const POLL: Duration = Duration::from_millis(50);
/// How long a relay waits between looks at a socket with nothing to read or no room to write. The socket is non-blocking and has no
/// timeout of its own: a receive timeout can fail on Windows just as data arrives, which reads as the connection ending.
const RELAY_POLL: Duration = Duration::from_millis(5);
/// How long something that connected has to show the token, in all.
const TOKEN_WAIT: Duration = Duration::from_secs(2);
/// As many waiting to show one: past this the longest waiting goes.
const MAX_KNOCKING: usize = 16;
/// How long a shim that showed the token waits for a stream. The peer opens the next one when it hears the last was taken.
const STREAM_WAIT: Duration = Duration::from_secs(3);
/// A shim's whole patience with a door: longer than the door's own for a stream, so it hears the answer.
const ANSWER_WAIT: Duration = Duration::from_secs(8);
/// How long a line may wait on a program that stopped reading, or on a peer whose stream is full.
const STUCK: Duration = Duration::from_secs(30);
/// How long a shim with nothing behind it waits to answer its client's first message before it just exits.
const FIRST_LINE_WAIT: Duration = Duration::from_secs(10);

/// `offer` for everything but a standing stream, which is kept here in `dir`.
pub fn handler(offer: Handler, dir: PathBuf) -> Handler {
    let doors = Arc::new(Doors {
        dir,
        open: Mutex::new(BTreeMap::new()),
    });
    Box::new(move |stream: &dyn LinkStream, accepted: &Accepted| {
        if accepted.metadata == METADATA {
            if let Err(why) = keep(&doors, stream, &accepted.peer) {
                stream.close_with(CloseReason::Refused, why);
            }
        } else {
            offer(stream, accepted);
        }
    })
}

/// `sha256:` and 64 hex digits, which is what a proof shows and what is safe to put in a file's name.
fn hex_of(peer: &str) -> Option<&str> {
    let hex = peer.strip_prefix("sha256:")?;
    (hex.len() == TOKEN_BYTES * 2
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    .then_some(hex)
}

fn file_for(dir: &Path, peer: &str) -> Result<PathBuf, &'static str> {
    let hex = hex_of(peer).ok_or("that is not a peer id")?;
    Ok(dir.join(format!("standing-{hex}.json")))
}

/// Where this user's standing files are kept.
pub fn dir(home: Option<&Path>) -> Result<PathBuf, String> {
    home.map(Path::to_path_buf)
        .or_else(home::dir)
        .map(|dir| dir.join("offers"))
        .ok_or_else(|| "no home directory for this user".to_owned())
}

fn new_token() -> Result<String, &'static str> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    // A guessable token lets anyone on this machine in, so no randomness means no door.
    coqui_peer_webcrypto::random()
        .fill(&mut bytes)
        .map_err(|_| "this machine has no randomness to offer that with")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Takes away what `%LOCALAPPDATA%`'s own entries would give others, which on some machines is more than its owner. Run on the empty file,
/// so nothing secret was ever readable under them. This crate has no unsafe code, so it asks the system's own tool.
#[cfg(windows)]
fn owner_only(path: &Path) -> io::Result<()> {
    let system =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let status = std::process::Command::new(system.join("System32").join("icacls.exe"))
        .arg(path)
        // S-1-3-4 is whoever owns the file.
        .args(["/inheritance:r", "/grant:r", "*S-1-3-4:F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("icacls refused"))
    }
}

#[cfg(not(windows))]
fn owner_only(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Every door of this process, one for each peer with a standing stream.
struct Doors {
    dir: PathBuf,
    open: Mutex<BTreeMap<String, Arc<Door>>>,
}

/// A loopback port for one peer's streams, named in that peer's file. It lasts as long as the peer has a stream here.
struct Door {
    path: PathBuf,
    /// What a shim shows to be let in.
    token: String,
    /// What the door shows back. Something else on a port a dead bridge left has neither, so a shim tells it from this.
    answer: String,
    free: Mutex<VecDeque<Slot>>,
    streams: Mutex<usize>,
    stop: AtomicBool,
}

struct Slot {
    id: u64,
    give: mpsc::Sender<TcpStream>,
}

impl Doors {
    fn enter(&self, peer: &str) -> Result<Arc<Door>, &'static str> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(door) = open.get(peer) {
            *door.streams.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            return Ok(Arc::clone(door));
        }
        let refused = |_| "no local port could be opened";
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(refused)?;
        listener.set_nonblocking(true).map_err(refused)?;
        let address = listener.local_addr().map_err(refused)?;
        let door = Arc::new(Door {
            path: file_for(&self.dir, peer)?,
            token: new_token()?,
            answer: new_token()?,
            free: Mutex::new(VecDeque::new()),
            streams: Mutex::new(1),
            stop: AtomicBool::new(false),
        });
        let contents = json!({
            "address": address.to_string(),
            "token": door.token,
            "answer": door.answer,
        });
        publish(&door.path, contents.to_string().as_bytes())
            .map_err(|_| "the door could not be stored")?;
        let running = Arc::clone(&door);
        thread::spawn(move || running.run(&listener));
        open.insert(peer.to_owned(), Arc::clone(&door));
        Ok(door)
    }

    fn leave(&self, peer: &str, door: &Door) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let mut streams = door.streams.lock().unwrap_or_else(PoisonError::into_inner);
        *streams -= 1;
        if *streams == 0 {
            door.stop.store(true, Ordering::SeqCst);
            open.remove(peer);
            door.forget_file();
        }
    }
}

/// Into place whole, so a shim reads the old file or the new one and never part of either.
fn publish(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let draft = path.with_extension(new_token().map_err(io::Error::other)?);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = options
        .open(&draft)
        .and_then(|mut file| {
            owner_only(&draft)?;
            file.write_all(contents)
        })
        .and_then(|()| fs::rename(&draft, path));
    if written.is_err() {
        let _ = fs::remove_file(&draft);
    }
    written
}

/// Something that connected and has not yet shown the token: read without waiting, so one slow connection holds up nobody.
struct Knock {
    connection: TcpStream,
    shown: Vec<u8>,
    since: Instant,
}

enum Knocked {
    Waiting,
    Wrong,
    Right,
}

impl Knock {
    /// Reads only as far as the first line, which is all anything without the token gets to send.
    fn read(&mut self, token: &str) -> Knocked {
        let wanted = token.len() + 1;
        let mut buffer = [0_u8; 128];
        while self.shown.len() < wanted {
            let room = (wanted - self.shown.len()).min(buffer.len());
            match (&self.connection).read(&mut buffer[..room]) {
                Ok(0) => return Knocked::Wrong,
                Ok(count) => self.shown.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    return if self.since.elapsed() < TOKEN_WAIT {
                        Knocked::Waiting
                    } else {
                        Knocked::Wrong
                    };
                }
                Err(_) => return Knocked::Wrong,
            }
        }
        let same = self.shown.pop() == Some(b'\n')
            && self
                .shown
                .iter()
                .zip(token.as_bytes())
                .fold(0, |differs, (a, b)| differs | (a ^ b))
                == 0;
        if same { Knocked::Right } else { Knocked::Wrong }
    }
}

impl Door {
    fn run(&self, listener: &TcpListener) {
        let mut knocking: Vec<Knock> = Vec::new();
        let mut admitted: Vec<(TcpStream, Instant)> = Vec::new();
        while !self.stop.load(Ordering::SeqCst) {
            // All that are waiting, so however many connected first cannot put off the real one by a turn each.
            while let Ok((connection, _)) = listener.accept() {
                if connection.set_nonblocking(true).is_err() {
                    continue;
                }
                if knocking.len() >= MAX_KNOCKING {
                    knocking.remove(0);
                }
                knocking.push(Knock {
                    connection,
                    shown: Vec::with_capacity(self.token.len() + 1),
                    since: Instant::now(),
                });
            }
            knocking.retain_mut(|knock| match knock.read(&self.token) {
                Knocked::Waiting => true,
                Knocked::Wrong => false,
                Knocked::Right => {
                    if admitted.len() < MAX_KNOCKING
                        && let Ok(connection) = knock.connection.try_clone()
                    {
                        admitted.push((connection, Instant::now()));
                    }
                    false
                }
            });
            admitted.retain(|(connection, since)| self.place(connection, *since));
            thread::sleep(POLL);
        }
    }

    /// Gives an admitted shim a stream if there is one, or tells it there is none once it has waited. True while it is still waiting.
    fn place(&self, connection: &TcpStream, since: Instant) -> bool {
        let slot = self
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        match slot {
            Some(slot) => {
                if self.say(connection, "ok")
                    && let Ok(handed) = connection.try_clone()
                {
                    let _ = slot.give.send(handed);
                }
                false
            }
            None if since.elapsed() >= STREAM_WAIT => {
                let _ = self.say(connection, "no");
                false
            }
            None => true,
        }
    }

    fn say(&self, mut connection: &TcpStream, word: &str) -> bool {
        connection
            .write_all(format!("{} {word}", self.answer).as_bytes())
            .is_ok()
    }

    fn offer_slot(&self, id: u64, give: mpsc::Sender<TcpStream>) {
        self.free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(Slot { id, give });
    }

    fn withdraw_slot(&self, id: u64) {
        self.free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|slot| slot.id != id);
    }

    /// Unless another bridge has published since, whose file this no longer is.
    fn forget_file(&self) {
        let ours = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .is_some_and(|value| value["token"] == self.token.as_str());
        if ours {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct Leaving<'a> {
    doors: &'a Doors,
    peer: &'a str,
    door: Arc<Door>,
}

impl Drop for Leaving<'_> {
    fn drop(&mut self) {
        self.doors.leave(self.peer, &self.door);
    }
}

/// One standing stream, until it ends or the program it carries does.
fn keep(doors: &Doors, stream: &dyn LinkStream, peer: &str) -> Result<(), &'static str> {
    static NEXT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let door = doors.enter(peer)?;
    let leaving = Leaving { doors, peer, door };
    if stream.send(READY).is_err() {
        return Ok(());
    }
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    let (give, take) = mpsc::channel();
    leaving.door.offer_slot(id, give);
    while stream.life() == StreamLife::Open {
        match take.recv_timeout(POLL) {
            Ok(connection) => {
                if stream.send(CLAIMED).is_ok() {
                    relay(&connection, stream);
                }
                return Ok(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
    leaving.door.withdraw_slot(id);
    Ok(())
}

/// Lines both ways until the connection or the stream ends. A message is one line, so one with a line break in it would forge another.
fn relay(connection: &TcpStream, stream: &dyn LinkStream) {
    if connection.set_nonblocking(true).is_err() {
        return;
    }
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let stop = &stop;
        scope.spawn(move || {
            read_lines(connection, stream, stop);
            stop.store(true, Ordering::SeqCst);
        });
        while !stop.load(Ordering::SeqCst) {
            match stream.receive(Some(POLL)) {
                Ok(Some(message)) => {
                    let written = !message.iter().any(|byte| matches!(byte, b'\n' | b'\r'))
                        && write_line(connection, &message, stop);
                    if !written {
                        break;
                    }
                }
                Err(Failure::Timeout) => {}
                Ok(None) | Err(_) => break,
            }
        }
        stop.store(true, Ordering::SeqCst);
    });
    let _ = connection.shutdown(Shutdown::Both);
}

fn read_lines(mut connection: &TcpStream, stream: &dyn LinkStream, stop: &AtomicBool) {
    let mut line = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    while !stop.load(Ordering::SeqCst) {
        let count = match connection.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(RELAY_POLL);
                continue;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        for part in buffer[..count].split_inclusive(|byte| *byte == b'\n') {
            line.extend_from_slice(part);
            if line.len() > MAX_LINE + 2 {
                return;
            }
            if line.last() != Some(&b'\n') {
                continue;
            }
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !line.is_empty() && !deliver(stream, &line, stop) {
                return;
            }
            line.clear();
        }
    }
}

/// One line to a non-blocking socket, all of it or `false`: a partial write goes on from where it stopped, and a program that takes
/// nothing for [`STUCK`] has stopped reading.
fn write_line(mut connection: &TcpStream, message: &[u8], stop: &AtomicBool) -> bool {
    let mut line = Vec::with_capacity(message.len() + 1);
    line.extend_from_slice(message);
    line.push(b'\n');
    let mut done = 0;
    let mut moved = Instant::now();
    while done < line.len() {
        match connection.write(&line[done..]) {
            Ok(0) => return false,
            Ok(count) => {
                done += count;
                moved = Instant::now();
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if moved.elapsed() >= STUCK || stop.load(Ordering::SeqCst) {
                    return false;
                }
                thread::sleep(RELAY_POLL);
            }
            Err(_) => return false,
        }
    }
    true
}

fn deliver(stream: &dyn LinkStream, line: &[u8], stop: &AtomicBool) -> bool {
    let until = Instant::now() + STUCK;
    loop {
        match stream.send(line) {
            Ok(()) => return true,
            Err(Failure::NoProgress) if Instant::now() < until && !stop.load(Ordering::SeqCst) => {
                thread::sleep(POLL);
            }
            Err(_) => return false,
        }
    }
}

/// What to paste into an MCP client's configuration, for the tools of `peer`.
///
/// # Errors
///
/// When `peer` or `name` cannot be used, or this program cannot find itself.
pub fn config(
    helper: &crate::helper::Helper,
    home: Option<&Path>,
    peer: &str,
    name: &str,
) -> Result<String, String> {
    if hex_of(peer).is_none() {
        return Err("a peer id is sha256: and 64 lowercase hex digits".to_owned());
    }
    let named = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if !named {
        return Err("a server's name is ASCII letters, digits, - and _, up to 64".to_owned());
    }
    let program = helper
        .program
        .to_str()
        .ok_or("this program's path is not text")?;
    // A client may start a server with little of its own environment, and what home is depends on that.
    let home = home
        .map(Path::to_path_buf)
        .or_else(home::dir)
        .ok_or("no home directory for this user")?;
    let home = home.to_str().ok_or("this user's home is not text")?;
    let config = json!({
        "mcpServers": {
            name: {
                "type": "stdio",
                "command": program,
                "args": helper.command(&["mcp-shim", "--peer", peer]),
                "env": { home::HOME_ENV: home },
            }
        }
    });
    serde_json::to_string_pretty(&config).map_err(|error| error.to_string())
}

/// Finds this peer's door, shows the token and checks what comes back. `Err` says why there is nothing to talk to.
fn reach(dir: &Path, peer: &str) -> Result<TcpStream, String> {
    let nobody = |why: &str| {
        format!(
            "Nothing is here for {peer}: {why}. The browser has to be running with this turned on for this machine under Canoa browser \
             permissions, and coqui-acp-bridge serve --mcp-offers has to be running here."
        )
    };
    let path = file_for(dir, peer).map_err(str::to_owned)?;
    let text = read_private(&path).map_err(|_| nobody("no browser is connected"))?;
    let door: Value = serde_json::from_str(&text).map_err(|_| nobody("no browser is connected"))?;
    let (Some(address), Some(token), Some(answer)) = (
        door["address"].as_str(),
        door["token"].as_str(),
        door["answer"].as_str(),
    ) else {
        return Err(nobody("no browser is connected"));
    };
    let address: std::net::SocketAddr = address
        .parse()
        .map_err(|_| nobody("no browser is connected"))?;
    // The token would go to whoever is there, so it goes nowhere but this machine.
    if !address.ip().is_loopback() {
        return Err(nobody("the address it names is not on this machine"));
    }
    let mut connection = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|_| nobody("no browser is connected"))?;
    connection
        .write_all(format!("{token}\n").as_bytes())
        .map_err(|_| nobody("no browser is connected"))?;
    let heard = read_answer(&connection).ok_or_else(|| nobody("no browser is connected"))?;
    let (shown, word) = heard.split_at(TOKEN_BYTES * 2);
    // Not a secret worth a comparison in constant time: it is shown to whoever holds the token.
    if shown != answer.as_bytes() {
        return Err(nobody("what answered is not that bridge"));
    }
    match word {
        b" ok" => {
            connection
                .set_nonblocking(false)
                .map_err(|_| nobody("no browser is connected"))?;
            Ok(connection)
        }
        _ => Err(nobody("every stream it offers is in use")),
    }
}

/// Exactly the door's answer and no further, since what follows it is MCP's. Polled, for the reason [`RELAY_POLL`] gives.
fn read_answer(mut connection: &TcpStream) -> Option<Vec<u8>> {
    connection.set_nonblocking(true).ok()?;
    let until = Instant::now() + ANSWER_WAIT;
    let mut heard = vec![0_u8; ANSWER_LENGTH];
    let mut have = 0;
    while have < heard.len() {
        match connection.read(&mut heard[have..]) {
            Ok(0) => return None,
            Ok(count) => have += count,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= until {
                    return None;
                }
                thread::sleep(RELAY_POLL);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(heard)
}

#[cfg(unix)]
fn read_private(path: &Path) -> io::Result<String> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "others can reach it",
        ));
    }
    fs::read_to_string(path)
}

/// [`owner_only`] made it, and this does not read its ACL back.
#[cfg(not(unix))]
fn read_private(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

/// Says why on standard error, and to the client as an answer to its first message, which is usually what its user gets to see.
fn tell_client(why: &str) -> u8 {
    eprintln!("coqui-acp-bridge: {why}");
    let (send, heard) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_ok() {
            let _ = send.send(line);
        }
    });
    if let Ok(line) = heard.recv_timeout(FIRST_LINE_WAIT) {
        let id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|message| message.get("id").cloned());
        if let Some(id) = id {
            let answer = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": why },
            });
            println!("{answer}");
        }
    }
    1
}

/// The stdio server of what `mcp-config` prints. It carries bytes, since the bridge's relay holds each line to its limits.
pub fn shim(peer: &str, home: Option<&Path>) -> Result<u8, String> {
    let connection = match reach(&dir(home)?, peer) {
        Ok(connection) => connection,
        Err(why) => return Ok(tell_client(&why)),
    };
    let connection = Arc::new(connection);
    let upstream = Arc::clone(&connection);
    let client_left = Arc::new(AtomicBool::new(false));
    let left = Arc::clone(&client_left);
    // On its own thread, and left behind: a read of standard input cannot be interrupted, and returning ends the program. The client has
    // gone when it ends, so this ends both ways at once, which also tells the read below.
    thread::spawn(move || {
        let mut input = io::stdin().lock();
        let mut buffer = [0_u8; 16 * 1024];
        while let Ok(count) = input.read(&mut buffer) {
            if count == 0 || (&*upstream).write_all(&buffer[..count]).is_err() {
                break;
            }
        }
        left.store(true, Ordering::SeqCst);
        let _ = upstream.shutdown(Shutdown::Both);
    });
    // Reads and writes, not io::copy: on Linux that splices a socket into a piped standard output, and the client got nothing that way.
    let mut output = io::stdout().lock();
    let mut buffer = [0_u8; 16 * 1024];
    while let Ok(count) = (&*connection).read(&mut buffer) {
        if count == 0
            || output
                .write_all(&buffer[..count])
                .and_then(|()| output.flush())
                .is_err()
        {
            break;
        }
    }
    if !client_left.load(Ordering::SeqCst) {
        eprintln!("coqui-acp-bridge: the browser ended this server's connection");
    }
    Ok(0)
}
