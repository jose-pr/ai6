# `@boriken/coqui-peer-webcrypto` — API header

Every export of this dependency-free ES module (`src/index.mjs`) with its contract, so it can be used
without reading the source. One file, so it can be vendored by copying it. It needs `crypto.subtle`:
a browser needs a secure context; Node 22 or later has it. The wire is `PROTOCOL.md` of
<https://github.com/Boriken-Dev/coqui-peer-webcrypto>, which the Rust crate of that name also
implements. Not independently reviewed; it may change in any release before 1.0.

```js
export const PROFILE;              // 'dev.boriken.coqui.peer-webcrypto/v1'
export const METHOD;               // 'webcrypto'
export const DEFAULT_MAX_MESSAGE;  // 3072
export const DEFAULT_SESSION_LIMIT; // 2 ** 38
export const profile;              // {id, version, method}

export async function generateIdentity();              // -> identity
export async function importIdentity(pkcs8);           // PKCS#8 DER P-256 bytes -> identity
export async function loadOrCreateIdentity(name);      // IndexedDB database `name`, required -> identity
// identity: frozen {method, peerId: 'sha256:<hex>', publicKey: base64url SPKI, privateKey: CryptoKey}

export async function createInitiator(identity, expectedPeerId, options);
// -> {hello() -> string, async answer(raw) -> string, async finish(raw) -> {peerId, session}}
export async function createResponder(identity, {allowClient, ...options});
// -> {async challenge(raw) -> string, async accept(raw) -> {reply: string, peerId, session}}
// allowClient: async (peerId) -> exactly true to admit; runs after the peer proved its key

// session (frozen):
//   id                          Uint8Array(32), both ends agree; not secret
//   async seal(bytes | string)  -> Uint8Array record; send whole, in the order of the calls
//   async close()               -> Uint8Array, the last record of this direction
//   async open(record)          -> Uint8Array message, or null for the peer's close
//   verifyEnd()                 throws when the peer's close never came; call when the transport ends
//   async export(label, length) -> Uint8Array, secret to the two peers; length is bound in
//   destroy()                   drops the session's keys; every later call fails
```

`raw` is a message as text or bytes, at most `maxMessage` bytes. Options: `maxMessage` (default
`DEFAULT_MAX_MESSAGE`, no floor); `sessionLimit` (per direction, each record counting its length and 32;
default `DEFAULT_SESSION_LIMIT`, no floor; a longer record is then refused, a close still passes, and a
new handshake carries on: there is no rekeying); `binding`, a `Uint8Array` or `Buffer` holding the bytes
of an outer channel both ends really read (a TLS exporter where each end terminates that TLS itself; never
through a reverse proxy or from a browser), and anything else throws; `random` and `ephemeral`, replacing
the generators **for test vectors only**. The module reads `seal` and `open` input when called, so the
caller may reuse its buffer.

An identity's private key must be non-extractable, P-256 and able to sign, and `peerId` must be the
hash of `publicKey`, or `createInitiator` and `createResponder` throw. Each application names its own
`loadOrCreateIdentity` database: the name is its identity. Concurrent tabs end up with one key.

The flow: the initiator sends `hello()`; the responder answers `challenge(raw)`; the initiator answers
`answer(raw)`, having verified the responder and its pin; the responder ends with `accept(raw)`,
having verified the client and `allowClient`, and sends `reply`; the initiator ends with `finish(raw)`.

Then **every application message goes through the session**, as a binary message:

```js
socket.binaryType = 'arraybuffer';
socket.send(await session.seal(JSON.stringify(message)));
const text = new TextDecoder().decode(await session.open(new Uint8Array(event.data)));
```

End a stream with `socket.send(await session.close())`, and when the transport ends call
`session.verifyEnd()`: a relay can cut the connection at any record boundary, and that is how a cut shows.

A plain message on the same transport comes from whoever carries it, such as a TLS-terminating reverse
proxy, even after a successful handshake. A record that fails to open ends the session in both directions:
close the connection. When done, `session.destroy()`. The module drops the ephemeral key when the handshake
ends and wipes the key bytes it holds; a `CryptoKey`'s memory is cleared when the engine collects it.

## Errors

Every failure is a thrown `Error` whose message quotes nothing the peer sent; a handshake object is
over after one, and a later call says `Unexpected ...`. What is refused is in `PROTOCOL.md`.

## Gotchas

- A non-extractable key cannot be copied out, but any script in the origin can use it.
- A proxy that serves the page can replace this module. Ship it with the application.
- Signatures are transferable evidence that a key took part in a run; see `PROTOCOL.md`.
- Each `hello` costs a responder a signature for free: bound connections and time in the host.
- There is no PIN exchange here: a browser's id is carried to its peer out of band.
- Nothing bounds time: the caller sets a deadline on the handshake.
