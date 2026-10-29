//! The detached wrapper: how a Station runs a harness on a POSIX Theater that has no runner.
//!
//! The harness is started over the Theater's transport in a session of its own (`setsid`, or
//! `nohup` where there is none), so the link that started it can drop without ending it. Its
//! output is appended to a file and its input read from a FIFO it holds open itself. While the
//! link is down a read finds nothing new; once the transport reconnects, reads resume from the
//! last byte taken. The Station still supervises the harness: one whose Station stops is
//! left behind, and the next start for the same Mission ends it before starting another. A
//! harness out of reach, or quiet, for its [`ReachWatch`] grace is probed once over the
//! transport; one that is gone without recording an exit is given up as lost.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use guatiao::library::{Object, ProviderError};
use guatiao::{Bytes, FromValue, Status, ToValue, Value};

use crate::directorates::q::terminal::{ExitInfo, READ_TIMED_OUT, Spec, Terminal, TerminalSession};
use crate::directorates::q::transport::kit::{is_variable_name, quote};
use crate::directorates::q::transport::{ReadSpec, RunSpec, TransportClient, describe};
use crate::station::limits::DetachedLimits;
use crate::station::reach::ReachWatch;

/// Where wrapped harnesses keep their files: relative to the transport's default directory,
/// the login user's home over SSH.
pub(crate) const ROOT: &str = ".ai6/detached";
/// The name a launch record gives the wrapper in place of a terminal provider.
pub(crate) const TERMINAL: &str = "detached";

/// Makes the harness's directory: ends a harness an earlier start of the same Mission left,
/// then makes the environment and input FIFOs, the output file and the wrapper script, and
/// prints the directory's absolute path. The environment passes through a FIFO, so no value
/// (a leased credential among them) is ever written to the Theater's disk. The wrapper reads
/// it with `cat` and `eval`s it: bash 3.2, macOS's `sh`, sources a FIFO as an empty file.
const SETUP: &str = r#"set -e
umask 077
mkdir -p "$1"
cd "$1"
for old in "$2"-*; do
  [ -f "$old/pid" ] && [ ! -f "$old/exit" ] || continue
  pid=$(cat "$old/pid")
  kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
done
mkdir "$3"
cd "$3"
mkfifo env
mkfifo in
: > out
cat > run.sh <<'WRAPPER'
echo $$ > pid
ai6_env=$(cat ./env)
rm -f ./env
eval "$ai6_env"
unset ai6_env
ai6_dir=$(pwd -P)
ai6_cwd=$1
shift
cd "$ai6_cwd" || { echo 127 > "$ai6_dir/exit"; exit 127; }
exec 3<>"$ai6_dir/in"
"$@" <&3 >>"$ai6_dir/out" 2>&1
ai6_code=$?
echo "$ai6_code" > "$ai6_dir/exit.tmp" && mv "$ai6_dir/exit.tmp" "$ai6_dir/exit"
WRAPPER
pwd -P
"#;

/// Starts the wrapper detached from this link, hands it the environment (standard input)
/// through its FIFO, and prints the harness session's process ID.
const LAUNCH: &str = r#"cd "$1" || exit 44
shift
if command -v setsid >/dev/null 2>&1; then
  setsid sh ./run.sh "$@" </dev/null >/dev/null 2>&1 &
else
  nohup sh ./run.sh "$@" </dev/null >/dev/null 2>&1 &
fi
cat > env
i=0
while [ ! -s pid ] && [ "$i" -lt 200 ]; do sleep 0.05; i=$((i + 1)); done
cat pid
"#;

/// Exits 0 while the harness runs or once its exit is recorded; anything else means it is gone.
const ALIVE: &str = r#"[ -f "$1/exit" ] || kill -0 "$2" 2>/dev/null
"#;

/// Ends the harness's session (its process group under `setsid`, else the wrapper).
const KILL: &str = r#"pid=$(cat "$1/pid") || exit 0
kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
"#;

fn refuse(message: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &message.to_string())
}

/// Starts harnesses on one Theater for one Mission.
pub(crate) struct DetachedTerminal {
    client: Arc<TransportClient>,
    limits: DetachedLimits,
    prefix: String,
    relink: Option<Arc<dyn Fn() + Send + Sync>>,
    reach: ReachWatch,
}

impl DetachedTerminal {
    /// The wrapper for `mission_id`'s harnesses over `client`, within `limits`.
    pub(crate) fn new(
        client: Arc<TransportClient>,
        mission_id: &str,
        limits: DetachedLimits,
    ) -> Self {
        let mut prefix: String = mission_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .take(limits.name_chars.or_max())
            .collect();
        if prefix.starts_with('.') || prefix.starts_with('-') {
            prefix.insert(0, 'm');
        }
        Self {
            client,
            limits,
            prefix,
            relink: None,
            reach: ReachWatch::default(),
        }
    }

    /// Notes in `reach` whether the harnesses this starts can be reached, and gives them up
    /// there when they are gone.
    pub(crate) fn watched_by(mut self, reach: ReachWatch) -> Self {
        self.reach = reach;
        self
    }

    /// Runs `relink` when a read gets through again after the link was down.
    #[cfg_attr(
        not(feature = "cables-mcp"),
        allow(dead_code, reason = "only the Cable forward needs re-opening")
    )]
    pub(crate) fn on_relink(mut self, relink: impl Fn() + Send + Sync + 'static) -> Self {
        self.relink = Some(Arc::new(relink));
        self
    }

    fn script(
        &self,
        script: &str,
        arguments: impl IntoIterator<Item = String>,
        stdin: Option<Vec<u8>>,
    ) -> Result<String, ProviderError> {
        let mut argv = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            script.to_owned(),
            "sh".to_owned(),
        ];
        argv.extend(arguments);
        run_script(&self.client, argv, stdin, &self.limits)
    }
}

fn run_script(
    client: &TransportClient,
    argv: Vec<String>,
    stdin: Option<Vec<u8>>,
    limits: &DetachedLimits,
) -> Result<String, ProviderError> {
    let output = client.run(&RunSpec {
        argv,
        stdin: stdin.map(Bytes),
        timeout_ms: millis(limits.script_timeout.0),
        ..RunSpec::default()
    })?;
    if output.code != Some(0) {
        return Err(refuse(format!(
            "the detached wrapper failed on the Theater ({}): {}",
            output
                .code
                .map_or_else(|| "no exit status".to_owned(), |code| code.to_string()),
            String::from_utf8_lossy(&output.stderr.0).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout.0).trim().to_owned())
}

/// The environment file the wrapper sources: one `export NAME='value'` line each.
fn environment_file(spec: &Spec) -> Result<Vec<u8>, ProviderError> {
    let mut text = String::new();
    for (name, value) in spec.env.iter().flatten() {
        if !is_variable_name(name) || value.contains('\0') {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_BAD_VALUE,
                &format!("{name:?} cannot be passed to a harness through a shell"),
            ));
        }
        text.push_str(&format!("export {name}={}\n", quote(value)));
    }
    Ok(text.into_bytes())
}

impl Terminal for DetachedTerminal {
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let spec = Spec::from_value(spec).map_err(|error| {
            ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &error.to_string())
        })?;
        if spec.argv.is_empty() || spec.argv.iter().any(|word| word.contains('\0')) {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_BAD_VALUE,
                "argv is empty or holds a NUL byte",
            ));
        }
        let name = format!(
            "{}-{}",
            self.prefix,
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        );
        let environment = environment_file(&spec)?;
        let dir = self.script(SETUP, [ROOT.to_owned(), self.prefix.clone(), name], None)?;
        let mut arguments = vec![
            dir.clone(),
            spec.cwd.clone().unwrap_or_else(|| ".".to_owned()),
        ];
        arguments.extend(spec.argv.iter().cloned());
        let pid = self
            .script(LAUNCH, arguments, Some(environment))?
            .parse::<i64>()
            .map_err(|_| refuse("the detached wrapper did not report the harness's process"))?;
        Ok(DetachedSession {
            client: self.client.clone(),
            limits: self.limits.clone(),
            dir,
            pid,
            offset: Mutex::new(0),
            killed: AtomicBool::new(false),
            unreachable: AtomicBool::new(false),
            relink: self.relink.clone(),
            reach: self.reach.clone(),
        }
        .into_object())
    }

    /// The harness reads a FIFO and writes a file.
    fn tty(&self) -> bool {
        false
    }
}

/// One harness under the wrapper, read and written through the transport.
struct DetachedSession {
    client: Arc<TransportClient>,
    limits: DetachedLimits,
    dir: String,
    pid: i64,
    offset: Mutex<u64>,
    killed: AtomicBool,
    unreachable: AtomicBool,
    relink: Option<Arc<dyn Fn() + Send + Sync>>,
    reach: ReachWatch,
}

impl DetachedSession {
    fn path(&self, name: &str) -> String {
        format!("{}/{name}", self.dir)
    }

    /// Asks the Theater, over the transport, whether the harness still runs or has exited.
    fn probe(&self) -> Result<(), String> {
        let output = self
            .client
            .run(&RunSpec {
                argv: vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    ALIVE.to_owned(),
                    "sh".to_owned(),
                    self.dir.clone(),
                    self.pid.to_string(),
                ],
                timeout_ms: millis(self.limits.script_timeout.0),
                ..RunSpec::default()
            })
            .map_err(|error| describe(&error))?;
        match output.code {
            Some(0) => Ok(()),
            _ => Err(format!(
                "the harness's process {} is gone and recorded no exit",
                self.pid
            )),
        }
    }

    /// Output after the last byte taken, up to `limit` bytes; `None` when the link is down.
    fn output(&self, limit: usize) -> Option<Vec<u8>> {
        let offset = *self
            .offset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match self.client.read_file(&ReadSpec {
            path: self.path("out"),
            offset,
            limit: limit as u64,
        }) {
            Ok(read) => {
                if self.unreachable.swap(false, Ordering::AcqRel) {
                    eprintln!(
                        "ai6: detached harness {} is reachable again; reading from byte {offset}",
                        self.dir
                    );
                    if let Some(relink) = &self.relink {
                        relink();
                    }
                }
                Some(read.data.0)
            }
            Err(error) => {
                self.reach.missed(|| self.probe());
                if !self.unreachable.swap(true, Ordering::AcqRel) {
                    eprintln!(
                        "ai6: detached harness {} is unreachable; it keeps running: {}",
                        self.dir,
                        describe(&error)
                    );
                }
                None
            }
        }
    }

    /// The harness's exit code, once the wrapper has recorded it.
    fn exit_code(&self) -> Option<i64> {
        let read = self
            .client
            .read_file(&ReadSpec {
                path: self.path("exit"),
                offset: 0,
                limit: 64,
            })
            .ok()?;
        String::from_utf8_lossy(&read.data.0).trim().parse().ok()
    }

    fn take(&self, bytes: &[u8], dst: &mut [u8]) -> i64 {
        let count = bytes.len().min(dst.len());
        dst[..count].copy_from_slice(&bytes[..count]);
        *self
            .offset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += count as u64;
        count as i64
    }
}

impl TerminalSession for DetachedSession {
    fn pid(&self) -> i64 {
        self.pid
    }

    fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError> {
        let deadline =
            Instant::now() + Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0));
        loop {
            if self.killed.load(Ordering::Acquire) {
                return Ok(0);
            }
            if let Some(bytes) = self.output(dst.len()) {
                if !bytes.is_empty() {
                    self.reach.reached();
                    return Ok(self.take(&bytes, dst));
                }
                // The exit code is written after the last output, so output found after it
                // is the end of what there is.
                if self.exit_code().is_some() {
                    let rest = self.output(dst.len()).unwrap_or_default();
                    return Ok(self.take(&rest, dst));
                }
                // A quiet harness is probed once per grace, to notice one that died unrecorded.
                self.reach.missed(|| self.probe());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(READ_TIMED_OUT);
            }
            std::thread::sleep(remaining.min(self.limits.poll.0));
        }
    }

    fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
        run_script(
            &self.client,
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                r#"cat >> "$1""#.to_owned(),
                "sh".to_owned(),
                self.path("in"),
            ],
            Some(data.to_vec()),
            &self.limits,
        )
        .map(drop)
    }

    fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
        // The harness has no terminal of its own to resize.
        Ok(())
    }

    fn kill(&self) -> Result<(), ProviderError> {
        self.killed.store(true, Ordering::Release);
        run_script(
            &self.client,
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                KILL.to_owned(),
                "sh".to_owned(),
                self.dir.clone(),
            ],
            None,
            &self.limits,
        )
        .map(drop)
    }

    fn exit_status(&self, timeout_ms: i64) -> Result<Value, ProviderError> {
        let deadline =
            Instant::now() + Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0));
        let status = loop {
            if let Some(code) = self.exit_code() {
                break ExitInfo {
                    running: false,
                    code: Some(code),
                    signal: None,
                };
            }
            if self.killed.load(Ordering::Acquire) {
                break ExitInfo {
                    running: false,
                    code: None,
                    signal: Some(15),
                };
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break ExitInfo {
                    running: true,
                    code: None,
                    signal: None,
                };
            }
            std::thread::sleep(remaining.min(self.limits.poll.0));
        };
        status.to_value().map_err(|error| refuse(error.to_string()))
    }
}

/// A duration as the whole milliseconds a transport request takes.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_file_quotes_every_value_and_refuses_what_a_shell_cannot_carry() {
        let spec = |env: &[(&str, &str)]| Spec {
            argv: vec!["claude".to_owned()],
            cwd: None,
            env: Some(
                env.iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            ),
            rows: 24,
            cols: 80,
        };
        let text = environment_file(&spec(&[("A", "it's $HOME"), ("B", "")])).unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "export A='it'\\''s $HOME'\nexport B=''\n"
        );
        assert!(environment_file(&spec(&[("NOT-A-NAME", "x")])).is_err());
        assert!(environment_file(&spec(&[("A", "nul\0")])).is_err());
    }

    #[test]
    fn a_mission_id_becomes_a_safe_directory_prefix() {
        let client = Arc::new(TransportClient::new(Box::new(
            crate::station::transports::testing::DirTransport::new(std::path::Path::new(".")),
        )));
        assert_eq!(
            DetachedTerminal::new(client.clone(), "mission-1", DetachedLimits::default()).prefix,
            "mission-1"
        );
        assert_eq!(
            DetachedTerminal::new(client.clone(), "a b/c*", DetachedLimits::default()).prefix,
            "a_b_c_"
        );
        assert_eq!(
            DetachedTerminal::new(client, "-x", DetachedLimits::default()).prefix,
            "m-x"
        );
    }
}
