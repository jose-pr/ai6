![Outer Office](../../docs/assets/outer-office-header.png)

# Outer Office

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The AI6 operator app for desktop and web (short name OO), where operators deal with
Moneypenny, the AI assistant. **Outer Office talks only to Universal Exports,
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
  each against its limit (other Operations' counted, not named), quota used per window, tokens today and until when a limited
  account waits, refreshed every fifteen seconds; it needs `pool.read`.
- Agents (`agent.read`): each Agent of an Operation with its record there
  (Missions done, failed, stalled and the incidents charged to it), the model occupying the
  codename, the clearance it works at, a coming retirement and a promotion waiting for green
  ink; with `agent.configure`, lower its clearance there or restore the configured one.
  Its Roles tab holds the Operation-scoped Agent roles (purpose and policy).
- Stations, with tabs for every Station's Theaters, each Station's settings (its Mission
  capacity and resource limits: the free memory and CPU past which it takes no new
  Missions; 0 MiB and 100% turn them off) and a placement preview.
- Overview, the page Outer Office opens on: the theme's hero art beside the person's
  Operations (each with the share of its Missions done) and the Stations; counts of
  Operations, Stations online and dark, active Missions and waiting Sessions; recent
  activity from the Operations' timelines (with `audit.read`); Upcoming, the Missions
  waiting to run (queued Missions oldest first, then held briefs); a Moneypenny card whose
  quick questions and field open her chat and ask her; and what needs attention (blocked
  Sessions, briefs vetting holds in the Operations where the person has `mission.create`,
  dark Stations), each with a link to its page. Operations: each Operation's Missions by
  state; Sessions: every Session in a table sorted by any column (a list on a phone).
- The top bar: the OO wordmark, search across the Operations, Missions, Sessions and
  Stations on the board, **Ask Moneypenny**, a bell counting what needs attention (its
  menu opens each item's page), appearance settings, and the person's chip (initials, name
  and first directory group) opening the operator menu.
- Roster: the Operations, Agents, Agent roles, cover policies and Moneypenny's settings every
  Station reads, with the revision each Station runs and what any Station refused or waits to
  restart for (`roster.read`); with `roster.manage` and a roster that takes edits (SQL), add,
  edit (the record's definition as JSON) or remove a record. The roster checks the whole
  roster first: a refused edit shows why and changes nothing; an accepted one reaches every
  Station at its next poll. Moneypenny's settings are one record, keyed `assistant`; after an
  edit, each setting it leaves looser than its default is shown.
- Green ink: the irreversible steps Agents wait to take (scope, Agent, Mission, reason,
  expiry), each approved for a chosen time with an optional note or rejected with a reason
  (`approval.decide`), and the Operation's class authorisations with their uses, revoking
  the live ones (`approval.class`). A Session held for green ink shows `BLOCKED · GREEN INK`.
- Moneypenny, the assistant: **Ask Moneypenny** on every page opens a chat with her (see
  Layout for where it goes and its shortcuts). She
  answers from AI6's records, through tools that run with the signed-in operator's own
  permissions, shows tables, charts, cards, timelines and diffs rendered by the app, and
  proposes changes the operator confirms or cancels (the exact request is shown); **Stop**
  ends an answer she is still giving. Her menu opens **Her memory of you**, the files of
  `memory/` in the operator's personal repository: open, edit and save, add, or forget one
  (after a confirmation), each a commit by the operator. See the
  [assistant guide](https://jose-pr.github.io/ai6/assistant/).
- Audit log for operators with `audit.read`; operator menu with identity, effective
  permissions per Operation, theme and wallpaper, and sign-out.
- Six themes from the Outer Office design kit (Night Ops, the default, Black Ops, Dusk,
  Classic, Light and Red File) or the system's light or dark setting, stored per device as
  `oo.theme`. Every text colour reaches WCAG AA (4.5:1) on its surface, the system's high
  contrast and reduced motion settings apply, and a theme's wallpaper (`oo.backdrop`) shows
  only behind sign-in, the Overview hero and empty pages. The hero is decorative: a world
  map with glowing routes (in Night Ops as drawn, in Black Ops, Classic, Light and Red File
  recoloured from the theme's own colours) or, in Dusk, a Westminster skyline; nothing is
  plotted on it, and its text sits on a solid panel. Large icons (stat tiles, recent
  activity, the phone menu) are duotone drawings of the kit's icons in the theme's accent.

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
& dart run tool/cdn_files.dart fill
```

Every web build, a release one included, runs `tool/cdn_files.dart fill` after
`flutter build web`: it puts the fallback fonts into `build/web` (below).

The Windows runner uses the Skia renderer until Impeller with native windowing is verified
across Windows GPUs.

## Quick start

A demo board with sample data and no network connection. Moneypenny answers there from a
short scripted sample, labelled "Sample conversation — demo": canned replies to her quick
questions (one with a table and a chart, one with a proposal whose Confirm and Cancel change
nothing) and one reply to anything else. A signed-in app never uses the script.

```powershell
& $flutter run -d web-server --web-hostname 127.0.0.1 --web-port 7357 --release --dart-define=OO_DEMO=true
& $flutter run -d windows --dart-define=OO_DEMO=true
```

Open `http://127.0.0.1:7357` for the web build.

## Connecting to Universal Exports

| Build define | Effect | Default |
| --- | --- | --- |
| `OO_API` | Universal Exports origin for desktop builds | `http://127.0.0.1:7082` |
| `OO_DEMO` | `true` shows sample data and makes no connection | `false` |

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

## No outside requests by default

Outer Office ships everything it shows: the Roboto font (Apache-2.0) for text and the
Cinzel font (SIL Open Font License 1.1, version 2.000) for the wordmark and section labels,
both in `assets/fonts/` and listed on the app's licence page, Material icons, the design
kit's icons and their duotone drawings, the wallpapers and the hero art.
The web build loads its engine (CanvasKit or skwasm) from its own `canvaskit/` directory
and the fonts for characters Roboto lacks from its own `assets/fonts/fallback/`, so by
default the browser fetches nothing from any origin but the one serving the app. Desktop
builds never use a CDN: the operating system's fonts cover other scripts and emoji there.

The fallback fonts are the 727 Noto files the Flutter web engine can ask for (SIL Open
Font License 1.1, `assets/fonts/LICENSE-Noto.txt`, on the licence page of the web app):
34.4 MiB in all, of which Chinese (simplified, traditional and Hong Kong), Japanese and
Korean are 25.3 MiB, Noto Color Emoji 2.0 MiB and every other script 7.1 MiB (Noto Sans
for Latin, Greek and Cyrillic 0.2 MiB, Arabic 64 KiB, Hebrew 21 KiB, Devanagari 63 KiB,
Thai 21 KiB). The engine fetches a file only when the text needs one of its characters;
CJK comes in slices of about 60 KiB, emoji in ten of about 200 KiB.

They are not in this repository. `tool/cdn_files.json` lists every file the web app would
otherwise take from a CDN, with its host, path, size, SHA-256 and licence, and
`tool/cdn_files.dart` fetches them:

- `dart run tool/cdn_files.dart fill [--deps DEPS] [--web build/web]` puts the fonts into
  the build and checks the build's engine files against the list. With `--deps`, a
  checkout of this repository's `deps` branch, it reads `DEPS/cdn/` and needs no
  network; otherwise it downloads from `fonts.gstatic.com`. A file whose SHA-256 differs
  from the list stops it.
- `dart run tool/cdn_files.dart mirror --out DIR` writes every listed file to
  `DIR/<host>/<path>`, which is how `deps` keeps its `cdn/`.
- `dart run tool/cdn_files.dart update --sdk FLUTTER_SDK` rebuilds the list for a new
  Flutter engine, after `flutter build web`, from the engine's own fallback font table and
  the build's `canvaskit/` files.

A web deployment may opt in with `[assets] cdn` in an `OO.cfg` beside `index.html`, read
by `web/flutter_bootstrap.js` before the engine starts:

| `cdn` | Fetched from outside |
| --- | --- |
| `"none"` (default; also a missing, unreadable or unknown value) | nothing |
| `"fonts"` | the fallback fonts, from Google Fonts (`fonts.gstatic.com`) instead of the build |
| `"all"` | those fonts, and the engine from Flutter's CDN (`www.gstatic.com`) |

```toml
[assets]
cdn = "fonts"
```

`test/no_cdn_test.dart` fails if the page or the bootstrap names a CDN host, if the
default stops loading the engine and fallback fonts from the app's origin, or, given a
`build/web`, if a listed font is missing from it. Moneypenny's linked pages are separate
pages and may load what they need.

## Layout

A top bar, a rail of pages (Overview, Operations, Missions, Sessions, Agents, Stations,
Green Ink, Briefings, Roster, Account Pools, Timeline, Audit; each shown only with its
permission) and the page, Overview after sign-in. From 1200 px wide the rail has labels
beside its icons, from 900 px labels below them, and narrower it opens from a menu button,
one view at a time, its pages as rows with icon tiles. Below 600 px (a phone) the top bar
keeps the page title, search, Moneypenny, the bell and the person's initials; Overview is
one column, tables are lists, and Moneypenny's chat fills the screen.

Moneypenny's chat is hidden behind the top bar's Moneypenny button until opened; a dot on
the button says she answered or proposed something meanwhile. Docked, it sits beside the
page from 1200 px wide (drag the handle, or focus it and use the arrow keys, to make it 320
to 720 px wide; `OO.cfg`'s `[chat]` changes these and its other sizes, see
[Configuration](https://jose-pr.github.io/ai6/configuration/)), is a modal sheet over the
page from 900 px, and fills the screen below
that. Expanded, it takes the centre, the rail stays, and the page keeps its scroll and
filters for **Back to …**; below 900 px it is the same as docked. On desktop **Pop out**
moves her into her own window, "Moneypenny | Outer Office"; closing that window brings her
back. Every layout shows the same conversation.

| Keys (macOS) | Effect |
| --- | --- |
| Ctrl+Alt+I (⌃⌘I) | Open the chat where it was last open and focus the composer; focus it if open; hide it if it has focus |
| Ctrl+Alt+Shift+I (⌃⌘⇧I) | Expanded or docked |
| Ctrl+Alt+O (⌃⌘O) | Desktop: pop out, or bring back |
| Esc | In the composer: back to the page; expanded: docked; sheet or full screen: hide |
| Enter, Shift+Enter | Send; a new line |

A key that types a character in a text field is left to it, so AltGr layouts (where Windows
reports AltGr as Ctrl+Alt) keep typing. The layout and width are kept per person on each
device under `oo.chat.<person ID>`; a phone starts with the chat hidden.

Web is one Flutter app view: wide windows keep the board beside the selected Session,
narrow windows navigate to it in-page with browser back. It opens no browser popups and
no embedded Flutter views. Native desktop opens each Session, and Moneypenny when popped
out, in its own window sharing the board's state.

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
