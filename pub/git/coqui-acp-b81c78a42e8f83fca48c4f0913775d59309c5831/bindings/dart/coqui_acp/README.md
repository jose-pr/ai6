# coqui_acp

A client for the [Agent Client Protocol](https://agentclientprotocol.com), version 1 and version 2, over any stream of JSON-RPC
messages, for browsers and Flutter. It speaks whichever version the agent selects and never retries anything unacknowledged. It
authenticates nobody: give it a channel only after the peer is proven. Not published; the API may change in any release before 1.0.

## Quick start

```dart
final acp = AcpClient(incoming: lines, send: (line) async => socket.add(line));
await acp.initialize();
final description = await acp.describe();
if (description.options case AcpKnown(:final value)) {
  final models = value.where((option) => option['category'] == 'model');
  // show them to the person
}
final session = await acp.openSession(cwd: '/work');
await acp.prompt(session.id, 'Say hello');
```

## What an agent offers before a session

`describe()`, after `initialize()` and before any session, opens none. Each part is `AcpKnown(value)`, `AcpUnknown(reason)` when the
agent offers the question and could not tell, or `AcpNotSupported()` when it does not offer it, in which case nothing is sent.

- `options`: ACP's `SessionConfigOption`s as JSON maps, of the version spoken (`id` in v1, `configId` in v2), at the values a session
  starts with. A model is the option of category `model`, an effort the one of category `thought_level`. From the
  `_coqui_acp/session_options` request, sent only when the agent's capabilities advertise `dev.boriken.coqui.acp/session_options` in
  `_meta` (`acpSessionOptionsMethod`, `acpSessionOptionsCapability`).
- `modes`: ACP's v1 `SessionModeState`, from the same request. v2 has no modes, so it is not supported there.
- `sessions`: `session/list`, as `AcpSessionInfo`, when v1's `sessionCapabilities.list` is set; in v2 always asked, and a
  method-not-found answer is not supported. Pages are followed by their `nextCursor` until one gives none or an empty one, the
  sessions limit is reached, or the page limit is. The answer is an `AcpSessionList`: its `sessions`, and `incomplete`, null only
  when they are all the agent listed, otherwise `a later page failed: <why>`, `the sessions limit was reached` or `the page limit
  was reached`. A first page that fails decides the answer; a later one ends the list with the pages before it. A page limit of 0
  asks nothing, and the answer is `AcpUnknown`.

A failed or late answer is `AcpUnknown`. What the agent says is untrusted and only read: items that are not ACP's shape are skipped,
`_meta` is dropped, and `describeLimits` on the client bounds the rest. Each field of `AcpDescribeLimits` takes any value from 0 up:

| Field | Default | Bounds |
| --- | --- | --- |
| `options` | 64 | options kept |
| `values` | 1024 | values kept per option, across its groups |
| `modes` | 64 | modes kept |
| `sessions` | 500 | sessions kept, across pages |
| `text` | 4096 | UTF-8 bytes: a longer identifier drops its item, and other text is cut there |
| `pages` | 10 | `session/list` requests: the 500 sessions at 50 a page, and at most ten request timeouts on an agent that always gives a cursor |

```dart
final acp = AcpClient(
  incoming: lines,
  send: (line) async => socket.add(line),
  describeLimits: const AcpDescribeLimits(sessions: 50, pages: 2),
);
```

The Rust client keeps the same, with the same defaults, as `vectors/describe.json` says.

A person's choice is set once the session exists, with `setConfigOption` or `setMode`. Under v2 `setConfigOption` names the
value's type, `id`, as v2 requires; v1 has no type.

## Sessions

`openSession`, `prompt`, `cancel`, `closeSession`; `listSessions` and `reopenSession` for earlier sessions (by `session/resume` where the agent offers it, else `session/load`; `resumeSession` returns the agent's answer); `events` for one
normalized stream of `AcpEvent`s and `updates` for every `session/update` as it arrives. Agent text in `events` gathers until
something follows it, or until `textFlush` UTF-8 bytes have gathered (default 64 KiB, as in the Rust client; any value from 0 up).
`listSessions` keeps at most `maxListedSessions` sessions of a page (default 500; any value from 0 up), and a page cut short by it
says `the sessions limit was reached` in `AcpSessionPage.incomplete`. `takesImages` and `takesFiles` say what an
`AcpAttachment` may be; `AcpAttachment.link`, a `resource_link`, goes to any agent, which ACP has every agent take. `package:coqui_acp/page.dart` is a person's agents from a page, over `document.acp`.

`openSession`, `createSession`, `reopenSession` and `resumeSession` take an `AcpSessionSetup`: `mcpServers` (`AcpMcpStdio`
with an absolute `command`, `AcpMcpHttp`, SSE with `sse: true`), `systemPrompt` (text appended to the agent's own, ACP's `append`
mode), `additionalDirectories` and `_meta`. Each goes in the shape of the version spoken: v1's stdio entry has no `type`, v2's
has one, and v2 has no SSE. `takesMcp`, `takesSystemPrompt` and `takesAdditionalDirectories` say what the agent advertises, and
a setup asking for anything else fails with `AcpFailure` before anything is sent; a relative command or directory, or two
servers of one name, with `ArgumentError`. A server's `toString` names its variables and headers, never their values.

## Permission requests

The `decider` answers each `session/request_permission` of this client's sessions. `AcpPermissionRequest.options` holds what the
agent offers, in its order, as `AcpPermissionOption`s: `optionId`, the agent's `name` (empty when it sent none) and `kind`
(`allow_once`, `allow_always`, `reject_once`, `reject_always`, or another the agent named), with `allows` and `rejects`. An option
without an id or a kind is left out, and at most `maxPermissionOptions` are kept (default 16; any value from 0 up).

| Decision | Answer |
| --- | --- |
| `AcpSelect(optionId)` | that option, which has to be one of the request's own; any other is answered `cancelled`, with an `AcpBlocked` |
| `AcpAllow()` | the first `allow_once` option of those kept, else `cancelled` with an `AcpBlocked` |
| `AcpDeny(reason)` | the first `reject_once` option of those kept, else `cancelled` with an `AcpBlocked` |
| `AcpCancel()` | `cancelled`, as ACP requires for a request still open when its turn is cancelled |

What an `allow_always` or `reject_always` answer remembers, and for how long, is the agent's: the client passes the choice on and
keeps nothing. A request with no `options` list, one whose options repeat an id (which the agent could take for either), or
another session's, is answered `cancelled` without asking the decider, the first two with an `AcpBlocked`. Each
answer is also an `AcpPermission` event, whose `optionId` names the option an `AcpSelect` chose.

## Questions as forms

With an `elicitor`, the client advertises form elicitation (`elicitation.form`, under `clientCapabilities` in v1 and
`capabilities` in v2) and puts each `elicitation/create` of its sessions to it as an `AcpElicitation`: `message`, `title`,
`description`, `toolCallId` and its `fields`, in the agent's order: `AcpTextField`, `AcpChoiceField`, `AcpChoicesField`,
`AcpNumberField` (`integer` for a whole number), `AcpBooleanField`, or `AcpUnsupportedField` for a type this client does not
know, never to be rendered as a known control. The elicitor answers `AcpElicitAccept(content)`, `AcpElicitDecline()` or
`AcpElicitCancel()`. Without an elicitor nothing is advertised and the request is not taken (method not found).

Another session's question is answered `cancel` without asking. One that cannot be put to a person (URL mode, which is not
offered, no session, no form, more than `maxElicitationFields` questions (64) or a question with more than
`maxElicitationOptions` choices (256)) is answered `cancel` with an `AcpBlocked`. An accepted answer is sent only when it fits
the form, every required field there and each value of its type within its bounds; otherwise `cancel` and an `AcpBlocked`. The
Rust client does the same, as `vectors/elicitation.json` says. Everything in a form is the agent's text: show it as data.

## Secrets

The secret-prompt extension (`docs/secrets.md` in the repository) is `package:coqui_acp/secret.dart`. `AcpSecretForm.parseText` reads the JSON Schema text of a form whose secret properties are `writeOnly` or `x-sensitive`; a frontend masks a field from `AcpSecretField.secret` alone. `AcpSecretReceiver.create` makes a key pair for one request and `recipient` is what the collector is sent, bound to the receiver's identity (an `AcpSecretProver`) for the collector's id and for the digest of the `AcpSecretContext`. `AcpSecretVerifier` is a checker: it has its own id, knows the identities it was paired with, and `collector` takes the key only from the identity expected, for that context. `AcpSecretCollector.seal` seals each secret field, `authenticate` binds the whole answer (the plain values text and the sealed ones), and `receiver.openValues` opens it once, after it verified the sender. `AcpSecretRequestParams`, `AcpSecretRequestReply`, `AcpSecretShowOpen`, `AcpSecretShowOpened`, `AcpSecretShowDeliver` and `AcpSecretsCapability` read and write the messages.

An identity's key is never handed to the library: `AcpSecretProver.signing` takes the SPKI and a function that returns the 64-byte P1363 ECDSA signature of a message, and `AcpSecretProver.agreeing` a key agreement for an X25519 identity, so a product wires them to its platform key store or WebCrypto. `AcpLinkKey.derive` takes the per-process secret an application shares with the host it runs in its own process (never sent over a wire, never given to the agent) and `AcpSecretProver.linked` and the verifier's `link` and `linkRole` use it for `link-hmac-sha256`, for that link only. `package:coqui_acp/secret_unprotected.dart` makes provers from raw keys held in memory, for tests and tools only. Dart cannot zero memory, so `AcpSecretValue.clear` overwrites the bytes it holds and nothing prints them; verifying ECDSA runs in pure Dart and is not constant-time. The code runs in a browser too: `tool/test_web.sh` runs the vector tests in Chrome or Chromium (`CHROME_EXECUTABLE`), copying the vectors beside the tests for the run. A string with a lone surrogate is refused wherever it would be hashed (UTF-8 would turn it into U+FFFD), and so is JSON that repeats a key. `vectors/secret.json` holds this and the Rust crate to the same bytes.

## Development

```sh
dart pub get
dart analyze
dart test -p vm
```

The conversations in `vectors/` at the repository's root are shared with the Rust client: a change in how this client behaves on the
wire changes them.

## License

[MPL-2.0](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE).
