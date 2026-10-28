# AI6 build dependencies

This branch holds no AI6 source. `pub/` is a Dart pub cache with every package the Flutter
app in `clients/moneypenny` needs, so the app can be built for Windows, Linux, macOS and the
web without reaching pub.dev or GitHub.

It holds the packages from `pub.dev` (`pub/hosted/`, with their hashes in
`pub/hosted-hashes/`) and a bare clone of the `guatiao` Dart binding's repository
(`pub/git/cache/`), at the versions `clients/moneypenny/pubspec.lock` names on `main` at the
commit below. Pub checks the files out from it on first use. The Flutter SDK's own packages
(`flutter`, `flutter_test`) come with the SDK and are not here.

Built from `main` at `a942252` with Flutter `main` (Dart 3.15 dev).

## Use it

```sh
git clone --branch deps --depth 1 https://github.com/jose-pr/ai6 ai6-deps
cd ai6/clients/moneypenny
PUB_CACHE=/path/to/ai6-deps/pub flutter pub get --offline
```

PowerShell: `$env:PUB_CACHE = 'C:\path\to\ai6-deps\pub'; flutter pub get --offline`.

Pub writes into the cache it uses (a checkout of `guatiao` under `pub/git/`), so point
`PUB_CACHE` at a copy, or `git clean -fdx` the branch afterwards.

## Refresh it

When `clients/moneypenny/pubspec.lock` changes on `main`:

```sh
rm -rf /tmp/ai6-pub
cd ai6/clients/moneypenny && PUB_CACHE=/tmp/ai6-pub flutter pub get
rm -rf /tmp/ai6-pub/_temp /tmp/ai6-pub/active_roots /tmp/ai6-pub/git/guatiao-*
```

Then replace `pub/` on this branch with `/tmp/ai6-pub`, update the commit named above, and
commit.

The packages keep their own licences, which are in each package's directory.
