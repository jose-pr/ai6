# Liaison: A2A as Cables

Liaison lets agents outside AI6 talk to AI6's Agents over [A2A 1.0](https://a2a-protocol.org/latest/),
and AI6's Agents talk to them. It runs in Universal Exports. A2A is only the wire format at the
boundary: what crosses it is a [Cable](architecture.md#agent-cables), and AI6 routes it with the
same relay, records, grants, compartments, quotas, lifetime, deduplication and audit as any
other Cable. Liaison keeps nothing of its own.

A message never becomes work. Missions have objectives and deliverables; a peer's message is a
message, to one Agent, and it is delivered, not placed or scheduled.

## What crosses, and as what

| A2A | In AI6 |
| --- | --- |
| A peer's `SendMessage` | A Cable from `a2a:<peer>` to one Agent, acting for the peer's principal, stamped with what proved the peer |
| The addressee's answer | A Cable reply (`reply_to_cable`), returned as the A2A `Message` or in the Task |
| Task, `taskId` | The peer's Cable; the Task ID is the Cable ID |
| `contextId` | The Cable conversation |
| Task state | The Cable's delivery: `SUBMITTED` queued or held, `WORKING` delivered, `COMPLETED` replied to, `CANCELED` withdrawn, `REJECTED` refused, `FAILED` expired or not delivered |
| `GetTask`, `SubscribeToTask`, `ListTasks` | Read from the Registry; a peer sees only its own Cables |
| `CancelTask` | Withdraws the Cable while nobody has had it; after that, `TaskNotCancelableError` |
| A Cable to `a2a:<peer>` or `a2a:<peer>/<addressee>` | An A2A `SendMessage` to that peer (with `metadata.addressee` when one is named); its answer comes back as a reply Cable |

Neither a Task ID nor a context ID is ever a Mission ID or a Session ID. A Task is one message and
its replies; the next message in a conversation is a new Task in the same context. Only text
parts are carried (`ContentTypeNotSupportedError` otherwise), and push notifications are not
offered.

## Who may say what

A peer is configured, never discovered, in `[unex.liaison.peer.<name>]` of the gateway's
`ai6.cfg` ([Configuration](configuration.md)). It acts as its `principal` (`person:<id>`,
`account:<name>` or `group:<name>`), never as itself, and that principal is checked as a person
is, by the access policy, on the addressee's Operation:

- `cable.send` to send a message there, and to see the addressee on the card;
- `cable.read` to read a Task there, and to receive the Cables AI6 sends the peer from there.

The relay then asks its own grants, which name the peer's sender `a2a:<name>` like any Agent.
A peer `acme` writing to Agent `007`, and `007` answering, needs all four:

```toml
# the cables record
[[grants]]
agent = "a2a:acme"
action = "send"
scope = "agent"
name = "007"

[[grants]]
agent = "007"
action = "receive"
scope = "agent"
name = "a2a:acme"

[[grants]]
agent = "007"
action = "send"
scope = "agent"
name = "a2a:acme"

[[grants]]
agent = "a2a:acme"
action = "receive"
scope = "agent"
name = "007"
```

Nothing is granted by default. A peer's text is untrusted input, delivered as any Cable's is, and
shown to the Agent as `a2a:acme for person:acme-bot`. Every A2A call is audited
(`liaison.send_message`, `liaison.get_task`, `liaison.list_tasks`, `liaison.cancel_task`,
`liaison.subscribe_to_task`, `liaison.outbound`) with the principal as actor and metadata only,
and each Cable has the relay's own `cable.send` record, written before it is stored.

## Addressees

A peer's `addressees` list what it may write to: `agent:<operation>/<agent>`, or
`role:<operation>/<role>` for the Agent the roster selects for that role when the message
arrives. The card it reads lists them as skills, those its principal may send to now. A message
names one in `metadata.addressee`; a peer with a single addressee may leave it out, and a message
in a known context goes where that context went.

## Threads stay with their Session

A context is bound to the Agent's Session that took its first message: the relay records which
Session took a peer's Cable, and a follow-up in the context reaches that Session only. Once that
Session has ended, the follow-up goes to the Agent's next Session, one opened after it ended,
never to another Session of the Agent that was already open. Until a Session is free to take it,
it waits as any Cable does, until its lifetime ends.

## Reaching Liaison

Two bindings carry the same exchange.

**A proven link.** `[unex.liaison] listen` serves [coqui-agentic-peer](https://github.com/Boriken-Dev/coqui-agentic-peer)
links, the `a2a/1` binding of [coqui-a2a](https://github.com/Boriken-Dev/coqui-a2a). The gateway
proves its link key (`identity`, made the first time), and a dialer must prove one of the
configured `id`s, else nothing of its link is read. A peer with `a2a = false` stays paired and has
its A2A streams refused. Unmodified A2A software uses a link through coqui-a2a's loopback command
(`coqui-a2a loopback --url wss://gateway:7084/link --peer <the gateway's id>`), which serves the
standard HTTP binding on its machine's loopback. The gateway prints its id at start.

**HTTP JSON-RPC on the gateway.** `POST /v1/a2a`, with the card at
`/v1/a2a/.well-known/agent-card.json`, takes A2A's standard JSON-RPC binding (Server-Sent Events
for `SendStreamingMessage` and `SubscribeToTask`). A caller signs in as for any route and sends
its bearer token; the person it is must be one peer's principal, and is then that peer. Without a
token it is 401, and a person who is no peer's principal is 403. A peer with no `id` reaches
Liaison this way only.

## Cables to a peer

An Agent sends to a peer with `send_cable`, scope `agent`, target `a2a:<peer>`, or
`a2a:<peer>/<addressee>` to name one of the peer's own addressees. Liaison carries it on the link
the peer holds open to the gateway, else by dialing the peer's `url` (`wss://` with the public
roots, or `ws://` on loopback), proving the peer's `id`. The peer's answer, a message or a task
followed to its end, comes back to the sending Session as a reply Cable from `a2a:<peer>`. When
the peer refuses or fails, the reply says so; when it cannot be reached, the Cable waits for it,
tried again every `poll`, until it expires. A peer cannot address another peer. A grant naming
`a2a:<peer>` covers `a2a:<peer>/<addressee>` as well.

## Pairing a peer

1. On the peer, make or read its link key and note its id; on the gateway, set
   `[unex.liaison] listen` and start it once to make and print the gateway's id.
2. Give each side the other's id: the peer pins the gateway's, and the gateway's
   `[unex.liaison.peer.<name>] id` names the peer's.
3. Choose its `principal`, give that principal `cable.send` (and `cable.read`) on the Operations
   it may reach, and list its `addressees`.
4. Add the relay grants above for each Agent it talks to.
5. For Cables to it, give its `url`, or have it hold a link open.

## Not here

Files, data parts and push notifications are not carried. A peer's addressee must be listed in
the gateway's configuration; a peer cannot discover Agents beyond its card. A thread bound to a
Session wakes that Session between turns as any Cable does; once the Session has ended, it waits
for the Agent's next Session to read it, and is not put in a brief.
