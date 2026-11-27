# The link

A link is one proven, sealed connection between two peers, carrying named streams. This page is the wire, so any language can implement it. `vectors/mux.json` holds the frames as data, and every implementation runs them.

## Layers

| Layer | What it does | Where |
| --- | --- | --- |
| Transport | WebTransport or WebSocket: a channel a browser can open | `coqui-transport` channels; `coqui-transport-web` carries WebTransport and WebSocket |
| Proof | shows who holds a key, and seals every message after that with keys from the proof | a method of `coqui-peer` |
| Streams | this page | `cabuya` |
| Capabilities | any protocol, each its own messages | each protocol's own library |

A stream carries a protocol's messages unchanged. The link adds proof and streams, never a dialect.

## The proof's method

The proof is one of two methods, chosen per endpoint and never negotiated on a connection: [`noise`](https://github.com/Boriken-Dev/coqui-peer-noise), `Noise_XX_25519_AESGCM_SHA256` with PIN pairing in the browser, or [`webcrypto`](https://github.com/Boriken-Dev/coqui-peer-webcrypto), P-256, for a browser whose WebCrypto has no X25519. Nothing on the wire says which: a peer running the other method fails the handshake, and nothing falls back. A peer id is the hash of a key of one type, so it can be proved only by its own method, and a client that stores the method with the id cannot be talked into the other.

An identity is a key of one method, and is never used by the other. A new one is `noise`'s unless another is named; one that exists keeps its method, so a `webcrypto` peer paired before stays paired until a person moves it: a new `noise` identity, then pairing again, since a peer id names a key of one type. 
Each record of a session is longer than its message by what its method adds: 17 bytes under `webcrypto`; under `noise` 19 bytes for every 65,518 bytes of message or part of it, so 19 up to 65,518 and 4,883 at 16 MiB. A transport's frame limit is set from that length, never from the message's.

## Frames

Each message on the link is one frame:

| Bytes | Field |
| --- | --- |
| 1 | kind |
| 4 | stream id, little-endian |
| rest | body |

| Kind | Name | Body |
| --- | --- | --- |
| 0 | open | one byte, the token's length; a protocol token; metadata, the rest |
| 1 | data | the last or only part of a message, never empty |
| 2 | credit | a little-endian `u32`, never zero: bytes read |
| 3 | close | one byte: 0 done, 1 refused, 2 broken, 3 superseded, 4 revoked; then a reason in UTF-8, at most 256 bytes, possibly empty |
| 4 | ping | none, on stream 0 |
| 5 | pong | none, on stream 0 |
| 6 | more | a part of a message, with more to come, never empty |
| 7 | finish | none: this side will send nothing more on the stream, and still reads |

A receiver MUST treat anything else as a peer not speaking this, and end the link.

A **protocol token** is printable ASCII with no space, comma or equals sign, at most 64 bytes, so it can go in a DNS-SD record as it is: `files/1`, `rpc/1`, `tcp/1`, `dev.example.app/tether`. Tokens are open: a program may use its own, under a domain it controls, and a client ignores one it does not know.

## Streams

The side that connected uses odd ids and the other even ones, so the two never choose the same id. Ids only go up, so a stream that has ended cannot be opened again. An `open` with an id from the wrong side's range, or one not above the last, ends the link, so a sender puts its `open` frames on the link in the order it chose their ids, whatever threads open at once. Ids may skip: an `open` that could not be sent leaves a gap.

Either side may open a stream, whoever connected. The other either serves it or answers `close` with `refused`, because nobody serves that protocol there or this peer may not open it. A proof admits a peer to the link, and being admitted to the link grants no protocol.

**Metadata** on an `open` is opaque to the link, and for the side accepting it: a token, a session key, a resume cursor. It goes with one stream and is kept nowhere, so what rotates or belongs to one conversation stays out of the link's credentials.

A message may be cut into `more` frames and a last `data`, 64 KiB each by default, so a long one cannot hold other streams up for as long as it takes, and an interrupt passes between two parts of someone else's message. The parts of one message arrive in order, and only a whole message is delivered.

`finish` is half a close: a request answered on the same stream sends it after the request. The peer's reader then sees the end after what was sent, which tells a finished message from a truncated one, and it may go on answering. `data` after `finish` breaks the stream. `close` ends both directions.

A reader gets the same end of stream from a peer that sent `finish` and from a stream or link closed in order with no `finish`, so an implementation exposes whether `finish` arrived (`finished_by_peer`, `finishedByPeer`). A protocol that needs the whole of a message tells them apart: a close without `finish` may have cut the message short.

## Delivery

A stream delivers in order or fails, and never skips. Nothing is ordered across streams, so what needs one order shares a stream, and what must overtake a full stream, such as an interrupt, takes its own.

**The link replays nothing.** A bounded window would mean silent loss after a long drop. A host that needs to survive one reopens a stream with its own cursor in the metadata.

Each way a stream ends is its own, so a host can tell what to do next: closed in order, refused, broken, superseded, revoked, or the link lost. `superseded` means a newer stream took this one's place, so the holder must not simply reopen it, and it is sent before the old stream closes.

## Heartbeat

Optional, off unless set, at link level. A side sends `ping` every interval, and `pong` answers any `ping`. A side that has heard nothing at all for its interval times its allowed misses treats the link as lost. A stream silent for hours is not affected, since a link with live peers answers whatever its streams are doing. Set the interval under a reverse proxy's read timeout, which it also keeps from closing an idle link.

## What a link offers

Any admitted peer may open `link/1`. The other side answers with one JSON message and finishes:

```json
{ "version": 1, "instance": "9f2c…", "peer": "sha256:…", "offers": ["files/1", "rpc/1"], "window": 1048576, "max_frame": 65536 }
```

`offers` is what this peer may open there now, which is authoritative where a DNS-SD record's `offers` is only a hint. `instance` is random per process, so a peer seeing a different one after a drop knows the other side restarted and lost what it held, as against a blip. `window` and `max_frame` are that side's initial window and largest frame, so a host can report a difference (see Flow control); an older peer leaves them out.

A key is one entry in one store, so a new capability is a grant on that entry and never another pairing. Each open is decided against the grants, and each running stream again at an interval the host sets. A grant taken away ends its stream with `revoked`. A store that cannot be read refuses, so nothing fails open.

## Forwarding TCP

`tcp/1` carries one TCP connection per stream. The opener puts the target in the stream's metadata as `HOST:PORT` in UTF-8, with an IPv6 address in brackets. The answering side compares it, spelled one way (a lowercase name, a shortened IPv6 address), against what this peer was granted: it needs `tcp/1` and a grant token `tcp/1:RULE` that matches the target. A rule is `HOST:PORT`: HOST is an exact name, `*.suffix` (any name under it, not the suffix itself), `*` (any target), an address or a CIDR network (`10.0.0.0/8`, `[fd00::]/8`), and PORT is `N`, `A-B` or `*`, so a token naming one target is a rule that matches only that target. Nothing is resolved when a rule is checked: an address or network rule matches only an address target, a name rule only a name target, and `*` both, so `localhost:80` and `127.0.0.1:80` are different targets and a grant on addresses has to name addresses. An IPv4-mapped IPv6 address is the IPv4 address it maps. and a name is resolved by the side that connects, so a grant on a name trusts that name's DNS. A host that ends in a number has to be a plain dotted quad, since resolvers read `010.0.0.1` and `2130706433` as addresses other than they look, and an unspecified address is refused.

A target is decided before anything connects, so a refusal says nothing about what is there. It is decided again while the stream runs, so removing either grant closes the stream with reason `revoked`. A granted target that cannot be reached closes it with reason `refused`.

A stream's data frames are the connection's bytes in order, with no framing of their own. `finish` is a FIN: each direction ends by itself, and a client that stops sending still gets its answer. A stream closed in order ends the connection once what it sent has been written. Ended any other way, or when its link goes, it resets the connection, so what was cut short does not read as finished, and a side whose connection fails closes the stream with reason `refused`. Either side may open, so a forward runs in either direction, and each side decides from its own grants.

## Flow control

Each direction of each stream starts with a window of credit, 1 MiB by default. Sending a message costs its size in credit. A sender MAY start a message only while its credit is above zero, so a message larger than the window still passes, one at a time. A receiver returns credit as the application reads, not as the link delivers.

The initial window is not negotiated: both sides start a stream from it, so it is one setting for a deployment. A receiver that wants a stream wider grants credit beyond what it has read, as one `credit` frame, and raises the window it holds that stream to by the same amount, so the check below uses it. A sender counts credit without a cap below 2^32 - 1, so every implementation already accepts it and the peer needs no change. How far a stream may grow is the receiver's own `max_window`, since the receiver pays the memory. A receiver may also grow a stream by itself: while data arrives with none outstanding it sends a `ping`, counts the bytes that arrive until the `pong`, and if that is over two thirds of the stream's window doubles it, up to `max_window`. One stream moves at most its window per round trip, so on a long path this is what lifts it.

So a stream whose reader has stopped stalls only itself, and a receiver holds at most its window and one message for each stream. A peer that sends past that loses that stream, with `close` and `broken`, and keeps the link. Credit is what every other stream depends on, so a frame the mux owes the peer is not dropped when the link is momentarily full.

## Telling a link from a plain message

After the proof, the side that connected speaks first. A first message whose first byte is 0 is an `open`, and the link has streams. JSON never starts with a zero byte, so anything else is a peer speaking one JSON protocol directly, which keeps working. Nothing has to be agreed first, so this works through any proxy, on either transport, with no path to configure.

## Limits

A peer can make a mux hold no more than: 32 streams, 16 of one protocol, 8 opens nobody has accepted, 4 KiB of metadata on each, 600 opens a minute, and on each stream its window (up to `max_window` where it was widened) and one message of at most 16 MiB, which an unfinished message counts against. These are defaults, and each is a setting. An `open` past them is refused, not queued.

A frame adds 5 bytes, so the link's own largest message has to be 5 bytes larger than the largest frame.
