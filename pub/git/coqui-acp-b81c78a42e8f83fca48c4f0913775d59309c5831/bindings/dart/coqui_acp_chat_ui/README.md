# coqui_acp_chat_ui

Flutter widgets for an agent chat, over [`coqui_acp_chat`](../coqui_acp_chat): a transcript, the cards, the queue, a composer, a changes
pane and diffs, a **renderer registry** so an app adds its own content without forking, and **`ChatLook`**, the one theme extension they
draw with. It builds for the web (JavaScript and WebAssembly) and for desktop. Not published; the API may change in any release before 1.0.

- **Transcript** with three densities: *Verbose* shows every entry, *Thinking* folds a burst of finished tool calls into one line
  ("Edited calc.js, ran a command +1 −1"), *Normal* also folds a finished turn into "Worked for 1m 29s". A tool call that failed or
  waits is never folded. The agent's plan is a live checklist at the end.
- **Cards**: permission (the agent's own answers, plus slots for the app's), question (a form from the agent's schema), the prompt queue.
- **Composer**: `/` commands and the host's own actions, `@` mentions, paste (an image, files, a long text as an attachment), attachment chips, a `+` menu, Send, Queue and Stop,
  and slots for what is the app's: a source or mode chooser, a header, items in the `+` menu.
- **ChatLook**: semantic tokens (`bg`, `surface`, `raised`, `text`, `muted`, `accent`, `border`, `success`, `warning`, `danger`, sizes,
  `density`, `codeFont`) that default from the Material `ColorScheme`; an app's look is a preset.
- **Renderers**: `ChatRenderers(entry:, tool:, block:, burst:, work:, plan:)`.

```dart
final scheme = ColorScheme.fromSeed(seedColor: Colors.orange, brightness: Brightness.dark);
MaterialApp(
  theme: ThemeData(colorScheme: scheme, extensions: [ChatLook.fromScheme(scheme, density: ChatDensity.normal)]),
  home: Scaffold(
    body: ChatTranscript(
      entries: [for (final message in session.messages) ModelEntry(message)],   // session: a coqui_acp_chat ChatSession
      terminals: session.terminals,
      plan: session.plan,
      working: session.status == ChatStatus.working,
      renderers: ChatRenderers(tool: {'delegate': (context, call) => MyDelegationCard(call)}),
    ),
  ),
);
```

[`AGENTS.md`](AGENTS.md) lists the whole API; [`example/`](example) is a scripted conversation.

## Development

```sh
flutter pub get
flutter analyze
flutter test
(cd example && flutter build web --release && flutter build web --release --wasm)
```
