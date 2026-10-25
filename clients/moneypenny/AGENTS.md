# Moneypenny — contributor notes

Moneypenny is the AI6 operator app (Flutter desktop and web). It talks only to Universal
Exports, AI6's authenticated public API.

## Boundaries

- Universal Exports owns Station, Mission, provider Session and terminal-stream state; the app only presents it.
- Never pass M's ingress credential to this application.
- Do not put attach tokens in URLs, browser storage, logs or UI text.
- Terminal input requires an explicit user-scoped write lease; keep viewing read-only by default.
- A window closing only closes the view. It never stops or resumes the provider Session.
- Treat API fields as untrusted input and keep terminal bytes separate from lifecycle metadata.

## Flutter windowing

- Stable Flutter lacks the first-party Desktop Windowing API; use the isolated Flutter `main` SDK described in `README.md` while prototyping native windows.
- Enable `enable-windowing` for that SDK before running or building the Windows client.
- The API is experimental and internal in this Flutter revision; keep its use isolated in this package and do not describe it as production-stable.
- Web uses one ordinary Flutter app view. Session details navigate inside that page;
  do not enable Flutter multi-view or imply it opens browser child windows. A browser
  popup is a separate feature and would need an explicit `window.open` plus messaging.
- Native desktop windows are a separate, experimental Flutter feature. Opening or closing
  a Session window must not start, resume or stop its provider Session.
- The Windows runner starts `FlutterEngine` without a `FlutterViewController` host. `WindowManager` owns all native views, so do not reintroduce the legacy blank host window alongside the Dart-created windows.
- The live terminal uses Guatiao's Dart wire receiver and `web_socket_channel`. Web builds must run `dart run tool/build_guatiao_wasm.dart` before a non-demo Flutter build; this copies the ignored module from the pinned Guatiao Cargo source into `web/`.
- Attachment credentials remain in memory and one-use cookies stay out of URLs and UI. A browser app and Universal Exports must share an HTTPS origin; desktop connections require a public HTTPS gateway origin.

## Verification

From this directory, run `dart format --output=none --set-exit-if-changed lib test`, `flutter analyze`, `flutter test`, and build the Windows and Web targets with the documented main SDK.
