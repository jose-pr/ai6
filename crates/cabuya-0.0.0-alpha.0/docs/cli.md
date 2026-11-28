# The `cabuya` program

`cabuya` pairs two machines by PIN, serves a link, and carries a named stream or a TCP port over it. It is the crate built with the `cli` feature:

```sh
cargo install --git https://github.com/Boriken-Dev/cabuya --features cli
```

## The folder

Every command works in one per-user folder: `--home DIR`, else `CABUYA_HOME`, else `%APPDATA%\cabuya` on Windows and `$XDG_CONFIG_HOME/cabuya` or `~/.config/cabuya` elsewhere. It holds:

| File | What |
| --- | --- |
| `identity` | this user's key. Made by the first command that needs it, by `--method noise` (the default) or `webcrypto`; it keeps its method |
| `served/` | the keys of the peers that may prove themselves to `serve`, added by `pair` |
| `dialed/` | the keys of the hosts this user dials, added by `pair --join` |
| `grants.json` | what each peer may open (`--grants FILE` names another); `serve` reads it on every question and re-checks running streams each second |
| `hosts.json` | the names of the hosts, with the peer id and the address of the `serve` that answers |

A proof admits a peer and grants nothing: a paired peer opens only what the grants file gives it.

## Pairing and serving

On the machine that will be reached:

```sh
cabuya serve --listen 0.0.0.0:7711 --pair --grant tcp/1:127.0.0.1:22
```

It prints `listening ADDRESS`, `peer ID`, and `pin 123456`. On the other machine:

```sh
cabuya pair --join HOST:7711 --name box --pin 123456
```

The first prints `paired ID` for the joiner, and the second `paired box ID`. The PIN is for one attempt: a wrong one burns it, and it lasts ten minutes. Without `--pin` the joiner reads one line from standard input. `--grant` grants the paired peer a protocol or `tcp/1:RULE` at once; `cabuya grant PEER TOKEN...` does it later, and `cabuya revoke PEER [TOKEN...]` takes grants away, or with none the peer's record and key, so it cannot connect again. A running stream of a revoked peer ends within a second, saying why.

`cabuya pair` alone is the one-shot form: it listens, shows a PIN, pairs one joiner, and exits. Its `--join` side takes `--at ADDRESS` when `serve` listens somewhere else than where the pairing happened.

A `tcp/1:RULE` grant names a target (`127.0.0.1:22`) or a pattern: a name, `*.suffix`, an address or network, and a port or range.

## `connect`

```sh
cabuya connect box 127.0.0.1:22
```

opens a `tcp/1` stream to the target from the host's side and carries it on standard input and output. It ends 0 when the stream ends in order, and 2 with a reason on standard error when the host refused it, took the permission away, or the link was lost. `--stream PROTOCOL [--metadata TEXT]` carries any other named stream the same way.

That makes it an `ssh` `ProxyCommand`, which works in stock VS Code's Remote - SSH as well as in any client:

```text
Host box
    ProxyCommand cabuya connect box 127.0.0.1:22
```

## `forward`

```sh
cabuya forward box --local 8022=127.0.0.1:22 --local 0=db.lan:5432
```

listens on each local port (loopback unless an address is given, `0` picks one and prints it as `listening ADDRESS TARGET`) and opens a `tcp/1` stream per connection, until the link ends. It warns when an address is not loopback, since whoever connects there gets what this key was granted. It does not reconnect: a caller that wants that runs it again.

## The other commands

`cabuya id` prints this user's peer id and method. `cabuya hosts` lists the paired hosts, and `cabuya forget NAME` removes one.
