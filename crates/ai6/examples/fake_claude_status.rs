//! A stand-in for `claude auth status` that the renewer's tests run: it renews the login in the
//! configuration directory it is given, as Claude Code does in the last minutes of one, and prints
//! the answer Claude Code prints. It signs in to nothing and reaches no network.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments != ["auth", "status"] {
        eprintln!("this stand-in answers `auth status` only");
        return std::process::ExitCode::from(2);
    }
    let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) else {
        eprintln!("no configuration directory");
        return std::process::ExitCode::from(1);
    };
    let file = dir.join(".credentials.json");
    let Ok(text) = std::fs::read_to_string(&file) else {
        println!("{{\"loggedIn\":false}}");
        return std::process::ExitCode::from(1);
    };
    let Ok(mut login) = serde_json::from_str::<serde_json::Value>(&text) else {
        return std::process::ExitCode::from(1);
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let oauth = &mut login["claudeAiOauth"];
    oauth["accessToken"] = "access-rotated".into();
    oauth["refreshToken"] = "refresh-rotated".into();
    oauth["expiresAt"] = u64::try_from(now + 8 * 3_600_000)
        .unwrap_or(u64::MAX)
        .into();
    if std::fs::write(&file, login.to_string()).is_err() {
        return std::process::ExitCode::from(1);
    }
    println!("{{\"loggedIn\":true}}");
    std::process::ExitCode::SUCCESS
}
