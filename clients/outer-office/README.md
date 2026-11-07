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

- A name opens what it names: an Operation, Mission, Session, Station, Theater, Agent, role or
  pool anywhere on a page is a link through the typed routes (keyboard reachable, underlined under
  the pointer, announced as a link, a full tap target on a phone), and plain text where the person
  may not open that page. A count on Overview opens its page filtered to what it counted. Each
  page has one heading and a line on what it is for, and an empty one says what to do first.
- Stations: health, roles, capacity and the elected Headquarters Station (HQS).
- Operations → Missions → Sessions, with state filters; queue a Mission on an Operation
  you hold `mission.create` for, and review Agent-proposed Missions.
- The Missions page is the queue (with no Mission yet, **New Mission in …** starts the first):
  one row per Mission with its state as a word and icon
  (and why, when the Station says), its Operation and Mission, the Station and Theater of its
  current attempt, when the board last heard of it (relative, the exact local time and zone
  on hover) and the Agent and role its brief names, newest update first unless the route
  sorts, ties in Operation and Mission order so rows stay put between refreshes. Its state filter
  and counts go by a Mission's state, that of the Session updated last, as Overview and an
  Operation's counts do; the Sessions page filters each Session by its own. With the
  `queued` state a strip counts the briefs vetting holds and the Mission proposals waiting,
  each linking to Briefings; neither is ever a row. **Mission details** (route
  `missions?operation_id=…&mission_id=…`) opens a read-only detail beside the queue, or in
  place of it in a narrow window: the state and the Session that sets it, every attempt (its
  Station and Theater as links, why it was lost, its checkpoint branch, the Sessions on it
  to open), the brief and the Timeline, the actions the person has and, for a queued
  Mission, the Operation's placement now (what it rules out; hidden when the gateway refuses
  the preview). Closing it returns focus to the row.
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
- A Station (route `stations?station_id=…`) is a watch: Online or Dark, the last check-in,
  its roles, running Sessions of its capacity, resources as last sampled (**Not reported**
  when it sends none), and **Sessions here**, each Mission with an active attempt there as a
  link to its detail.
- An Operation (route `operations?operation_id=…`) opens a summary: what the person may do
  there, its Missions counted by state (each count links to the Missions in that state), the
  share done, the held briefs and Mission proposals waiting, and its newest activity with
  **View all** (`audit.read`).
- Stations, with tabs for every Station's Theaters, each Station's settings (its Mission
  capacity and resource limits: the free memory and CPU past which it takes no new
  Missions; 0 MiB and 100% turn them off) and a placement preview.
- Overview, the page Outer Office opens on: the theme's hero art beside the person's
  Operations (each with the share of its Missions done) and the Stations; counts of
  Operations, Stations online and dark, active Missions and waiting Sessions; recent
  activity from the Operations' timelines (with `audit.read`); Upcoming, the Missions
  waiting to run (queued Missions oldest first, then held briefs, each labelled); a
  Moneypenny card whose quick questions and field open her chat and ask her; and what needs
  attention (blocked Sessions, green ink requests waiting where the person has
  `approval.decide`, briefs vetting holds and Mission proposals waiting where the person has
  `mission.create`, dark Stations), each saying what, where and when and linking to its page
  or request. It says nothing needs the person only when every source was read; otherwise
  "Nothing found in what loaded" and the sources it could not read by name. With no visible
  Operation it says "No Operations you can see" and that a controller grants access, never
  whether any exist. Operations: each Operation's Missions by
  state; Sessions: every Session in a table sorted by any column (a list on a phone).
- The top bar: the OO wordmark, search across the Operations, Missions, Sessions and
  Stations on the board (Ctrl+K, or ⌘K on macOS, opens it unless the key would type a
  character into a text field; results say they are "in the loaded board", each opens what it
  names, and a miss says "No matches in the loaded board"), **Ask Moneypenny**, the bell **Needs attention** (a
  count of the items above, derived and never stored, with no unread state; its menu opens
  each item's page and, under **Your requests**, lists the controls you asked for that are
  not yet applied or refused, offering **Check again** on an outcome unknown), a **Demo
  data** chip in the demo, Settings, and the person's chip (initials, name and
  first directory group) opening the operator menu.
- Freshness: each source (Stations, Sessions, Missions, held briefs, green ink, Mission
  proposals, timeline) is read on its own. A failed read keeps that source's last data and
  the page says "Last updated 14:02:31"; while the gateway is unreachable a strip says so
  and when the data is from; a 403 hides a source (it is not shown empty); a 401 signs the
  person out and drops every view, item and request before the sign-in page shows. Every
  time is relative text with the exact local time and zone on hover and in its semantics
  label. A Station that has not checked in for three board intervals reads "No check-in for
  N s" beside its state word.
- A control (cancel, retry, resume, interrupt) is sending, accepted, then applied or
  refused; one still open after ten seconds is an outcome unknown. While a request is open
  the Mission's action menu is disabled ("A request is in progress"); **Check again** reads
  the same control and never posts a second time. Requests are not kept across a reload.
- Roster: the Operations, Agents, Agent roles, cover policies and Moneypenny's settings every
  Station reads, opening on **All records** (each kind's keys under its name, each a link to it in
  its kind) and narrowed to one kind by its picker, with the revision each Station runs and what any Station refused or waits to
  restart for (`roster.read`), each row its name (an Agent, Operation or role opens its page), a
  summary in the fields' own titles and **As
  configuration** for reading and copying what the files would hold; with `roster.manage` and a
  roster that takes edits (SQL), add, edit or remove a record in a form the gateway describes
  (below). The roster checks the whole roster first: a refused edit shows why on the field it
  names and changes nothing; an accepted one reaches every Station at its next poll. A save names
  the revision it read: if the record changed meanwhile, nothing is saved and **Reload** shows what
  it holds now. A removal says first what else names the record. Moneypenny's settings are one
  record; after an edit, each setting it leaves looser than its default is shown. **Add Operation**
  and **Add Agent**, and an edit and a remove on each one, are on those pages too, with which
  Stations run the change and any waiting for a restart.
- Access (after Audit): the roles (the built-in `chief` is fixed; role editor with a switch per
  action, greyed with the reason when you do not hold it or the delegation cap withholds it),
  the ordered rules (drag handles or move buttons; Save sends the whole list with the revision
  and a stale one asks to reload), the delegation cap with its weakenings, the chief list
  (host chiefs read-only), orphaned rules, the accounts (disable, enable, groups with
  `access.manage`, and a password reset whose new password shows once in a dialog and is kept
  nowhere) and a Check that explains an answer. It shows with `access.read` or
  `account.manage`; an Operation's own policy opens from that Operation's card (`access.read`
  there) and holds only actions within the cap.
- Green ink: the irreversible steps Agents wait to take (scope, Agent, Mission, reason,
  expiry), each approved for a chosen time with an optional note or rejected with a reason
  (`approval.decide`), and the Operation's class authorisations with their uses, revoking
  the live ones (`approval.class`). A Session held for green ink shows `BLOCKED · GREEN INK`.
  Reasons are rendered as Markdown, what they name links to the Operation's Sources, and a request's
  links follow them. The Expired tab lists what no one decided in time, and re-arms it. A Mission's
  page shows its Agent's final message and any verdict, and takes one with `mission.verify`.
  The Missions board counts `verifying` Missions, those waiting for a decision. Such a Mission's page
  shows each deliverable with the latest probe of it (found, missing, unprobed or failed), the final
  report, and, with `mission.verify`, Probe now and Done, Redispatch (as the next attempt, with an
  amendment) and Failed, each with a required reason.
- Agents, Account pools and Green ink open on **All Operations**: every Operation the person
  may read there, each one's rows under its name, an Operation that could not be read named with
  why. The picker narrows to one (`operation_id` in the route), and an action on a row goes to
  that row's Operation. **Add filter** narrows by a field the shown rows hold (Agent, Mission,
  pool), each a chip that removes it.
- Moneypenny, the assistant: **Ask Moneypenny** on every page opens a chat with her (see
  Layout for where it goes and its shortcuts), where the profile says the gateway serves her;
  elsewhere the app offers no card, button or chat for her and Settings says she is off and who
  can change that. She greets the person only once her socket says she is ready; a socket that
  fails, closes or stays silent past `[chat] connect_timeout` says so with **Try again**. She
  answers from AI6's records, through tools that run with the signed-in operator's own
  permissions, shows tables, charts, cards, timelines and diffs rendered by the app, offers
  pages as cards (the page and its filters as chips, and **Open**, which only reads and
  shows **Opened by Moneypenny** on the page), and
  proposes changes the operator confirms or cancels (the exact request is shown, with what
  she had read, the page it affects as a chip that only opens it, and how it stands:
  Waiting for you, Applying, Applied, Refused with the reason, Cancelled, Expired, or an
  outcome unknown when it has been applying for thirty seconds); **Stop**
  ends an answer she is still giving. **Copy** beside each of her answers copies it as she wrote
  it, Markdown included, and any text in the app can be selected and copied. Her menu's **Export
  conversation** saves the conversation as Markdown: a download in a browser, a new file in
  Downloads on a desktop. Her menu opens **Her memory of you**: the entries she keeps
  of the operator, searched, opened in a form, saved, added or forgotten (after a confirmation).
  The same page shows the global memory and each Operation's the operator may read, with who wrote
  each entry and what Agents proposed for it. See the
  [assistant guide](https://jose-pr.github.io/ai6/assistant/) and
  [memories](https://jose-pr.github.io/ai6/memories/).
- Pages she publishes (a dashboard of live data, a report or a file) appear in her chat as a
  card: kind, size, expiry or **Remembered**, the feeds it reads and whom it is shared with.
  **Open** launches it in a new browser tab (the system browser on a desktop), never inside
  the app; the menu opens it with the network off, remembers or forgets it, shares it (one
  `person:`, `group:`, `account:`, `compartment:` or `operation:` subject a line) and deletes
  it. Her menu's **Her pages** lists your pages, then those shared with you, which say whose
  they are and only open. Only you, and whoever you share a page with, can open it.
- Audit log for operators with `audit.read`, each action in words with its name on hover;
  operator menu with identity, effective
  permissions per Operation, appearance and sign-out.
- Sign-in: the wordmark over the theme's wallpaper, the account and password form, and the
  reason a try was refused; the login limit's wait counts down and holds **Continue**.
- Six themes from the Outer Office design kit (Night Ops, the default, Black Ops, Dusk,
  Classic, Light and Red File) or the system's light or dark setting, stored per device as
  `oo.theme`. Settings (the person's chip in the top bar) show each
  theme as a miniature of its page, panel, selected row and button, with its name, a line
  on its character and a check mark on the chosen one; a choice applies at once. Every
  colour text is drawn in reaches WCAG AA (4.5:1) where it is drawn, panels over art
  included; each theme's eight chart colours stay apart for colour-blind readers. The
  system's reduced motion stops theme and page transitions, and its high contrast drops
  every glow, decorative gradient and picture. **Wallpaper** (`oo.backdrop`) puts art
  behind sign-in, the Overview hero and empty pages only. The hero is decorative: a world
  map with glowing routes (Night Ops as drawn, Black Ops quieter in its greys, Red File in
  its crimson), a Westminster skyline (Dusk), or pale cartographic contours on paper
  (Light as drawn, Classic in its own paper and ink); nothing is plotted on it, and its
  text sits on a solid panel. Accent edges and glow mark only the selection and the
  primary action. Large icons (stat tiles, recent activity, the phone menu) are duotone
  drawings of the kit's icons in the theme's accent.

## Forms

`lib/forms/` draws a schema and a form from anywhere and gives back a value; it knows no kind of
record. The types in the `ai6` crate declare every field once (title, words, default, choices,
section, whether it is rarely changed), and `GET /v1/user/roster/{kind}/schema` serves them.
`guatiao_intake` judges the form: whether it fits, the layout, and which fields show. The record's
`format` names what a field refers to (`record:agents`, `reported:handlers`), and the page says what
to offer for it (`lib/roster/record_editor.dart`). `assets/record_forms.json` holds the same answers
for the preview's summaries and the tests; a test in the `ai6` crate fails when it is stale, and
`AI6_UPDATE_RECORD_FORMS=1 cargo test -p ai6 --all-features --lib records` writes it.

`[oo]` is declared the same way, on `OoSettings` in the `ai6` crate.
`lib/settings/oo_settings.g.dart` holds its schema and form and each default as a constant, since a
`const` constructor needs one; `lib/settings/oo_schema.dart` reads a file by that schema, and the
operator menu draws it as a form. `cargo run -p ai6 --features cli --example settings_reference`
writes the file, and a test in the `ai6` crate fails when it is stale. A new key is added on the
struct, not here; a test in `test/oo_settings_test.dart` fails until `OoConfig` holds it.

The forms need the `guatiao_intake` surface: `web/ai6_guatiao.wasm` on the web, built below, and on
desktop the library its build hook bundles.

## Toolchain

The app builds for Linux, Windows and the web; a checkout has no macOS runner, so on macOS it runs
`flutter test` and the web build.

Native desktop Session windows use Flutter's first-party Desktop Windowing API, which is
experimental and only on Flutter `main`, so the app builds with the Flutter commit
[`tools/sdk/sdk.lock`](../../tools/sdk/README.md) names. From the repository root:

```sh
sh tools/sdk/bootstrap.sh          # or: pwsh -File tools/sdk/bootstrap.ps1
. ~/.ai6-sdk/env.sh                # or: . ~/.ai6-sdk/env.ps1
```

That SDK lives under its own directory, so any other Flutter on the machine is unchanged, and the
file sets `FLUTTER_WINDOWING`, which turns the windowing API on for this shell only. Then, from
this directory:

```sh
flutter pub get --offline
flutter analyze --fatal-infos
flutter test
flutter build linux --debug        # or: flutter build windows --debug
dart run tool/build_guatiao_wasm.dart
flutter build web --release
dart run tool/cdn_files.dart fill --deps "$AI6_SDK_DEPS"   # PowerShell: "$env:AI6_SDK_DEPS"
```

Every web build, a release one included, runs `tool/cdn_files.dart fill` after
`flutter build web`: it puts the fallback fonts into `build/web` (below).

An image with all of this in it, on any base and any Flutter, is one `podman build` away:
`containers/README.md` on the
[`deps` branch](https://github.com/jose-pr/ai6/blob/deps/containers/README.md).

The Windows runner uses the Skia renderer until Impeller with native windowing is verified
across Windows GPUs.

## Quick start

A demo board with sample data and no network connection. Moneypenny answers there from a
short scripted sample, labelled "Sample conversation — demo": canned replies to her quick
questions (one with a table and a chart, one with a proposal whose Confirm and Cancel change
nothing) and one reply to anything else. A signed-in app never uses the script.

```sh
flutter run -d web-server --web-hostname 127.0.0.1 --web-port 7357 --release --dart-define=OO_DEMO=true
flutter run -d windows --dart-define=OO_DEMO=true
```

Open `http://127.0.0.1:7357` for the web build.

## Connecting to Universal Exports

| Build define | Effect | Default |
| --- | --- | --- |
| `OO_API` | Universal Exports origin for desktop builds | `http://127.0.0.1:7082` |
| `OO_DEMO` | `true` shows sample data and makes no connection | `false` |

Web uses its own origin: a reverse proxy serves the build and passes `/v1/` to Universal
Exports on one HTTPS origin, which is the gateway's `public_origin`, so a live web build opened
straight from the Flutter dev server cannot sign in. [Access and
sign-in](https://jose-pr.github.io/ai6/access/#signing-in-from-a-browser) lists what the proxy must
pass. The sign-in page asks `GET /v1/auth/methods` what the gateway accepts and shows its name and
password form, the button of an organization proxy that vouches for the person, or both, on web
and desktop alike. The bearer token is kept in memory only, and signing out revokes it. On the web
a sign-in also gets an HttpOnly session cookie that this code cannot read, nothing else is kept, and
a reload asks `POST /v1/auth/resume` for a new token and goes back to the page the URL names and the
last conversation. So someone with the browser open on that machine stays signed in up to `[gateway]
session_max` (twelve hours) after signing in, until they sign out; `[gateway] session_cookie = false`
turns that off and a reload signs in again. The desktop app has no cookie: closing it signs out. An
open app renews its token `[refresh] renew_before` ahead of its end, on its own
timer, so the person never sees the hour, and says once when the sign-in can be renewed no
further. Signing in again goes back to the last conversation, kept per person on the device
([Staying signed in](https://jose-pr.github.io/ai6/access/#staying-signed-in)).

The board reads what the page shown displays every three seconds (`[refresh] sessions`), such
as `/v1/missions` on Missions and nothing on Audit; what only the bell and the search need
every fifteen (`[refresh] slow`); and the person's profile every minute (`[refresh] profile`).
While the tab or window cannot be seen it reads everything once a minute (`[refresh] hidden`),
and it catches up as soon as it can be seen again or another page opens. Universal Exports
filters them by `mission.read` per Operation. A Session view exchanges a
Session-scoped grant for a one-use HttpOnly cookie, attaches over the WebSocket, decodes
DipTel frames with Guatiao's Dart receiver and renders them with `xterm2`, resuming from
its last event after a disconnect.

The web build loads Guatiao's WebAssembly wire runtime from `web/ai6_guatiao.wasm`, which
is generated and ignored: run `dart run tool/build_guatiao_wasm.dart` after `pub get` and
before a non-demo web build. It builds `guatiao-intake` at the revision in `pubspec.lock`, and the
record forms run in the same module.

## No outside requests by default

Outer Office ships everything it shows: the Roboto font (Apache-2.0) for text and the
Cinzel font (SIL Open Font License 1.1, version 2.000) for the wordmark and section labels,
both in `assets/fonts/` and listed on the app's licence page, Material icons, the design
kit's icons and their duotone drawings, the wallpapers and the hero art.
The web build loads its engine (CanvasKit or skwasm) from its own `canvaskit/` directory
and the fonts for characters Roboto lacks from its own `assets/fonts/fallback/`, so by
default the browser fetches nothing from any origin but the one serving the app. Desktop
builds never use a CDN: the operating system's fonts cover other scripts and emoji there.

The hero art in `assets/heroes/`, each file named by its width and chosen at run time by the
box's physical size (`OoArt`; `[oo.art] max_width` in `ai6.cfg` caps it): `map-940.jpg` and
`skyline-774.jpg` cut from the AI6 brand banners; `paper-1536.jpg`, the bottom 454 px of a
1536×1024 picture, and `paper-940.jpg`, its 940×278 crop at the left edge, 746 px down, made with a built-in image generation tool on 2026-10-29 from a written prompt and no
reference images (pale ivory paper, thin taupe coast-like contours and drafting arcs at the
left and right edges, a blank centre, no text, labels, routes or markers), mirrored so its
contours sit on the right, clear of the hero's text panel, and saved at JPEG quality 92.
The `-2560` and `-3840` files are enlarged from those pictures with Real-ESRGAN (BSD-3-Clause),
`realesrgan-ncnn-vulkan` from release v0.2.5.0, SHA-256 of the Windows zip
`abc02804e17982a3be33675e4d471e91ea374e65b70167abc09e31acb412802d`: the map and skyline from the
banners' lossless crops with the `realesrgan-x4plus-anime` model, the paper from `paper-1536.jpg`
with `realesrgan-x4plus`. Each is blended 70% with a bicubic enlargement to keep the art's tone,
then resized and saved at JPEG quality 92. The model invents the added detail, so it is decoration
and shows no real geography or building.
The wallpaper in `assets/backdrops/` is one drawing, `wallpaper.svg`, painted at the window's size
in each theme's colours: its graph and circles were measured from the AI6 brand pack's pictures,
which it replaces, and its label, `wallpaper-label.svg`, is outlined from Roboto, so no font is
involved. It draws crisp at any size, and the label is left out where it would not fit whole.

The fallback fonts are the 727 Noto files the Flutter web engine can ask for (SIL Open
Font License 1.1, `assets/fonts/LICENSE-Noto.txt`, on the licence page of the web app):
34.4 MiB in all, of which Chinese (simplified, traditional and Hong Kong), Japanese and
Korean are 25.3 MiB, Noto Color Emoji 2.0 MiB and every other script 7.1 MiB (Noto Sans
for Latin, Greek and Cyrillic 0.2 MiB, Arabic 64 KiB, Hebrew 21 KiB, Devanagari 63 KiB,
Thai 21 KiB). The engine fetches a file only when the text needs one of its characters;
CJK comes in slices of about 60 KiB, emoji in ten of about 200 KiB.

They are not in this repository. `tool/cdn_files.json` lists every file the web app would
otherwise take from a CDN, with its host, path, size, SHA-256 and licence, for each Flutter
engine in its `engine_revisions`: an engine's own files are those under
`/flutter-canvaskit/<revision>/`, and the fonts serve them all. It lists the engine
`tools/sdk/sdk.lock` names, and may list others, such as that of a vendored SDK.
`tool/cdn_files.dart` fetches them:

- `dart run tool/cdn_files.dart fill [--deps DEPS] [--web build/web]` puts the fonts into
  the build and checks the build's engine files against its engine's entry. A build of an
  engine the list does not name gets the fonts and a warning, with no engine check. With
  `--deps`, a checkout of this repository's `deps` branch, it reads `DEPS/cdn/` and needs
  no network; otherwise it downloads from `fonts.gstatic.com`. A file whose SHA-256 differs
  from the list stops it.
- `dart run tool/cdn_files.dart mirror --out DIR` writes every listed file to
  `DIR/<host>/<path>`, which is how `deps` keeps its `cdn/`.
- `dart run tool/cdn_files.dart update --sdk FLUTTER_SDK` rebuilds the list for one
  Flutter engine, after `flutter build web`, from the engine's own fallback font table and
  the build's `canvaskit/` files; `add` in its place keeps the other engines listed and
  downloads only what the list does not have yet.

A web deployment may opt in with `[oo.assets] cdn` in the public `ai6.cfg` beside `index.html`,
read by `web/flutter_bootstrap.js` before the engine starts:

| `cdn` | Fetched from outside |
| --- | --- |
| `"none"` (default; also a missing, unreadable or unknown value) | nothing |
| `"fonts"` | the fallback fonts, from Google Fonts (`fonts.gstatic.com`) instead of the build |
| `"all"` | those fonts, and the engine from Flutter's CDN (`www.gstatic.com`) |

```toml
[oo.assets]
cdn = "fonts"
```

That file is `ai6 config public --config /etc/ai6/ai6.cfg --output /srv/web/ai6.cfg`, written beside
the deployed build: it holds `[unex] url`, the host's `[unex] public_origin` when it has one, and
what the page needs of `[oo]`, from an allow list. A web
build never bundles a host's own `ai6.cfg`, and `test/oo_config_test.dart` fails if `web/` holds one.

`test/no_cdn_test.dart` fails if the page or the bootstrap names a CDN host, if the
default stops loading the engine and fallback fonts from the app's origin, or, given a
`build/web`, if a listed font is missing from it. Moneypenny's linked pages are separate
pages and may load what they need.

## Layout

A top bar, a rail of pages and the page, Overview after sign-in. The rail groups its pages by
what a person is asking: **Work** (Overview, Missions, Sessions, Green Ink, Briefings), **People
and policy** (Operations, Agents, Roster, Account Pools, Access), **Machines** (Stations) and
**Record** (Timeline, Audit). A page shows only with its permission, and a group with no page
left does not show. From 1200 px wide the rail has labels beside its icons; from 900 px it is
icons with each label on hover, the groups set apart by a line; narrower it opens from a menu
button, one view at a time, its pages as rows with icon tiles. Below 600 px (a phone) the first
pages sit in a bottom bar, **More** opens the same drawer, and the top bar keeps the page title,
search, Moneypenny, the bell and the person's initials; Overview is one column, tables are
lists, and Moneypenny's chat fills the screen.

How often each page asks again (`[oo.refresh]`), how a Mission control is followed
(`[oo.controls]`), how much the timeline and audit log ask for (`[oo.pages]`), the live terminal's
scrollback and reconnects (`[oo.terminal]`) and how much a text field takes (`[oo.limits]`) are
`ai6.cfg` tables too; every key is optional and the example shows the defaults. On the desktop the
app reads the same `ai6.cfg` as the other programs on the machine (`AI6_CONFIG`, else the user's
then the system's configuration directory), and takes `[unex] url`, `[unex] ca_file` and `[oo]`.

Moneypenny's chat is hidden behind the top bar's Moneypenny button until opened; a dot on
the button says she answered or proposed something meanwhile. Docked, it sits beside the
page from 1200 px wide (drag the handle, or focus it and use the arrow keys, to make it 320
to 720 px wide; `ai6.cfg`'s `[oo.chat]` changes these and its other sizes, see
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
device under `oo.chat.<person ID>`; a phone starts with the chat hidden. **Open
Moneypenny's links at once** in her menu (`oo.chat.<person ID>.follow`, off by default) opens
each page card as it arrives, except while a dialog covers the page.

Web is one Flutter app view: wide windows keep the board beside the selected Session,
narrow windows navigate to it in-page with browser back. It opens no browser popups and
no embedded Flutter views. Native desktop opens each Session, and Moneypenny when popped
out, in its own window sharing the board's state.

## Development

```sh
dart format --output=none --set-exit-if-changed lib test
flutter analyze --fatal-infos
flutter test
```

`test/session_connection_test.dart` decodes the protocol vectors in
`crates/ai6-diptel/tests/vectors/`, so run tests from this directory inside the repository.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
