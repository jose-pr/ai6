//! `discover`: search this machine and the link for agents, one line of JSON each. What it prints is what they claim.

use std::time::Duration;

use coqui_agentic_peer::discovery;

/// Prints each candidate found in `seconds` as one line of JSON on standard output.
///
/// # Errors
///
/// Never today: the result type is the commands'.
pub fn run(seconds: u64) -> Result<u8, String> {
    for candidate in discovery::discover(Duration::from_secs(seconds)) {
        println!("{}", candidate.to_json());
    }
    Ok(0)
}
