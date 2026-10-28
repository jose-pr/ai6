# Contributing to AI6

## Licence

This project is distributed under the **Mozilla Public License 2.0**
(`LICENSE`). Its copyleft is per file, which means:

- **Running AI6 imposes nothing on your code.** The agents it supervises,
  your loadouts, your tracker integrations and anything else you write beside
  it are yours, under whatever licence you choose.
- **Changing AI6's own files does carry an obligation.** A modified file
  of this project must be published under the MPL 2.0 to the people you ship
  it to.
- A combined work may be distributed under terms of your choosing (MPL 2.0
  section 3.3), and MPL 2.0 remains compatible with the GPL.

## Contributor licence agreement

A first pull request needs a one-time signature. The bot will comment with
the exact sentence to reply with; read [`CLA.md`](CLA.md) first.

It exists so the project can be relicensed later without tracking down every
past contributor. You keep the copyright in everything you write.

## Before you open a pull request

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features

cd clients/outer-office
flutter analyze --fatal-infos
flutter test
```

All checks must pass. See [`AGENTS.md`](AGENTS.md) for the repository's layout
and the rules every change keeps:

- M owns policy, the Station owns provider Sessions, and Universal Exports owns user access;
- Mission IDs and provider Session IDs are distinct durable identities;
- a secret value never reaches a worktree, a log or a terminal stream;
- files are LF, on every platform.
