# coqui_acp_chat: API header

Pure Dart, `import 'package:coqui_acp_chat/coqui_acp_chat.dart';`. Depends only on `coqui_acp` (path `../coqui_acp`; its protocol
types are this package's input). No Flutter, no `dart:io`, no `dart:html`: it runs on the VM and in browsers, and a test checks the
import graph. Not published; alpha.

## The shape

`ChatStore` holds `ChatSession`s. A session is one conversation: its state is read through getters (lists and maps are unmodifiable
views) and changed only by its methods; it tells `addListener` listeners after each change and `events` (a stream) for what
happens once. A `ChatConnection` carries the agent's updates in and the session's requests out; the session never opens, verifies
or retries anything. Everything an agent sends is bounded by `ChatLimits` and read as untrusted: wrong types are ignored, text is
cut between characters, names lose control and bidi characters. Words for a person to read (usage figures, mode labels) belong
to the UI, not here.

## Connection

- `abstract class ChatConnection`: `updates: Stream<AcpSessionUpdate>`, `events: Stream<AcpEvent>`, `isClosed`, `protocolVersion`,
  `prompt(sessionId, text, {attachments}) -> Future<String?>` (v2: the inserted message's id; v1: completes when the turn ends),
  `cancel`, `setMode`, `setConfigOption -> Future<Map>`, `close`. Optional: `reportsUsage` (false), `usageNotices`, `fetchUsage`.
- `AcpChatConnection(AcpClient)`, or `AcpChatConnection.routed(requests, (decider, elicitor) => AcpClient(...))`, which wires the
  client's decider and elicitor to `requests` (a `ChatRequests`, which `ChatStore` is) under this connection.
- Requests are routed by the connection they came on **and** the session id (two agents can both say `s1`):
  `ChatStore.askPermission(request, from)`; none: `AcpDeny` (question: `AcpElicitCancel`).
- A v1 turn ends by the connection's `AcpCompleted`/`AcpCancelled`/`AcpFailed` for **this session's id** while it is working
  (`coqui_acp` names the session in them), or else by the answer to its prompt. An end for a session that is not working is
  ignored. `AcpOutputLost` carries no session and is noted in every session on the connection.

## ChatStore (extends `ChatNotifier`)

`ChatStore({limits, recorder, hostNoticesKey, clock})`. `add({title, agent, peerId, sessionId, cwd}) -> ChatSession` (inserted first,
selected), `sessions`, `selected`, `selectedId`, `select(id)` (clears `unread`), `waiting`, `attention: Stream<Attention>`
(`({session, kind})`; a stopped turn is not news; a finished turn in a session not selected sets `unread` **before** any listener is
told), `forget(session)` (detach, keep the connection), `close(session)` (answer what waits cancelled, end the connection when no
other session uses it), `dispose()`.

## ChatSession (extends `ChatNotifier`)

`ChatSession({required id, title, agent, peerId, sessionId, cwd, limits, recorder, hostNoticesKey, clock, agentNames = true})`.
Host-owned plain fields: `agent`, `peerId`, `sessionId`, `cwd`, `unread`; hooks `onChange`, `onAttention`, `onModeKept(mode, bypass)`.

- **State** (getters): `messages` (`ChatMessage`: `role`, `text`, `at`, `messageId`, `tool`, `compaction`, `sentBy`, `turn`),
  `toolEntries`, `compactions`, `terminals`, `plan`, `commands`, `options`, `usage`, `usageReport`, `spend`, `reportsUsage`,
  `status` (`disconnected`/`connected`/`working`), `title`, `agentNames`, `turns` (`ChatTurn`: `startedAt`, `endedAt`, `outcome`,
  `duration(now)`), `currentTurn`, `queue`, `queueHeld`, `pendingPermissions`, `pendingQuestions`, `settledPermissions`,
  `settledQuestions`, `pendingChoices`, `bypass`, `mode`, `underlyingMode`, `inBypass`, `chosenMode`, `targetMode`, `refusedModes`,
  `opening`, `isReplaying`, `clients`, `connection`.
- **Children**: `children` (`ChatChild`: `id`, `order`, `parentToolCallId`, `title`, `task`, `state` (`ChildState`: `starting`, `working`, `completed`, `failed`, `cancelled`; null until reported), `result`, `model`, `usage` (`ChildUsage`: `totalTokens`, `toolUses`, `durationMs`), `updates`, `reportsProgress`), `child(id)`. An update whose `_meta[childMetaKey]` (`dev.boriken.coqui.acp/child`) has an `id` is the child's: a `tool_call_update` of the parent's own call id is a report and changes the child (never the parent's tool line); any other joins `child.updates` and never the conversation's messages. A child with no updates shows its task and result only (`reportsProgress` false). Bounds: `ChatLimits.children`, `childUpdates`.
- **Input**: `apply(AcpSessionUpdate)` (a connection's updates arrive by themselves), `applyOpened(AcpOpenedSession)`,
  `attach(connection)` / `detach()`, `rename(name)`.
- **Turns**: a turn starts at a v1 prompt or a v2 `running` state and ends on idle, an end event, the answer to a v1 prompt, or the
  loss of the connection (failed). `ChatMessage.turn` is the turn running when an entry was added, and a prompt belongs to the turn
  it starts. `TurnEvent(turn)` goes out on `events` for every end, cancelled included; what a reopen replays ends none.
- **Prompts**: `send(text, {attachments, from}) -> Future<void>` completes when the agent has been handed the message (v2: its
  acknowledgement; v1: the prompt is under way and was not refused at once). The message is in the conversation before the agent can
  answer, takes its id from the acknowledgement or its echo, and a prompt that is refused leaves nothing behind (a v1 prompt the agent fails before it said anything too; one it echoed in v2 stays). Queued while a turn
  runs, another message is being handed over, or the conversation is being opened (sent after the kept mode is back; held if it did not open); throws `StateError` past `queued`. `steer(text) -> QueuedMessage?` goes first, even past `queued`, and with a turn running
  cancels the turn; it returns the message, whose `handedOver` completes when the agent has it, and a steer that cannot be asked of the agent (it throws) or cannot be sent after the turn
  ended (`handedOver` fails with the reason, also when the turn ended any other way than cancelled) leaves the queue, for the host to give back to the person.
  `send` and `steer` take `origin`, kept on the `QueuedMessage` for the host (never read here).
  `steerQueued`, `sendQueued([m])`, `editQueued`, `removeQueued([m])`, `cancel({from})`.
- **The queue**: `mergeQueued(m)` joins a message to the one above it (texts with a blank line, one attachment per name; returns the one they became, null for the first; throws
  `StateError` over `ChatLimits.attachments` or across two clients, changing nothing), `mergeAllQueued() -> int`, `moveQueued(m, by)`, `unqueue(m) -> bool` (out of the queue, its `text` and
  `attachments` still readable, for the host to put back in the draft), `sendMerged()` (a held queue as one prompt). `queueDelivery` (`QueueDelivery.each`, the default, or `merge`):
  with `merge` the first send after a turn that ended as it should joins what waits, as many as one prompt holds, and the rest follow in the next turns.
- **Requests**: `PermissionQuestion` (`title`, `kind`, `detail` what it would run or touch, `reason` why the agent says it needs it, `diff` the
  change an edit would make as a `ToolDiff` (`describeToolDiff`), `options` in the agent's order, `forPerson`, `toolCallId`,
  `outcome`), `AgentQuestion`. `answerPermission(allow:, by:)`, `choosePermission(option, by:)`, `cancelPermission`,
  `settle(question, decision, by:)`, `answerQuestion`, `settleQuestion`: each returns whether **it** settled the request (the first
  answer wins; `outcome` is `Settled`: `how` answered/cancelled/withdrawn, `by`, `at`, `answer`). A request whose connection ended
  or was replaced, or whose conversation was let go, is **withdrawn**: its tool line is `stopped` and the agent is told it was
  cancelled (so it never waits). Bypass is checked before the waiting limit.
- **Modes and options**: `choose(option, value) -> Future<bool refused>` (held while a turn runs or the conversation opens),
  `holdChoice`, `setBypass(on)`, `chooseMode(ModeEntry)`, `cycleMode()`, `leaveBypass()`, `canChangeMode`. Mode changes run one at a
  time in the order asked; presses go on from `targetMode`; a mode the agent refused is in `refusedModes` and cycling skips it; nothing
  changes while `opening`: `chooseMode`, `cycleMode` and `setBypass` do nothing then, so the kept mode holds (`leaveBypass` still leaves the chat's own Bypass, which only makes requests asked, and the ledger hears of it once it is open). `modeEntries(option)`, `nextMode`, `modeOrder`, `isBypassMode`, `bypassAnswer`, `ownBypass`.
- **Reopening**: `reopen(open, {keptMode, keptBypass}) -> Future<AcpOpenedSession>`: when the agent sends the conversation it
  replaces what is held (nothing twice); when it only resumes, what is held stays; history is not news; open tool lines become
  `stopped`; the kept mode and Bypass are put back before anything is sent. A second `reopen` while one is under way throws
  `StateError`. A turn running when it starts ends as cancelled (a `TurnEvent`) and holds the queue; a queue the person held stays
  held, and so does one behind a kept mode the agent did not take (it is then in `refusedModes`, and `chosenMode` follows the agent)
  or that could not be put back; only otherwise are messages written while it opened sent.
- **Clients**: `attachClient(id, {name})`, `detachClient(id)`; `from:`/`by:` must name an attached client (or `bypassClient`).
- **A host's own part** (all optional, plain fields): `permissionPolicy` (`PermissionPolicy`: `prepare(request)` awaited in request order, `review(request, {forPerson}) -> PermissionReview(forPerson, label, ownCall, hostData)` (`forPerson`: the request is already the person's by what it asks), `answer(request, review) -> PermissionAnswer(decision, note)?` asked after Bypass; with one, requests are decided one after another and Stop, close or a lost connection answers those still waiting cancelled; `PermissionQuestion.hostData` carries what `review` attached), `bypassOnceOnly`, `agentModeIsChosen` (true: a mode the agent moved to by itself is the chosen mode; false: shown only), `modeLabel(Choice)` (words a mode in notes and cycling), and hooks `onPermissionSettled(question)`, `onPrompt(text, attachments)` (after held choices went, before the agent has the prompt), `onChoice(role, value)` (the agent took a choice a person made), `onOptions()`, `onTitle(title)`, `onAgentMode(before, now)`. Content a host puts in: `note(text)`, `addMessage(role, text, {at})`, `retitle(name)` (the agent may still name it), `holdFirst(text, {attachments})` / `releaseFirst()` / `heldFirst` (a draft's first message shown as the person's own before the agent has started; `send` replaces it, keeping its time), `presetBypass(on)` (silent), settable `agentNames`, `dropChoices({where})`, `removeMessages(where)`. A choice made with no connection is held (`PendingChoice.byPerson`, `toJson`/`fromJson`) for the agent that comes.

## Experimental: `package:coqui_acp_chat/experimental.dart`

Not exported by the main library, because the wire a session hub would carry them on is not specified and the `_meta` keys are
provisional: `ChatSessionHub` on `ChatSession` adds `replay() -> List<AcpSessionUpdate>` (messages with `sentBy` and times, tool
calls, terminals, compactions, turns with their times and outcomes, plan, commands, options and the v1 mode, usage, title and
whether the person named it), `applyReplay(updates)` (applies them as history, not news), and `settleElsewhere(question, by:,
optionId:)` (`by` must be attached). Requests still waiting are not carried: read `pendingPermissions`. `chatClientMetaKey`, `chatAtMetaKey` (an update's time) and `chatNamesMetaKey`.

**Views** (the same import): one conversation shown in another place, such as a window popped out of the one that holds it. One
copy owns it and talks to the agent; each view shows a read-only copy that follows it and asks through it, so there is one model and
the first answer to a request settles it for everyone.

- `ChatViewLink`: a channel of JSON maps (`messages`, `send`, `close`, `isClosed`); implement it over a platform channel, a
  `BroadcastChannel` or a pipe. `MemoryViewLink.pair({wire: true})` is two ends in memory that carry JSON only.
- Owner: `ChatSessionShare.of(session, {minInterval: 80 ms})` (or `ChatSessionShare(SharedChat)` for a conversation kept some other
  way: `snapshot() -> ChatSnapshot`, `listen`, `attachClient`, `detachClient`, `send`, `cancel`, `settlePermission`, `settleQuestion`).
  `serve(link)` takes a link whose far end is a `ChatSessionView.attach`; `views`, `events` (`ChatViewEvent`: a window came or went),
  `flush()`, `close()` (every view is told, and ends). Each view is sent the whole conversation as `replay()` makes it plus what waits
  and what was settled, after each change, at most once per interval (longer when building it takes long) and never twice the same.
- View: `await ChatSessionView.attach(link, clientId:, name:, limits:, timeout:)` throws `StateError` when refused (client id in
  use, wire version, bypass as an id) or unanswered. `session` is a `ChatSession` that mirrors the owner (messages, turns, queue,
  clients, `pendingPermissions` under the owner's ids, `settledPermissions` with who answered); do not call its `send` or `cancel`.
  `send(text)` (completes when the owner took it, throws when it could not; text only, no attachments yet), `cancel()`,
  `choosePermission(question, option|null)`, `answerQuestion`, `detach()` (dock back: the owner goes on as it was), `isOpen`,
  `done` (the owner's reason, or null).
- `ChatSnapshot` (`toJson`/`fromJson`, wire `chatViewWireVersion` 1) is what a state carries; `ChatSessionHub.resetFromReplay`,
  `adoptPermission` and `adoptQuestion` are what a view builds its copy with.
- Not yet: attachments from a view, steering, choosing a mode or option from a view, and a state that carries only what changed
  (each state is the whole conversation).

## Content types and parsers

`ToolCall` (`id`, `title`, `kind`, `status: ToolStatus` pending/running/done/failed/cancelled/stopped, `blocks`, `locations`,
`isOpen`, `changedPaths`, `stop()`) with `ToolBlock`: `ToolText`, `ToolDiff(path, oldText?, newText)`, `ToolPatch(text, paths)`,
`ToolTerminal(terminalId)`. `TerminalView` (`command`, `output` from its end, `exitCode`, `isRunning`); `PlanEntry`/`PlanStatus`;
`Compaction`; `AgentCommandInfo`; `ConfigOption`/`Choice` (`value`, `label`, `selectable`, and `description`: what the agent says the value is, read from ACP's `description` of an option value or a mode, cut like any text shown)/`OptionRole`/`roleOf`/`optionIn`/`modeOptionIn`/`PendingChoice`;
`SessionUsage`; `UsageReport.parse(json, [limits])` (the `dev.boriken.coqui.acp/usage` extension: buckets, spend, `fractionPermille`,
`resetsIn`; strings sanitised, lists cut at `usageBuckets`), `UsageSupport.parse`; `reasonOf(error)`, `titleFrom(text, [limits])`,
`toolRuns(entries)` (consecutive tool calls grouped, for a transcript that folds them), `BoundedText` (appendable bounded text). Identifiers an agent chooses (tool, terminal, compaction, option and choice ids) are used as they came or not at all: empty, longer than `shortText` or holding a control character, they are dropped.

## Recording

`UpdateRecorder({write})` records each update's kind and shape (never its text) and `unshown`, the kinds that reached no rendering;
`renderingOf(update, limits) -> Rendering`. A host's test fails when `unshown` is not empty.

## ChatLimits (every field: any value from 0 up)

`entries` 5000, `messageText` 2 MiB, `pendingPermissions` 64, `pendingQuestions` 16, `settledRequests` 32, `queued` 20, `attachments` 8 (per joined queued message), `clients` 16,
`turns` 1000, `toolText` 256 KiB, `terminalOutput` 256 KiB (kept from its end), `planEntries` 200, `commands` 500, `configOptions` 32,
`choices` 200, `toolBlocks` 64, `locations` 64, `shortText` 512, `terminals` 256 (one still running is never dropped), `describedInput`
4000, `hostNotices` 4, `listShapes` 8, `shapeDepth` 8, `titleLength` 60, `usageBuckets` 64, `children` 256, `childUpdates` 1000, `handOverSeconds` 60 (0: no bound; a v2 agent that does not acknowledge a prompt in time fails `send`, and the queue behind it is held). A request past `pendingPermissions` or
`pendingQuestions` is answered cancelled and a notice says so; the names of what went with messages are remembered up to `entries`.

## Gotchas

- `status` is `working` from a v2 `state_update` running, or at once for a v1 prompt.
- `hostNoticesKey` (default none) names the `_meta` key of the host's notices in the answer to `session/new`; they show as the
  agent's word, never the chat's.
- Attachments are `AcpAttachment`s already built; reading files, size checks and what an agent takes are the host's.
- Tests: `dart test -p vm`; `dart test -p chrome` (and `-c dart2wasm`) for the browser; the files tagged `@TestOn('vm')` read files
  (`embedded_test.dart` holds the text the other tests carry to the files it came from; `import_graph_test.dart`).
