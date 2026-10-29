# AI6 build dependencies

This branch holds no AI6 source. `pub/` is a Dart pub cache with every package the Outer Office
Flutter app in `clients/outer-office` needs, so the app can be built for Windows, Linux, macOS and the
web without reaching pub.dev or GitHub. `cdn/` holds every file the Outer Office web app would
otherwise fetch from a CDN (below).

It holds the packages from `pub.dev` (`pub/hosted/`, with their hashes in
`pub/hosted-hashes/`) and a bare clone of the `guatiao` Dart binding's repository
(`pub/git/cache/`), at the versions `clients/outer-office/pubspec.lock` names on `main` at the
commit below. Pub checks the files out from it on first use. The Flutter SDK's own packages
(`flutter`, `flutter_test`) come with the SDK and are not here.

Built from `main` at `0df0390a` with Flutter `main` (Dart 3.15 dev).

## Use it

```sh
git clone --branch deps --depth 1 https://github.com/jose-pr/ai6 ai6-deps
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
