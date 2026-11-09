# The WebCrypto method: wire profile v1

**Status:** experimental. It has had no independent security review and is not an IETF or ACP
standard. Implementations: Rust (`src/`) and JavaScript (`bindings/js/coqui-peer-webcrypto/`),
checked against each other and against `tests/vectors/handshake.json`.

## What this is

An authenticated key exchange followed by an encrypted channel, between two peers who each hold a
P-256 key. It runs over any reliable, ordered message transport and depends on nothing beneath it:
no TLS session, certificate, origin or host name. Both peers end the handshake holding a session id
and keys, and **only records made with those keys are tied to the peers that were proved**.

The handshake is signed ephemeral Diffie-Hellman with both identities inside the signatures. Its
closest analysed relative is protocol `sig-dh` in Canetti and Krawczyk, "Analysis of Key-Exchange
Protocols and Their Use for Building Secure Channels" (Eurocrypt 2001, ePrint 2001/040), section 4.2,
which that paper calls very similar to a protocol in ISO/IEC 9798-3. It signs both parties, a session
id and both exponentials, and the paper proves it SK-secure and proves such a protocol, with a MAC and
symmetric encryption, gives secure channels. **That proof covers the simple form, not this instance**:
here the identities are public keys, the session id is built from the nonces, a third signature
(`accept`) is added, keys come from HKDF and records are AEAD. This instance has no analysis of its
own.

Records are AES-256-GCM with a 96-bit deterministic nonce: 32 fixed bits then a 64-bit counter, the
split NIST SP 800-38D section 8.2.1 suggests, under a key used for one direction of one session.
Section 8.3's limit of 2^32 invocations does not apply to that construction. Unlike TLS 1.3 (RFC 8446
section 5.3), the sequence number is not XORed with a secret IV.

### Why not Noise

Noise is a real alternative and this is a choice, not a constraint. `Noise_XX_25519_AESGCM_SHA256`
would have no signatures, so it would leave no transferable evidence, and it would hide the initiator's
key from anyone but the authenticated responder. WebCrypto now offers X25519, in current browsers
with a non-extractable key (checked in Chrome 156), and AES-GCM and SHA-256 were always there, so it
could be built with no dependency, and published analyses cover Noise's patterns (Kobeissi and
Bhargavan, ePrint 2018/766; Dowling, Rosler and Schwenk, ePrint 2019/436; Girol et al., USENIX Security
2020).

This profile stays with P-256 because P-256 ECDSA and ECDH are long established across WebCrypto
implementations, whereas X25519 is recent (Chrome 133 and Firefox 130 by MDN's data; Safari's first
version is not settled in the sources checked), so some deployed browsers lack it; and Noise's
specification (revision 34, section 12) defines only Curve25519 and Curve448, so Noise over P-256 would
be off-specification and lose what those analyses give. A Noise profile would be a separate method with
its own name and wire, and is a decision for the project to take before this wire is frozen.

## Identity

| Field | Value |
| --- | --- |
| Method name | `webcrypto` |
| Profile | `dev.boriken.coqui.peer-webcrypto/v1` |
| Public key | SubjectPublicKeyInfo DER of a P-256 key with an uncompressed point: always 91 bytes, the 27 bytes `3059301306072a8648ce3d020106082a8648ce3d03010703420004` then the 64 coordinate bytes |
| Signature | ECDSA with SHA-256, IEEE P1363: 64 bytes `r` then `s` |
| Peer id | `sha256:` and the lowercase hex SHA-256 of the 91 key bytes: coqui-peer's `PeerId` |
| Key material | method `webcrypto`, label `PUBLIC KEY`, the 91 key bytes |

One key has one encoding and so one id. These keys are not `coqui-peer-tls` certificate keys, and a
certificate's fingerprint is not an id here.

## Handshake

A message is a UTF-8 JSON object in exactly one encoding: the members below in this order, no
whitespace, no escapes, nothing else. Binary values are unpadded base64url with zero trailing bits.
Every message starts `"profile":"dev.boriken.coqui.peer-webcrypto/v1","version":1,"type":...`.

| # | From | `type` | Other members |
| --- | --- | --- | --- |
| 1 | Initiator | `hello` | `publicKey`, `ephemeral`, `nonce` |
| 2 | Responder | `challenge` | `publicKey`, `ephemeral`, `nonce`, `signature` for role `server` |
| 3 | Initiator | `proof` | `signature` for role `client` |
| 4 | Responder | `accept` | `signature` for role `accept` |

`nonce` is 32 fresh random bytes. `ephemeral` is a fresh P-256 ECDH key as 65 bytes, `04` then the
coordinates, used for this run only and then erased.

Order of checks. The initiator verifies the `server` signature, then its trust in the responder's
key, and only then signs. The responder verifies the `client` signature, then its trust in the
initiator's key, and only then derives keys and signs `accept`. The initiator verifies `accept`.
Neither side treats the peer as proved before that point. Trust is never decided on an unproved key,
so an unknown key and a bad signature look the same on the wire: the connection ends with no message.

A responder keeping a trust store records a proved, unknown peer as waiting, per coqui-peer's
key-proof contract. Nothing enrolls a peer automatically.

### What is signed

The signature input is these fields, each as a 4-byte big-endian length then its bytes:

1. the profile identifier
2. the role: `server`, `client` or `accept`
3. the initiator's public key
4. the responder's public key
5. the initiator's nonce
6. the responder's nonce
7. the initiator's ephemeral key
8. the responder's ephemeral key
9. the outer binding (empty unless both ends set one; see below)

So each side's signature names both parties' keys, both nonces, both ephemeral keys and the binding:
a signature cannot be replayed into another run, reflected to another role, or made to vouch for
another peer.

### Session id and keys

- **Session id**: SHA-256 of the fields above without the role. Both ends compute it, and both
  signatures cover everything it hashes, so one id at both ends means both signed for one pair of
  keys and one run. It is not secret: whoever carried the handshake can compute it.
- **Keys**: `HKDF-SHA256(salt = absent, ikm = the ECDH shared secret's x coordinate,
  info = field("<profile>/keys") || field(session id))`, 96 bytes: the initiator's sending key, the
  responder's sending key, and the export secret. `field` is the same length-prefixed form as above.
  The transcript goes in `info`, not the salt: RFC 5869 section 3.4 assumes a salt independent of the
  input keying material, and the id hashes the ephemeral keys that material comes from.
- **Exports**: `HKDF-SHA256(salt = absent, ikm = export secret,
  info = field("<profile>/export") || field(label) || length as 4 big-endian bytes)`. Secret to the two
  peers and bound to this run, and another label or length gives unrelated bytes.

## Records

After the handshake, each message is one record: `AES-256-GCM(key of the sending direction, nonce =
4 zero bytes then the 8-byte big-endian sequence number, no associated data)`, ciphertext then the
16-byte tag, carried whole as one binary message. Each direction's sequence starts at 0 and rises by
one per record, and is never reused or wrapped.

The plaintext of a record is one inner type byte and then its contents:

| Type | Meaning | Contents |
| --- | --- | --- |
| `01` | data | the application message, possibly empty |
| `02` | close | nothing; the last record of its direction |

**A stream that ends without a close record is reported as truncated**: the transport ending does not
say the stream is complete, the close record does. Each direction closes on its own, so one side can
go on receiving after it closed. After `seal`ing a close a side seals nothing more; a record after the
peer's close is refused.

A record that fails to open, has any other type, or carries contents in a close, ends the session in
**both** directions: nothing later is believed, and nothing more is sent.

Each direction also has a limit, a setting that defaults to 2^38. Each record counts its length plus
32, about the bytes of AES blocks it uses, so the default sits below the 2^38.5 bytes of RFC 8446
section 5.5's 2^24.5 full-size records for a margin of about 2^-57. There is no rekeying. At the limit,
sealing or opening a longer record is refused with a limit error, the session stays intact, a close
record (17 bytes) still passes, and carrying on takes a new handshake. There is no floor: a limit of
zero passes only a close.

## Through a relay

Anything carrying the connection is a relay: a TLS-terminating reverse proxy such as nginx, or an
attacker holding a certificate, or one a user was lured to. The method needs no cooperation from it
and no TLS beneath it.

| A relay **can** | A relay **cannot** |
| --- | --- |
| forward the run unchanged, which only connects the two real parties | read a record, or learn the session's keys or exports |
| see both public keys and peer ids (a `hello` names its sender before the responder is proved), the session id, and each record's size and time | inject, alter, reorder, replay or reflect a record |
| block, delay or cut the connection, which a peer reports as truncated | end a stream so that it looks complete: it has no key to make a close record |
| | replace a key, ephemeral key or nonce: every signature covers them, so the other side's verification fails |
| run a handshake of its own under its own key, against a peer that trusts that key or accepts any proved key | make a peer that pins another id accept its key, or use a proof from another run |

**The right-hand column holds only for what goes through the session.** A peer that speaks plain
application messages beside the records, trusting the handshake, hands the whole connection to the
relay: it forwards the four messages, both ends succeed, and it then reads and writes as it likes.

It does not protect against a party that serves the page. A browser runs the code its origin sends, so
if the reverse proxy also serves the page's JavaScript, it can replace that code. Ship the module
in the application (an extension, a packaged app), or serve it from where the carrier cannot change it.

### The outer binding

Where each end really terminates one TLS connection itself, both may pass that connection's exporter
as the binding. It is never sent. Both signatures cover it, so a relay ending TLS on both sides gives
the two ends different values and the handshake fails: it refuses such a relay, rather than only
blinding it. It is optional and adds nothing to what the session already protects. Through a
reverse proxy, or from a browser, no two ends can read one value, so it stays empty.

## Pairing

`coqui_peer::pair` runs on a session of its own, bound to the session and not to any TLS beneath it.
A joiner is a stranger, so the responder admits any proved key for this. The Rust implementation
enforces what follows from that with a type: such a handshake ends in a `PairingSession`, which holds
the proved key and runs the exchange, and has no way to carry an application message. A session that
can carry one exists only with a peer a pin or a trust store admitted.

The `Channel` is `local` this side's key, `remote` the key the handshake proved, `binding` a 32-byte
export under `EXPORTER-coqui-pair`, and `verified` when the joiner admitted the responder by a pin or a
store. Each message is carried as a record; the crate's drivers do this. Once paired, both sides close
and reconnect, and the full key is what ordinary handshakes prove.

What each mode needs from the joiner's handshake:

- **Invitation.** The secret goes as is, so the channel must be confidential and the proved key must be
  the inviter's. `Pairing::join` refuses otherwise, and the secret is only ever sent inside a record.
  The joiner should also pin the inviter, which stops anyone else at the handshake.
- **PIN.** A proved key of anyone is enough: the PIN authenticates both keys and the binding. A relay
  forwarding everything pairs the real parties with each other and learns nothing. A relay forwarding
  between two sessions of its own fails at the first message, since the keys differ. A relay that runs
  the exchange itself with each side, under its own key, has one guess at the PIN per run, as
  `coqui_peer::pair` states, and otherwise fails the confirmation.
- **Device flow.** The joiner must have verified the responder by a pin or a store, or `join` refuses
  unless told `allow_unverified`, which lets a relay pair in place of both sides.

The JavaScript module has no PIN exchange: a browser's key is paired by carrying its id out of band.

## Bounds and refusal

Every message is checked before anything else against a limit, a setting that defaults to 3072 bytes
of UTF-8. The exact encoding above is then enforced by decoding and re-encoding, so duplicate or
unknown members, member order, whitespace and escapes are all refused alike. Refused too: any other
profile or version, a key that is not the exact shape above or is off the curve, a signature with `r`
or `s` out of range, either side presenting the other's key. There is no negotiation, so nothing to
downgrade. `s` may be high or low: WebCrypto does not normalise it, so signatures are malleable, and
nothing here uses a signature as an identifier.

Errors never quote what the peer sent. A failed handshake drops what it had queued and stays failed.

## Erasure

Forward secrecy holds only if the ephemeral keys and session keys are erased. What the implementations
actually do:

- **Rust.** Ephemeral scalars, traffic keys, the export secret, long-term keys and PEM text are held on
  the heap in place and zeroed when dropped, and `tests/erasure.rs` scans every freed block for them.
  Copies a move leaves on the stack, and the working state of `p256`, `hkdf`, `sha2` and `aes-gcm` on the
  stack, are not erased. The binding `pair_channel` returns is not zeroed by `coqui_peer`.
- **JavaScript.** Key bytes are wiped where the module holds them, and every key after that is a
  non-extractable `CryptoKey` it cannot wipe. It drops its reference to the ephemeral key when the
  handshake ends, and `session.destroy()` drops the session's keys; when the memory is cleared is up to
  the engine's garbage collector.

## Not in this protocol

- **Identity hiding.** The initiator's key is sent before the responder is proved; a responder signs
  for anyone who says `hello`. A key can be tracked by whoever sees it.
- **Deniability.** Each signature names both keys and the run, and a third party can verify it. Whoever
  holds a transcript, either peer or anyone it hands one to, holds transferable evidence that the signer
  took part in that run. It does not reveal what was said.
- **Limiting pre-authentication work.** Every `hello` costs a responder a signature and an ephemeral key,
  and costs the sender nothing; a `proof` from a stranger costs a verification. The protocol has no
  cookie or puzzle, so bounding this is the host's: admission by address, connections at once, a deadline
  on the handshake. A responder derives no keys until the proof verifies and the peer is admitted. A
  message is refused by its length before it is parsed, and the crate's `FramedStream` refuses a frame by
  its declared length before reading it, where `coqui_peer::drive_stream` would take up to 1 MiB.
- **Revocation of a running session.** Trust is read at each handshake. A host that removes a peer
  closes that peer's open connections itself.
- **Rekeying.** A session ends when its sequence does; a long-lived one reconnects.
- **Keeping a page's key from its own origin.** A non-extractable key cannot be copied out, but any
  script in the origin can use it, and IndexedDB's contents can be cleared by the browser or the user.
