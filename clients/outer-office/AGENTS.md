# Outer Office — contributor notes

Outer Office (short name OO) is the AI6 operator app (Flutter desktop and web); Moneypenny
is the AI assistant inside it. It talks only to Universal Exports, AI6's authenticated
public API. Build defines and stored preference keys use the short name (`OO_API`,
`OO_DEMO`, `oo.theme`, `oo.backdrop`); the directory and Dart package keep the long one.
Colours come only from `OoTokens` (`lib/theme/`); icons from the design kit's SVGs through
`OoIcon` (line, small sizes) and `OoDuotone`/`OoTile` (`assets/icons/duotone/`, drawn in the
sentinel colours `#F6D58E`, `#B9832F`, `#2A1C08` that `OoDuotone` swaps for the theme's);
Cinzel (`ooDisplayFamily`) only for the wordmark and section labels. The wallpaper
(`OoBackdrop`, one SVG recoloured from `OoWallpaper`) and the hero art (`OoHero`, Overview only;
files by width in `OoArt`, each within `ooArtBudget`) never sit behind tables,
terminals, Session pages, dialogs, green ink, briefings or the chat, and nothing is plotted
on the hero. Ornament (glows, decorative gradients, art) reads `OoTokens.decorated`, which
high contrast clears; never infer it from colours. Accent edges and glow belong to the
selection and the primary action only. `test/contrast.dart` lists the pairs text is drawn
with and checks each theme's chart series: a new text use on a new background joins that
list. Theme choices go through `OoAppearanceChooser` (`lib/theme/oo_appearance.dart`) and
native windows build their app with `boardThemedApp`.

## Boundaries

- Universal Exports owns Station, Mission, provider Session and terminal-stream state; the app only presents it.
- Never pass M's ingress credential to this application.
- Do not put attach tokens in URLs, browser storage, logs or UI text.
- Terminal input requires an explicit user-scoped write lease; keep viewing read-only by default.
- A window closing only closes the view. It never stops or resumes the provider Session.
- Treat API fields as untrusted input and keep terminal bytes separate from lifecycle metadata.
- Moneypenny's views (`lib/assistant/`) are data in the version 1 view schema, rendered with
  the app's own widgets: never render model-written HTML, run a script or follow an outside
  link; a view the parser refuses is shown as refused with its reason. Proposals run only on
  the operator's Confirm. `test/fixtures/assistant_views.json` is shared with the gateway's
  tests: change both sides together.
- The Memory page (`lib/assistant/memory_page.dart`, route `memory` with `scope=global` or
  `operation_id`) shows one memory at a time: the signed-in person's through
  `/v1/user/assistant/memory`, the global one and an Operation's through `/v1/user/memories`.
  Rows by ID; a save names the revision it opened, and a refusal shows in Universal Exports' own
  words. Each entry shows who wrote it, its compartments and any mark; proposals waiting for the
  person sit above the entries. `test/fixtures/assistant_routes.json` holds the route's cases.

- `lib/forms/` renders any schema and form and names no kind of record, which
  `test/record_forms_test.dart` checks; what is about the roster is in `lib/roster/`. No field,
  title, default, choice or bound is written in Dart: the app shows what the route sends. A record
  type's change regenerates `assets/record_forms.json` (see `README.md`).

## Flutter windowing

- Stable Flutter lacks the first-party Desktop Windowing API; use the Flutter `main` commit `tools/sdk/sdk.lock` names, installed as `README.md` describes.
- `env.sh` and `env.ps1` under the SDK prefix set `FLUTTER_WINDOWING`; any other Flutter needs `enable-windowing` before running or building a desktop client.
- The API is experimental and internal in this Flutter revision; keep its use isolated in this package and do not describe it as production-stable.
- Web uses one ordinary Flutter app view. Session details navigate inside that page;
  do not enable Flutter multi-view or imply it opens browser child windows. A browser
  popup is a separate feature and would need an explicit `window.open` plus messaging.
- Native desktop windows are a separate, experimental Flutter feature. Opening or closing
  a Session window must not start, resume or stop its provider Session.
- Every native window (a Session's, Moneypenny's popped-out chat) opens through
  `openAppWindow` (`lib/shell/app_window.dart`). Moneypenny's conversation and layout live
  in one `MoneypennyChat` per sign-in (`lib/assistant/chat_layout.dart`), so every layout
  and window shows the same conversation and only one `AssistantPanel` is mounted at a time.
  Her shortcuts are one `HardwareKeyboard` handler in `ChatShell`, which also serves her
  window. The preview's scripted sample (`lib/assistant/demo_conversation.dart`) is built
  only when `MissionBoard.demo`; keep it that way (`test/demo_conversation_test.dart`).
- Pages lay out by the width they are given (`LayoutBuilder`), never the window's: a docked
  chat takes part of it. The window width decides only the shell (rail, top bar, chat).
- The Windows runner starts `FlutterEngine` without a `FlutterViewController` host. `WindowManager` owns all native views, so do not reintroduce the legacy blank host window alongside the Dart-created windows.
- The live terminal uses Guatiao's Dart wire receiver and `web_socket_channel`. Web builds must run `dart run tool/build_guatiao_wasm.dart` before a non-demo Flutter build; this copies the ignored module from the pinned Guatiao Cargo source into `web/`.
- Attachment credentials remain in memory and one-use cookies stay out of URLs and UI. A browser app and Universal Exports must share an HTTPS origin; desktop connections require a public HTTPS gateway origin.

## Verification

From this directory, run `dart format --output=none --set-exit-if-changed lib test`, `flutter analyze`, `flutter test`, and build the Windows and Web targets with the documented main SDK.

`lib/issues/linked_issues.dart` holds a Mission's Linked issues section and the issue proposals panel.
Issue text is other people's: show it through `revealInvisible`. An action a person lacks is shown
disabled with what it needs, not hidden. `reasonWords` words a blocked reason everywhere it is shown.

A Mission is shown by its handle (`MissionBoard.handleOf`) and routed and linked by its UID, the
`mission_id` the gateway sends. A route's `mission_id` may be a handle, current or former: the
board matches the current one without case, and the Missions page asks `GET
/v1/missions?mission=` for any other and moves the route to the UID. Renaming is
`lib/shell/mission_rename.dart`.
