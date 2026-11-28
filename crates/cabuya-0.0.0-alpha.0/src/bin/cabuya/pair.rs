//! `pair --join`: pair with a waiting listener by its PIN, and keep its key under a name.

use std::io::BufRead as _;

use cabuya::pairing;
use coqui_peer::pair::{CodeAlphabet, Pin};

use crate::PairArgs;
use crate::home::{Home, Host};
use crate::wire;

/// The PIN from `--pin`, or one line of standard input.
fn pin_of(given: Option<&str>) -> Result<Pin, String> {
    let text = match given {
        Some(text) => text.to_owned(),
        None => {
            eprintln!("cabuya: PIN:");
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .map_err(|error| format!("the PIN could not be read: {error}"))?;
            line
        }
    };
    Pin::parse(&CodeAlphabet::pin(), text.trim()).map_err(|_| "that is not a PIN".to_owned())
}

pub fn join(
    home: &Home,
    method: Option<cabuya::auth::Method>,
    address: &str,
    args: &PairArgs,
) -> Result<u8, String> {
    let name = args
        .name
        .as_deref()
        .ok_or("--join needs --name, the name this host is kept under")?;
    let pin = pin_of(args.pin.as_deref())?;
    let identity = home.identity(method)?;
    let dialed = home.dialed()?;
    let channel = wire::channel(wire::open(address)?)?;
    let paired = pairing::join_shared(&channel, &identity, pin, &dialed, wire::PROOF_WITHIN);
    coqui_transport::Channel::close(&channel);
    let paired = paired.map_err(|error| error.to_string())?;
    let peer = paired.peer.id();
    let at = args.at.as_deref().unwrap_or(address);
    home.put_host(
        name,
        &Host {
            peer,
            address: at.to_owned(),
        },
    )?;
    println!("paired {name} {peer}");
    println!("this-peer {}", identity.peer_id());
    Ok(0)
}
