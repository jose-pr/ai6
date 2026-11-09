# Pairing

Pairing is how two devices that have never met come to trust each other's keys without a
person comparing 52-character fingerprints. The `pair` module runs the exchange; your app
moves its messages, shows what it returns, and decides. Nothing in the crate draws a screen,
logs, or asks anyone.

## Choosing a mode

| Mode | The person does | Use it when |
| --- | --- | --- |
| Invitation | Hands over a link or QR code carrying a 32-byte secret and the inviter's id | The inviter can give the joiner something out of band (a QR code on screen, a link in a message) |
| PIN | Reads a short code on one device and types it on the other | Neither side can verify the other in advance: a new device on the network |
| Device flow | Enters the code the new device shows into the server's own app, then approves | The device already verifies the server (a pin from its configuration, or the web PKI for its name) and has no keyboard worth typing on |

A responder turns off the modes it does not offer (`PairingOptions`).

## What an attacker on the network can and cannot do

- **Invitation.** The joiner sends the secret only over a channel that proved the inviter's
  key, so an attacker in the middle never sees it. Whoever holds the secret pairs, until it
  expires or its uses run out: treat the link like a password.
- **PIN.** The PIN is checked by SPAKE2, a password-authenticated key exchange: watching or
  relaying a run teaches an attacker nothing about the PIN, and an active attacker gets one
  guess per run. With the default single attempt per PIN, a wrong guess burns it and the
  person starts again with a new one, so the chance of success is one in the number of codes
  (10^6 for six digits) per PIN shown. The confirmation covers both devices' keys and, over
  TLS, the connection's exporter: an attacker that relays between two connections, or swaps
  in its own key, makes the run fail on both sides.
- **Device flow.** The device must already verify the server; then only the server sees the
  user code, and the operator approves a request whose device id, address and label the app
  can show. Wrong codes are throttled per approver and in total. If the device does not verify
  the server (an option, off by default), an attacker relaying between them pairs in place of
  both: the operator approves the attacker's code and the device trusts the attacker.
- **All modes.** The number of exchanges in progress, and PIN runs per source address, are
  capped; past a cap the responder answers `Throttled`.

## Options and what they cost

Every number is an option with a default, and none is refused for differing from its default.
`PairingOptions::non_defaults()` and `DeskLimits::non_defaults()` list what differs from the default
so your app can show it. Fewer code combinations or more attempts per PIN raise an attacker's
odds per PIN in proportion; longer lifetimes keep a code guessable or approvable longer; larger
caps allow more attempts per window. Codes are matched exactly over their alphabet: normalising
what a person typed (case, dashes, look-alike characters) is your app's choice.

## Key rotation

A paired peer replaces its key by announcing the successor over a session its current key
authenticated. The trust store records it as pending; the first handshake that proves the new
key promotes it, and the old key keeps working for a window (24 hours by default) so that
connections in flight finish, then is refused. A store can instead require the app to approve
each successor. The store returns rotation events for the app to report.
