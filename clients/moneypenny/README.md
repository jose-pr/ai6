# Moneypenny

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The AI6 operator app for desktop and web. **Moneypenny talks only to Universal Exports,
AI6's authenticated public API; it never reaches a Station, a provider or M's
credentials.** It shows Station health and the Headquarters Station, the Operation →
Mission → Session board, Mission briefs, live Session terminals, administration and audit.

## Features

- Stations: health, roles, capacity and the elected Headquarters Station (HQS).
- Operations → Missions → Sessions, with state filters; queue a Mission on an Operation
  you hold `mission.create` for, and review Agent-proposed Missions.
- Mission brief view: the exact prompt and a sanitized launch manifest (environment
  variable names only, never values).
- Live terminals: read-only by default; **Request input lease** takes a write lease.
  Closing a view never stops the provider Session.
- Mission actions: each Mission card's menu cancels, retries (optionally for another
  Agent), resumes (optionally with a message) or interrupts, offering only what the
  Mission's current state allows and saying why the rest are unavailable; it needs
  `mission.control`, and the outcome the Station reports shows on the card.
- Timeline: an Operation's activity newest first (lifecycle, controls, Cables, Sleepers,
  audit), filtered by Operation and Mission, with older pages on request and new activity
  added on top every ten seconds; it needs `audit.read`.
- Account pools: the pool accounts each Operation may draw from, with the Sessions using
  each against its limit, quota used per window, tokens today and until when a limited
  account waits, refreshed every fifteen seconds; it needs `pool.read`.
- Administration: Operation-scoped Agent roles (purpose and policy) and Station capacity.
- Green ink: the irreversible steps Agents wait to take (scope, Agent, Mission, reason,
  expiry), each approved for a chosen time with an optional note or rejected with a reason
  (`approval.decide`), and the Operation's class authorisations with their uses, revoking
  the live ones (`approval.class`). A Session held for green ink shows `BLOCKED · GREEN INK`.
- Audit log for operators with `audit.read`; operator menu with identity, effective
  permissions per Operation, theme and sign-out.

## Toolchain

Native desktop Session windows use Flutter's first-party Desktop Windowing API, which is
experimental and only on Flutter `main`. Keep a separate `main` SDK checkout so the stable
SDK used elsewhere is unchanged, and use it for every command here:

```powershell
$flutter = '<Flutter main SDK>\bin\flutter.bat'
& $flutter config --enable-windowing
& $flutter pub get
& $flutter analyze --fatal-infos
& $flutter test
& $flutter build windows --debug
& dart run tool/build_guatiao_wasm.dart
& $flutter build web --release
```

The Windows runner uses the Skia renderer until Impeller with native windowing is verified
across Windows GPUs.

## Quick start

A demo board with sample data and no network connection:

```powershell
& $flutter run -d web-server --web-hostname 127.0.0.1 --web-port 7357 --release --dart-define=MONEYPENNY_DEMO=true
& $flutter run -d windows --dart-define=MONEYPENNY_DEMO=true
```

Open `http://127.0.0.1:7357` for the web build.

## Connecting to Universal Exports

| Build define | Effect | Default |
| --- | --- | --- |
| `MONEYPENNY_API` | Universal Exports origin for desktop builds | `http://127.0.0.1:7082` |
| `MONEYPENNY_DEMO` | `true` shows sample data and makes no connection | `false` |

Web uses its own origin: serve it from the same HTTPS origin as Universal Exports, behind
the organization's authenticated reverse proxy. The proxy supplies the signed-in identity
to `POST /v1/auth/login`, so a live web build opened straight from the Flutter dev server
cannot sign in. Desktop can sign in with the configured Authenticator. The bearer token is
kept in memory only, and signing out revokes it.

The board reads `/v1/stations`, `/v1/board` and `/v1/missions` every three seconds;
Universal Exports filters them by `mission.read` per Operation. A Session view exchanges a
Session-scoped grant for a one-use HttpOnly cookie, attaches over the WebSocket, decodes
DipTel frames with Guatiao's Dart receiver and renders them with `xterm2`, resuming from
its last event after a disconnect.

The web build loads Guatiao's WebAssembly wire runtime from `web/ai6_guatiao.wasm`, which
is generated and ignored: run `dart run tool/build_guatiao_wasm.dart` after `pub get` and
before a non-demo web build. It builds `guatiao-intake` at the revision in `pubspec.lock`.

## Layout

Web is one Flutter app view: wide windows keep the board beside the selected Session,
narrow windows navigate to it in-page with browser back. It opens no browser popups and
no embedded Flutter views. Native desktop opens each Session in its own window sharing
the board's state.

## Development

```powershell
dart format --output=none --set-exit-if-changed lib test
& $flutter analyze --fatal-infos
& $flutter test
```

`test/session_connection_test.dart` decodes the protocol vectors in
`crates/ai6-diptel/tests/vectors/`, so run tests from this directory inside the repository.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
