//! Writes what the repository generates from the settings' declarations; `--check` writes
//! nothing and fails when any of it differs. `--root DIR` works on a copy of the repository.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut check = false;
    let mut root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match (argument.as_str(), arguments.len()) {
            ("--check", _) => check = true,
            ("--root", 1..) => root = arguments.next().map(PathBuf::from).unwrap_or(root),
            _ => {
                eprintln!("usage: settings_reference [--check] [--root DIR]");
                return ExitCode::from(2);
            }
        }
    }
    let result = if check {
        ai6::settings::reference::stale(&root).map(|stale| {
            for path in &stale {
                eprintln!("{path} is not what the declarations say");
            }
            stale.is_empty()
        })
    } else {
        ai6::settings::reference::write(&root).map(|()| true)
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("run: cargo run -p ai6 --features cli --example settings_reference");
            ExitCode::FAILURE
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}
