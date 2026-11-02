# AI6 build dependencies

This branch holds no AI6 source. `pub/` is a Dart pub cache with every package the Outer Office
Flutter app in `clients/outer-office` needs, so the app can be built for Linux, Windows and the web
without reaching pub.dev or GitHub. `cdn/` holds every file the Outer Office web app would
otherwise fetch from a CDN (below). `crates/` holds every Rust crate the workspace needs, so it
builds without reaching crates.io or GitHub (below). `containers/` holds Dockerfiles for an image
that builds the app, on any base and any Flutter (its README).

`tools/sdk/sdk.lock` on `main` names the commit of this branch a build uses, so a new commit here
reaches nobody until the lock moves to it.

It holds the packages from `pub.dev` (`pub/hosted/`, with their hashes in
`pub/hosted-hashes/`) and a bare clone of the `guatiao` Dart binding's repository
(`pub/git/cache/`), at the versions `clients/outer-office/pubspec.lock` names on `main` at the
commit below. Pub checks the files out from it on first use. The Flutter SDK's own packages
(`flutter`, `flutter_test`) come with the SDK and are not here.

Built from `main` at `5cc1796e` with Flutter `main` (Dart 3.15 dev).

## Use it

```sh
git clone --branch deps --depth 1 --single-branch https://github.com/jose-pr/ai6 ai6-deps
cd ai6/clients/outer-office
PUB_CACHE=/path/to/ai6-deps/pub flutter pub get --offline
```

PowerShell: `$env:PUB_CACHE = 'C:\path\to\ai6-deps\pub'; flutter pub get --offline`.

Pub writes into the cache it uses (a checkout of `guatiao` under `pub/git/`), so point
`PUB_CACHE` at a copy, or `git clean -fdx` the branch afterwards.

## Refresh it

When `clients/outer-office/pubspec.lock` changes on `main`:

```sh
rm -rf /tmp/ai6-pub
cd ai6/clients/outer-office && PUB_CACHE=/tmp/ai6-pub flutter pub get
rm -rf /tmp/ai6-pub/_temp /tmp/ai6-pub/active_roots /tmp/ai6-pub/git/guatiao-*
```

Then replace `pub/` on this branch with `/tmp/ai6-pub`, update the commit named above, and
commit. Point `app deps` in `tools/sdk/sdk.lock` on `main` and `DEPS_REF` in `containers/` at that
commit.

The packages keep their own licences, which are in each package's directory.

## `cdn/`

Every file in `clients/outer-office/tool/cdn_files.json` on `main`, at
`cdn/<host>/<path>`, byte for byte as the host serves it:

- `cdn/fonts.gstatic.com/s/`: the 727 Noto fallback fonts the Flutter web engine asks for
  when text needs characters Roboto lacks (34.4 MiB; SIL Open Font License 1.1). The web
  build serves them itself from `assets/fonts/fallback/`.
- `cdn/www.gstatic.com/flutter-canvaskit/<engine revision>/`: the engine files Flutter's
  CDN serves for that revision (CanvasKit, skwasm and their variants, 33.0 MiB;
  BSD-3-Clause). A build already holds them in `canvaskit/`; this copy is what a
  deployment with `[assets] cdn = "all"` would fetch.

These are all the outside requests of an Outer Office web build with `cdn = "all"`,
measured by loading it in a browser with a network log, plus the rest of the engine's
fallback font table. The app's own Roboto and icons are in the app, not here.

Built for Flutter engine `2541e508405f5d9728286d6ed3307163a5406e08`.

### Use it

```sh
cd ai6/clients/outer-office
flutter build web --release
dart run tool/cdn_files.dart fill --deps /path/to/ai6-deps
```

`fill` reads `cdn/` and needs no network; each file is checked against its SHA-256 in
`tool/cdn_files.json`.

### Refresh it

For a new Flutter engine revision or font set, on `main`:

```sh
cd ai6/clients/outer-office
flutter build web --release
dart run tool/cdn_files.dart update --sdk /path/to/flutter
```

`update` reads the engine's fallback font table from the SDK checkout and the engine files
from `build/web/canvaskit/`, downloads each once and rewrites `tool/cdn_files.json`;
commit that on `main`. Then refill this branch:

```sh
rm -rf /path/to/ai6-deps/cdn
dart run tool/cdn_files.dart mirror --out /path/to/ai6-deps/cdn
```

Update the engine revision named above and commit `cdn/` here.

## `crates/`

Every package `Cargo.lock` on `main` names, for every target, and nothing else:

- `crates/<name>-<version>.crate`: the file crates.io serves, 488 of them, 67.5 MiB. Its SHA-256 is
  the lock's checksum. Each keeps its own licence, inside it.
- `crates/git/<repository>-<revision>.tar.gz`: a source archive of each pinned commit, with no
  history, 5.05 MiB: `guatiao` 4.78, `coqui-access` 0.20, `anycfg-rs` 0.07, each MPL-2.0. The lock
  has no checksum for these, so `crates/git/SHA256SUMS` holds what they were when `git` fetched
  that exact commit.

That is 72.6 MiB of this branch's 314.5 MiB, and 72 MiB of a plain clone of the repository, which
fetches this branch too (185 MiB in all; `main` alone is 10 MiB). Git cannot compress these files
further, and each new version of a crate stays in the history. The largest is 8.9 MiB. Unpacked for
a build, they are about 600 MiB.

### Use it

From a checkout of `main` at a commit whose `tools/sdk/sdk.lock` names this branch's commit, and a
checkout of that commit at `/path/to/ai6-deps`, with Python 3.11.4 or later:

```sh
python3 tools/sdk/crates.py use /path/to/ai6-deps
```

It checks each file, unpacks them as cargo directory sources in a new directory outside the
repository (`--out DIR` names it), writes a cargo config there and prints how to use it: set
`CARGO_HOME` to the directory it names, or pass `cargo --config FILE`. `cargo build --locked` then
needs no network and no cargo cache. `--repo-config` also writes the checkout's own
`.cargo/config.toml`, which git ignores; delete it to build from the network again.

### Refresh it

When `Cargo.lock` changes on `main`, from a checkout of `main` and a worktree on this branch:

```sh
python3 tools/sdk/crates.py refresh /path/to/ai6-deps
```

It makes `crates/` match the lock, fetching from crates.io and the pinned git commits and removing
what the lock dropped; commits that here; points `DEPS_REF` in `containers/` at that commit; and
points `app deps` in `tools/sdk/sdk.lock` at it, in a commit on `main`'s side. Push this branch
first, without rebasing it: the lock names its commit. `sync` does only the first of those, and
`check` only reports. `tools/sdk/check_lock.py` fails when the commit the lock names does not cover
`Cargo.lock`.
