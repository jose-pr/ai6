//! The `coqui-acp-history` command: look at histories and move them between files.
//!
//! `export` and `import` need a harness's codec, which lives in that harness's crate; here they
//! say so and fail. `convert` writes a history in this format to another file of the same format:
//! its lines as they are, unless redaction that removes something or a digest is asked for.
//!
//! Everything a history says is printed with the characters a terminal acts on written as
//! `\u{..}`: a history comes from elsewhere.

use core::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};

use coqui_acp_history::{
    Body, Document, HistoryError, Reader, ReaderSettings, RedactionSettings, Redactor, non_defaults,
};

#[derive(Parser)]
#[command(
    name = "coqui-acp-history",
    version,
    about = "Look at and move Agent Client Protocol session histories"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The sizes a history is held to. Each is a setting with a default; a looser one is reported.
#[derive(Args)]
struct Limits {
    /// The longest line, in bytes.
    #[arg(long)]
    max_line_bytes: Option<u64>,
    /// The most lines.
    #[arg(long)]
    max_lines: Option<u64>,
    /// The most bytes in all.
    #[arg(long)]
    max_total_bytes: Option<u64>,
    /// The deepest nesting in a line (the parser stops at 127 whatever this says).
    #[arg(long)]
    max_depth: Option<u64>,
    /// The most values in a line.
    #[arg(long)]
    max_values: Option<u64>,
}

impl Limits {
    /// The settings to read with. A history held in memory as a whole starts from a smaller
    /// `max_total_bytes` than one that is only streamed through.
    fn settings(&self, in_memory: bool) -> ReaderSettings {
        let mut settings = if in_memory {
            ReaderSettings::for_document()
        } else {
            ReaderSettings::default()
        };
        let bounds = &mut settings.bounds;
        bounds.max_line_bytes = self.max_line_bytes.unwrap_or(bounds.max_line_bytes);
        bounds.max_lines = self.max_lines.unwrap_or(bounds.max_lines);
        bounds.max_total_bytes = self.max_total_bytes.unwrap_or(bounds.max_total_bytes);
        bounds.max_depth = self.max_depth.unwrap_or(bounds.max_depth);
        bounds.max_values = self.max_values.unwrap_or(bounds.max_values);
        for setting in non_defaults(&settings) {
            eprintln!(
                "setting {} is {} (default {})",
                setting.setting, setting.value, setting.default
            );
        }
        settings
    }
}

#[derive(Subcommand)]
enum Command {
    /// List the histories in a directory (the `.jsonl` files), by their headers.
    List {
        /// The directory; the current one by default.
        dir: Option<PathBuf>,
        #[command(flatten)]
        limits: Limits,
    },
    /// Show a history: its header, what it holds, and its turns.
    Show {
        /// The history file.
        file: PathBuf,
        #[command(flatten)]
        limits: Limits,
    },
    /// Export a harness's session to a history (`coqui-acp-claude history export` and `coqui-acp-codex history export` do it).
    Export {
        /// The harness: claude, codex, agy.
        #[arg(long)]
        harness: String,
        /// The session, in the harness's own id.
        session: String,
        /// Where to write the history.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Import a history into a harness (needs that harness's codec).
    Import {
        /// The harness: claude, codex, agy.
        #[arg(long)]
        harness: String,
        /// The history file.
        file: PathBuf,
    },
    /// Write a history to another file of the same format. Its lines are copied as they are, after
    /// the whole history has been checked, unless redaction that removes something or --seal is
    /// asked for: then the history is rewritten in canonical spelling.
    Convert {
        /// The history to read.
        input: PathBuf,
        /// The file to write; `-` for standard output.
        #[arg(short, long)]
        output: PathBuf,
        /// A regular expression whose matches are removed. Repeatable.
        #[arg(long = "redact-pattern")]
        redact_patterns: Vec<String>,
        /// A file of text to remove, one per line. A secret is read from a file, never from the
        /// command line.
        #[arg(long = "redact-values-file")]
        redact_values: Option<PathBuf>,
        /// What replaces each removal.
        #[arg(long)]
        replacement: Option<String>,
        /// Rewrite the history in canonical spelling with a digest of its body in the header
        /// (a history that redaction changed loses its digest otherwise).
        #[arg(long)]
        seal: bool,
        #[command(flatten)]
        limits: Limits,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, HistoryError> {
    match cli.command {
        Command::List { dir, limits } => list(
            dir.as_deref().unwrap_or(Path::new(".")),
            &limits.settings(false),
        ),
        Command::Show { file, limits } => show(&file, &limits.settings(true)),
        Command::Export { harness, .. } | Command::Import { harness, .. } => {
            eprintln!(
                "this command cannot read a harness's own files: run `coqui-acp-{}` `history export` or                  `history import` (the program built with its `history` feature)",
                harness
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .collect::<String>()
            );
            Ok(ExitCode::from(2))
        }
        Command::Convert {
            input,
            output,
            redact_patterns,
            redact_values,
            replacement,
            seal,
            limits,
        } => {
            let redaction = if redact_patterns.is_empty() && redact_values.is_none() {
                None
            } else {
                let mut settings = RedactionSettings::default();
                settings.patterns = redact_patterns;
                if let Some(path) = redact_values {
                    let text = fs::read_to_string(path)?;
                    settings.values = text
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
                if let Some(replacement) = replacement {
                    settings.replacement = replacement;
                }
                Some(settings)
            };
            convert(&input, &output, redaction, seal, &limits)
        }
    }
}

/// `text` with each character a terminal acts on, or that could move or hide text in a listing,
/// written as `\u{..}`: the controls (C0, DEL, C1), the line and paragraph separators and the
/// bidirectional controls.
fn safe(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let hidden = c.is_control()
            || matches!(
                c,
                '\u{61c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{2028}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            );
        if hidden {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

fn shown(path: &Path) -> String {
    safe(&path.to_string_lossy())
}

fn open(path: &Path, settings: &ReaderSettings) -> Result<Reader<File>, HistoryError> {
    Reader::builder(File::open(path)?)
        .settings(settings.clone())
        .open()
}

fn list(dir: &Path, settings: &ReaderSettings) -> Result<ExitCode, HistoryError> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    paths.sort();
    for path in paths {
        match open(&path, settings) {
            Ok(reader) => {
                let header = reader.header();
                println!(
                    "{}  {}  {}  {}  {}",
                    header.id,
                    header.origin.harness,
                    safe(header.model.as_deref().unwrap_or("-")),
                    safe(header.cwd.as_deref().unwrap_or("-")),
                    shown(&path)
                );
            }
            Err(error) => println!(
                "-  not a history ({}: {error})  {}",
                error.code(),
                shown(&path)
            ),
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn show(path: &Path, settings: &ReaderSettings) -> Result<ExitCode, HistoryError> {
    let document = Document::from_reader(open(path, settings)?)?;
    let header = &document.header;
    println!("session   {}", header.id);
    println!(
        "origin    {} {}",
        header.origin.harness,
        safe(header.origin.version.as_deref().unwrap_or(""))
    );
    for (name, value) in [
        ("origin id", &header.origin.session_id),
        ("model", &header.model),
        ("mode", &header.mode),
        ("effort", &header.effort),
        ("cwd", &header.cwd),
        ("created", &header.created_at),
    ] {
        if let Some(value) = value {
            println!("{name:<9} {}", safe(value));
        }
    }
    if let Some(redacted) = header.redacted {
        println!(
            "redacted  {} removals by {} rules",
            redacted.replaced, redacted.rules
        );
    }
    let summary = document.check()?;
    println!(
        "lines     {} ({} bytes), {} turns, {} updates, {} compactions{}",
        summary.lines,
        summary.bytes,
        summary.turns,
        summary.updates,
        summary.compactions,
        if summary.open_turn {
            ", ends inside a turn"
        } else {
            ""
        }
    );
    let seed = document.seed();
    if let Some(point) = &seed.point {
        println!(
            "seed      from line {} ({}), {} turns after it{}",
            point.line,
            if seed.summary.is_some() {
                "a compaction summary"
            } else {
                "the start"
            },
            seed.turns(),
            if seed.mid_turn {
                ", the first cut mid-turn"
            } else {
                ""
            }
        );
    }
    let mut turn = 0;
    for (at, line) in document.lines.iter().enumerate() {
        match &line.body {
            Body::Prompt { prompt } => {
                turn += 1;
                let text = prompt
                    .iter()
                    .find_map(|block| block.get("text").and_then(|t| t.as_str()))
                    .unwrap_or("(no text)");
                let excerpt: String = text.chars().take(72).collect();
                println!(
                    "turn {turn:<4} line {:<6} {}",
                    at + 2,
                    safe(&excerpt.replace('\n', " "))
                );
            }
            Body::TurnEnd { stop, .. } => {
                println!("          line {:<6} ended: {}", at + 2, stop.name())
            }
            Body::Compaction { summary, .. } => println!(
                "          line {:<6} compaction{}",
                at + 2,
                if summary.is_some() {
                    " (with summary)"
                } else {
                    ""
                }
            ),
            _ => {}
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn convert(
    input: &Path,
    output: &Path,
    redaction: Option<RedactionSettings>,
    seal: bool,
    limits: &Limits,
) -> Result<ExitCode, HistoryError> {
    let mut bytes = Vec::new();
    let copy = |bytes: &mut Vec<u8>| -> Result<(), HistoryError> {
        let summary = open(input, &limits.settings(false))?.copy_to(bytes)?;
        eprintln!("copied {} lines unaltered", summary.lines);
        Ok(())
    };
    if redaction.is_none() && !seal {
        copy(&mut bytes)?;
    } else {
        let mut document = Document::from_reader(open(input, &limits.settings(true))?)?;
        let mut removed = 0;
        if let Some(redaction) = redaction {
            let mut redactor = Redactor::try_new(&redaction)?;
            let report = redactor.document(&mut document)?;
            removed = report.replaced;
            eprintln!(
                "redacted {} removals in {} lines",
                report.replaced, report.lines
            );
            for rule in &report.rules {
                eprintln!(
                    "  {:?} {}: {} removals in {} lines",
                    rule.kind, rule.index, rule.replaced, rule.lines
                );
            }
        }
        if removed == 0 && !seal {
            copy(&mut bytes)?;
        } else {
            if seal {
                document.seal()?;
            }
            document.to_writer(&mut bytes)?;
        }
    }
    write_whole(output, &bytes)?;
    Ok(ExitCode::SUCCESS)
}

/// Writes `bytes` to `output` whole or not at all: to a new file beside it that no one else could
/// have named first, then renamed over it.
fn write_whole(output: &Path, bytes: &[u8]) -> io::Result<()> {
    if output == Path::new("-") {
        return io::stdout().lock().write_all(bytes);
    }
    let directory = output.parent().filter(|d| !d.as_os_str().is_empty());
    let name = output.file_name().unwrap_or_default().to_string_lossy();
    let mut attempt = 0_u32;
    let (temporary, file) = loop {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let candidate = format!(".{name}.{}.{stamp:x}.{attempt}.part", std::process::id());
        let candidate = directory.map_or_else(|| PathBuf::from(&candidate), |d| d.join(&candidate));
        // `create_new` fails on any existing name, a link included, and follows none.
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 16 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    };
    let written = {
        let mut file = BufWriter::new(file);
        file.write_all(bytes).and_then(|()| file.flush())
    };
    match written.and_then(|()| fs::rename(&temporary, output)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}
