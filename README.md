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
`pub/hosted-hashes/`) and the `guatiao` repository's files at the pinned revision
(`pub/git/guatiao-<revision>/`), at the versions `clients/outer-office/pubspec.lock` names on `main`
at the commit below. There is no `pub/git/cache/`: an offline `pub get` takes a git package from
its checkout and needs no clone, once the checkout has a `.git/pub-packages` file (empty is enough;
pub fills it in). Git cannot carry a `.git` path, so whoever copies `pub/` adds it, as below,
`tools/sdk/bootstrap` and `containers/` do.
The Dart packages' tests are left out, and so is any `extension/*/build` directory of a hosted
package (the DevTools extension's prebuilt web app, wasm included); the rest of the checkout stays, because the packages' build
hooks compile guatiao's crates from it. The Flutter SDK's own packages (`flutter`,
`flutter_test`) come with the SDK and are not here.

Built from `main` with guatiao 0350bae7 for the Dart packages, with Flutter `main` (Dart 3.15 dev).

## Use it

```sh
git clone --branch deps --depth 1 --single-branch https://github.com/jose-pr/ai6 ai6-deps
for c in ai6-deps/pub/git/*/; do mkdir -p "$c.git" && : > "$c.git/pub-packages"; done
cd ai6/clients/outer-office
PUB_CACHE=/path/to/ai6-deps/pub flutter pub get --offline
```

PowerShell:

```powershell
Get-ChildItem ai6-deps\pub\git -Directory | ForEach-Object {
  New-Item -ItemType File -Force (Join-Path $_.FullName '.git\pub-packages') | Out-Null }
$env:PUB_CACHE = 'C:\path\to\ai6-deps\pub'; flutter pub get --offline
```

Pub writes into the cache it uses (a checkout of `guatiao` under `pub/git/`), so point
`PUB_CACHE` at a copy, or `git clean -fdx` the branch afterwards.

## Refresh it

When `clients/outer-office/pubspec.lock` changes on `main`:

```sh
rm -rf /tmp/ai6-pub
cd ai6/clients/outer-office && PUB_CACHE=/tmp/ai6-pub flutter pub get
rm -rf /tmp/ai6-pub/_temp /tmp/ai6-pub/active_roots /tmp/ai6-pub/git/cache
for c in /tmp/ai6-pub/git/guatiao-*; do rm -rf "$c/.git" "$c"/bindings/dart/*/test; done
```

Then replace `pub/` on this branch with `/tmp/ai6-pub`, leaving out each hosted package's `test`,
`example` and `benchmark` directories, update the commit named above, and commit. No pack file
or other git object belongs on this branch. Point `app deps` in `tools/sdk/sdk.lock` on `main` and `DEPS_REF` in `containers/` at that
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

A cargo vendor directory: every package `Cargo.lock` on `main` names, for every target, and nothing
else. Each is a directory `crates/<name>-<version>/` of unpacked source with a
`.cargo-checksum.json`, the way `cargo vendor` writes it, and the branch holds no archive:

- **From crates.io**, 511 packages: the `.crate` file's contents. `package` in the checksum file is
  the checksum `Cargo.lock` gives, which `crates.py` checked on the `.crate` before it unpacked it.
- **From git**, 31 packages of 12 repositories, each at the revision `Cargo.lock` pins and with no
  history: the package's own directory, with the settings it inherits from its workspace written
  into its `Cargo.toml` (whose first line names the repository and revision). `package` is `null`,
  since the lock has no checksum for these. `guatiao` is MPL-2.0, as are `coqui-access` and
  `anycfg-rs`.

`files` in each checksum file lists every file kept with its SHA-256, and cargo checks each file it
reads against it. About 365 MiB unpacked.

Only what compiling needs is kept. Left out of every package are its `tests/`, `examples/`,
`benches/`, `fuzz/`, fixture and test-data directories, its dot directories and `.git*` files, and
every file an archive scanner would refuse (archives, compressed data, executables, certificates and
keys, pack files, wasm, object files and any other binary data). A file that a kept source file reads with `include_str!`,
`include_bytes!` or `include!` stays, so a crate that documents itself with an example still
compiles. Two things change, for the same reason:

- an `include_bytes!` of a file that had to go (the DER files of `ring`, `rustls-pki-types` and
  `rustls-webpki`) holds the file's bytes as an array literal instead;
- a PEM private key in a test or a document has one dash fewer in its `-----BEGIN`/`-----END` line.

A `[[test]]`, `[[bench]]` or `[[example]]` table whose file is gone is removed from the manifest.

The branch holds text and images only, so a binary a crate has is turned into text or dropped:

- an `include_bytes!` of a binary data table is replaced by the table as an array literal
  (`bstr`'s little-endian DFAs, `jiff-tzdb`'s zoneinfo, 5 MB of text in all), and its big-endian
  copy is dropped, so `bstr` does not build for a big-endian target from here;
- `ring`'s prebuilt nasm objects for x86 Windows are kept as `pregenerated/*.o.hex`, and its
  `build.rs` writes each back to a `.o` in `OUT_DIR` before linking;
- test vectors, fuzz corpora and the like that only tests read are dropped, as are
  `wit-bindgen`'s prebuilt objects (its build links `libwit_bindgen_cabi.a`, an archive, for the
  `wasm32-wasi*` targets only; no AI6 target builds it) and the `winapi-*-gnu-libs` import
  libraries (`.a` archives), so nothing links for a `*-windows-gnu` target from here.

### Use it

From a checkout of `main` at a commit whose `tools/sdk/sdk.lock` names this branch's commit, and a
checkout of that commit at `/path/to/ai6-deps`, with Python 3.11.4 or later:

```sh
python3 tools/sdk/crates.py use /path/to/ai6-deps
```

It checks every package, writes a cargo config in a new directory outside the repository
(`--out DIR` names it) that replaces crates.io and each git source by `crates/`, and prints how to
use it: set `CARGO_HOME` to the directory it names, or pass `cargo --config FILE`. Nothing is
unpacked. `cargo build --locked` then needs no network and no cargo cache. `--repo-config` also
writes the checkout's own `.cargo/config.toml`, which git ignores; delete it to build from the
network again.

### Refresh it

When `Cargo.lock` changes on `main`, from a checkout of `main` and a worktree on this branch:

```sh
python3 tools/sdk/crates.py refresh /path/to/ai6-deps
```

It makes `crates/` match the lock, fetching from crates.io and the pinned git commits (each `.crate`
is checked against the lock before it is unpacked) and removing what the lock dropped; commits that
here; points `DEPS_REF` in `containers/` at that commit; and points `app deps` in
`tools/sdk/sdk.lock` at it, in a commit on `main`'s side. Push this branch first, without rebasing
it: the lock names its commit. `sync` does only the first of those, and `check` only reports.
`tools/sdk/check_lock.py` fails when the commit the lock names does not cover `Cargo.lock`, and when
it holds any archive, compressed blob, executable, key or pack file, or a path below `.git`.
