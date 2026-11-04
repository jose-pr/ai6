# Every SDK a build needs

`sdk.lock` says what a build of AI6 needs, at which exact version, and how to verify it.
`bootstrap.sh` and `bootstrap.ps1` install it under one directory, so a clean machine reaches a
working build with one command and no machine-local path.

## On a clean machine

`git` is the one thing to have first, and `curl` on Linux and macOS. A plain clone also downloads
the `deps` branch (about 175 MiB), so clone `main` alone:

```sh
git clone --branch main --single-branch https://github.com/jose-pr/ai6
cd ai6
sh tools/sdk/bootstrap.sh            # or: pwsh -File tools/sdk/bootstrap.ps1
. ~/.ai6-sdk/env.sh                  # or: . ~/.ai6-sdk/env.ps1
```

Then the commands in [`AGENTS.md`](../../AGENTS.md) and
[`clients/outer-office/README.md`](../../clients/outer-office/README.md) work as written.

Everything goes under the prefix, by version. Nothing changes the shell profile, `PATH` or the
system, and nothing elevates; remove the prefix and it is gone, apart from the settings Flutter
and Dart keep in the home directory on their first run. `env.sh` and `env.ps1` set `RUSTUP_HOME`,
`CARGO_HOME`, `FLUTTER_ROOT`, `PUB_CACHE`, `FLUTTER_WINDOWING` and `PATH`, for the shell that
sources them. `FLUTTER_WINDOWING` does what `flutter config --enable-windowing` would, without
writing to the settings every other Flutter on the machine shares.

## Flags

The same on both scripts:

| Flag | Effect | Default |
| --- | --- | --- |
| `--groups a,b` | Which groups of `sdk.lock` to install | `core,app` |
| `--prefix DIR` | Where everything goes | `AI6_SDK_HOME`, or `~/.ai6-sdk` |
| `--check` | Report each item and what it would fetch, changing nothing; exit 1 if anything is missing | |
| `--print-system` | Print the commands that install the `system` group, for this machine | |

| Group | What |
| --- | --- |
| `core` | rustup, the locked stable and minimum Rust with clippy, rustfmt and the wasm target; on Windows ARM64, clang if none is found |
| `app` | Flutter at the locked commit and its engine for this host and the web, and the `deps` branch at the locked commit: its `pub/` (copied, since pub writes to its cache), `cdn/` and `crates/`. `AI6_SDK_DEPS` names that checkout |
| `api` | The nightly and `cargo-public-api` of the public API snapshots; the install compiles it |
| `docs` | A virtual environment with the documentation tooling, from a `python3` already here |
| `test` | Node, for the live ACP adapter check. PostgreSQL's image is reported with its digest and left to your container engine |

Running it again does nothing when the versions match.

## What is trusted, and where it comes from

| Thing | Where it comes from | Verified by |
| --- | --- | --- |
| `sdk.lock` | this repository | it is the root of trust |
| Pub cache, CDN files | the `deps` branch, at the commit the lock names | that commit; `pubspec.lock` hashes; `tool/cdn_files.json` |
| Flutter | its git repository | the commit in the lock; the version and engine hash it reports |
| `rustup-init` | static.rust-lang.org | SHA-256 in the lock |
| Rust toolchains | static.rust-lang.org, through rustup | rustup checks each component against its channel manifest |
| Crates | crates.io and pinned git revisions, or the `deps` branch's `crates/` (below) | `Cargo.lock`'s checksums and revisions; `crates/git/SHA256SUMS` for an archive of a revision |
| LLVM on Windows ARM64, Node | upstream | SHA-256 in the lock |
| MSVC, Windows SDK, Xcode, Linux dev packages | the vendor or distribution | a presence check; `--print-system` prints the install command |

Flutter's engine downloads carry no checksum of their own: `flutter` fetches them from
storage.googleapis.com, or `FLUTTER_STORAGE_BASE_URL`, unverified.

A download goes to a new empty directory and is checked before anything unpacks it. A mismatch
stops with both hashes and keeps nothing, and so does a kept archive that no longer matches.

`AI6_SDK_DEPS_URL` and `AI6_SDK_FLUTTER_URL` point at a mirror of the two git repositories. The
commits still have to match. `AI6_SDK_CRATES_URL` does that for `crates.py`'s downloads, as a
template with `{name}` and `{version}`; each file still has to match `Cargo.lock`.

## Building with no network

The `deps` branch's `crates/` holds every crate `Cargo.lock` names for every target: each `.crate`
from crates.io, and a source archive of each pinned git commit. `crates.py`, which needs Python
3.11.4 or later, works on a checkout of that branch. Installing the SDKs needs the network once;
the build after this does not:

```sh
sh tools/sdk/bootstrap.sh --groups core,app; . ~/.ai6-sdk/env.sh
python3 tools/sdk/crates.py use "$AI6_SDK_DEPS"
# prints the CARGO_HOME to set, then:
cargo build --locked -p ai6 --features ai6/cli
```

| Command | Does |
| --- | --- |
| `crates.py use DEPS_DIR [--out DIR] [--repo-config]` | Checks `crates/`, unpacks it as cargo directory sources in a directory outside the repository, with a `[source]` entry for crates.io and each git source, and prints how to use the config it wrote. `--repo-config` also writes this checkout's `.cargo/config.toml` |
| `crates.py check DEPS_DIR` | Fails, naming what is missing, changed or stale |
| `crates.py sync DEPS_DIR` | Makes `crates/` match `Cargo.lock`: fetches what is missing, verifies it, removes what is gone |
| `crates.py refresh DEPS_DIR` | `sync`, then a commit there, `DEPS_REF` in its Dockerfiles, and `app deps` in `sdk.lock` in a commit here |

A package taken from a git archive keeps its files with its inherited workspace settings written
into its manifest, since a directory source reads each package alone.

## Changing a pin

Run `python3 tools/sdk/check_lock.py` afterwards (`--online` also compares each download's hash
with what upstream publishes; `--deps-ref` says where to read the build containers' defaults). It
also fails on a package `clients/outer-office/pubspec.lock` names that the `deps` commit's `pub/`
lacks, so a new dependency cannot land ahead of a refreshed branch.

- **Rust stable**: the newest tested on merge. The lock is for repeatable builds; CI keeps legs on
  floating stable, so a new compiler is still noticed.
- **Flutter**: one change. Refresh the `deps` branch for the new engine, then update the lock
  (`flutter`, `flutter-version`, `flutter-engine`, `dart`, `deps`), `tool/cdn_files.json` and the
  defaults of the `deps` branch's Dockerfiles together. `check_lock.py` wants the lock's engine
  among the list's `engine_revisions`; other engines it lists are allowed.
- **`pubspec.lock`**: refresh `pub/` on the `deps` branch (its README says how) and update `deps`.
- **`Cargo.lock`**: `python3 tools/sdk/crates.py refresh DEPS_DIR`, with `DEPS_DIR` a worktree on the
  `deps` branch, then push that branch first, without rebasing it. `check_lock.py` fails when the
  `deps` commit the lock names does not cover `Cargo.lock`.
- **Downloads**: copy the hash from upstream's own checksum file, never from your download.
- **Docs tooling**: `docs-requirements.txt` holds every package with every published file's hash;
  the lock repeats the three top-level versions.

The build containers, for when installing on the machine is not what you want, are on the `deps`
branch in `containers/`, with instructions.
