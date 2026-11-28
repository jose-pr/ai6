# coqui_acp_chat

The state of an agent chat, built from [ACP](https://agentclientprotocol.com) session updates, in pure Dart: no Flutter, no transport,
no widgets. It runs on the Dart VM and in browsers (JS and wasm), and any UI draws from it. Not published; the API may change in any
release before 1.0.

It is the model half of a chat. A connection (an [`coqui_acp`](../coqui_acp) client, or a channel of your own) feeds a
`ChatSession` the agent's `session/update`s and its requests; the session keeps the conversation and takes the person's side:
prompts, the queue, answers.

- **Messages and thoughts**, built from v1 chunks (by order) and v2 messages (by id, patched or replaced).
- **Tool calls** with their status, locations, text, diffs, patches and terminals; the plan as a checklist; compactions; usage.
- **Permission requests and questions** that wait for the person, with Bypass; **a request from a connection that ended is
  withdrawn** and its tool line marked `stopped`, never answered.
- **The prompt queue**: messages written during a turn go when it ends, held (not dropped) after a stop or a failure; **steer**
  puts one ahead and ends the turn.
- **Several attached clients**: each prompt is stamped with the client that sent it, **the first answer to a request wins** and the
  others see it settled. (`replay()` for a client that attaches late, and views of one conversation in another window or page, `ChatSessionShare` and `ChatSessionView`, are experimental, in `package:coqui_acp_chat/experimental.dart`.)
- **Turns** with their start, end, duration and outcome.
- **Every limit is a setting** (`ChatLimits`) with a default, taking any value from 0 up.

```dart
final store = ChatStore();
final connection = AcpChatConnection.routed(
  store,
  (decider, elicitor) => AcpClient(incoming: lines, send: send, decider: decider, elicitor: elicitor),
);
await connection.client.initialize();
final opened = await connection.client.openSession(cwd: '/work');
final session = store.add(title: 'Fix the test', agent: 'Claude', sessionId: opened.id)..attach(connection);

session.addListener(() => print(session.messages.length));   // redraw
await session.send('Fix the failing test');   // completes once the agent has been handed it; queued if a turn is running
// when the agent asks: session.pendingPermissions.first.options ...
session.choosePermission(session.pendingPermissions.first.options.first);
```

[`AGENTS.md`](AGENTS.md) lists the whole API.

## Development

```sh
dart pub get
dart format --set-exit-if-changed .
dart analyze
dart test -p vm
dart test -p chrome      # the browser (also `-c dart2wasm`); only the import graph and the embedded-text check read files, and run on the VM
```

`test/support/embedded.dart` holds, as text, the kind and shape (never the text) of every update a real agent sent in a coding task, and `vectors/usage.json`.
`vectors/usage.json` at the repository's root holds the usage model's conversations, shared with the Rust crate.

## License

[MPL-2.0](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE).
