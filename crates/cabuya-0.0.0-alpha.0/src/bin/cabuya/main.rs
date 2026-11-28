//! `cabuya`: pair two machines, serve a link, and carry a named stream or a TCP port over it.
//!
//! Every command works in one per-user folder (`--home`, else `CABUYA_HOME`, else the user's configuration folder) that holds this
//! user's identity, the keys of the peers that may connect, the keys of the hosts this user dials, the grants and the hosts' names.
#![forbid(unsafe_code)]
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

mod dial;
mod grants;
mod home;
mod pair;
mod serve;
mod wire;

use std::path::PathBuf;
use std::process::ExitCode;

use cabuya::auth::Method;
use clap::{Args, Parser, Subcommand};
use coqui_peer::PeerId;

use crate::dial::{Mapping, Wanted};
use crate::home::Home;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// The folder of this user's keys and hosts. Default: `CABUYA_HOME`, else the configuration folder.
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,
    /// The proof's method, for an identity that does not exist yet: `noise` (default) or `webcrypto`. An identity keeps its own
    /// method, and naming another is refused.
    #[arg(long, global = true, value_name = "METHOD")]
    method: Option<Method>,
    /// The grants file, for `serve`, `pair`, `grant` and `revoke`. Default: `grants.json` in the folder. `serve` reads it on every question,
    /// so editing it counts at once.
    #[arg(long, global = true, value_name = "FILE")]
    grants: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print this user's peer id and method, creating the identity if there is none.
    Id,
    /// Show a PIN and wait for one peer to pair with it, or with `--join` pair with a waiting one.
    Pair(PairArgs),
    /// Listen, prove each peer that connects, and serve it what the grants file lets it open.
    Serve(ServeArgs),
    /// Let a paired peer open a protocol, or `tcp/1:HOST:PORT` to reach a target (a pattern is allowed).
    Grant {
        /// The peer's id, as `pair` printed it.
        peer: PeerId,
        /// Each protocol or `tcp/1:RULE` to grant.
        #[arg(required = true)]
        tokens: Vec<String>,
    },
    /// Take grants from a peer, or with none take its record and its key, so it cannot connect again.
    Revoke {
        /// The peer's id.
        peer: PeerId,
        /// The grants to take; without any, everything.
        tokens: Vec<String>,
    },
    /// Carry one stream to a paired host on standard input and output: an `ssh` ProxyCommand when it is a target.
    Connect(ConnectArgs),
    /// Listen on local ports and carry each connection to a target the paired host can reach.
    Forward {
        /// A paired host, by name or peer id.
        host: String,
        /// `[ADDRESS:]PORT=HOST:PORT`; repeat for more. The address defaults to loopback, and port 0 picks one.
        #[arg(long = "local", value_name = "MAPPING", required = true)]
        mappings: Vec<Mapping>,
    },
    /// List the hosts this user has paired with.
    Hosts,
    /// Forget a host: it is no longer dialed. Its own record of this key is its operator's.
    Forget {
        /// The host's name.
        name: String,
    },
}

#[derive(Args, Debug)]
struct PairArgs {
    /// Pair with the `cabuya pair` or `cabuya serve --pair` listening at this address.
    #[arg(long, value_name = "ADDRESS")]
    join: Option<String>,
    /// With `--join`: the name this host is kept under.
    #[arg(long, requires = "join")]
    name: Option<String>,
    /// With `--join`: where `cabuya serve` listens, when not at the address paired at.
    #[arg(long, value_name = "ADDRESS", requires = "join")]
    at: Option<String>,
    /// With `--join`: the PIN. Without it one line is read from standard input, so it is not left in an argument.
    #[arg(long, requires = "join")]
    pin: Option<String>,
    /// The address to listen on for a joiner.
    #[arg(long, value_name = "ADDRESS", default_value = serve::PAIR_ADDRESS, conflicts_with = "join")]
    listen: String,
    /// Each protocol or `tcp/1:RULE` the paired peer is granted at once.
    #[arg(long = "grant", value_name = "GRANT", conflicts_with = "join")]
    grant_tokens: Vec<String>,
}

#[derive(Args, Debug)]
struct ServeArgs {
    /// The address to listen on.
    #[arg(long, value_name = "ADDRESS", default_value = serve::SERVE_ADDRESS)]
    listen: String,
    /// Also take a pairing on this listener, with a PIN shown at start.
    #[arg(long)]
    pair: bool,
    /// With `--pair`: each protocol or `tcp/1:RULE` the paired peer is granted at once.
    #[arg(long = "grant", value_name = "GRANT", requires = "pair")]
    pair_grants: Vec<String>,
}

#[derive(Args, Debug)]
struct ConnectArgs {
    /// A paired host, by name or peer id.
    host: String,
    /// The target to reach through the host, `HOST:PORT` (as `tcp/1`).
    #[arg(required_unless_present = "stream", conflicts_with_all = ["stream", "metadata"])]
    target: Option<cabuya::forward::Target>,
    /// A stream by protocol name instead of a target.
    #[arg(long, value_name = "PROTOCOL")]
    stream: Option<String>,
    /// With `--stream`: the metadata the stream opens with.
    #[arg(long, requires = "stream", default_value = "")]
    metadata: String,
}

fn run(cli: Cli) -> Result<u8, String> {
    let home = Home::locate(cli.home)?;
    let (method, grants_file) = (cli.method, cli.grants);
    match cli.command {
        Command::Id => {
            let identity = home.identity(method)?;
            println!("{} {}", identity.peer_id(), identity.method());
            Ok(0)
        }
        Command::Pair(args) => match args.join.clone() {
            Some(address) => pair::join(&home, method, &address, &args),
            None => serve::pair(&home, method, &args.listen, grants_file, &args.grant_tokens),
        },
        Command::Serve(args) => serve::serve(
            &home,
            method,
            &args.listen,
            grants_file,
            args.pair,
            &args.pair_grants,
        ),
        Command::Grant { peer, tokens } => {
            grants::add(&home.grants(grants_file), peer, &tokens).map(|()| 0)
        }
        Command::Revoke { peer, tokens } => {
            grants::revoke(&home.grants(grants_file), &home.served()?, peer, &tokens).map(|()| 0)
        }
        Command::Connect(args) => {
            let wanted = match (args.target, args.stream) {
                (Some(target), _) => Wanted::Target(target),
                (None, Some(protocol)) => Wanted::Stream {
                    protocol,
                    metadata: args.metadata.into_bytes(),
                },
                (None, None) => return Err("give a target or --stream".to_owned()),
            };
            dial::connect(&home, method, &args.host, &wanted)
        }
        Command::Forward { host, mappings } => dial::forward(&home, method, &host, &mappings),
        Command::Hosts => {
            for (name, host) in home.hosts()? {
                println!("{name} {} {}", host.peer, host.address);
            }
            Ok(0)
        }
        Command::Forget { name } => match home.forget_host(&name)? {
            Some(_) => Ok(0),
            None => Err(format!("no paired host is called {name}")),
        },
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(why) => {
            eprintln!("cabuya: {why}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    #[test]
    fn the_command_line_is_consistent() {
        super::Cli::command().debug_assert();
    }
}
