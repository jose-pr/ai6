# cabuya

**One proven connection between two peers, and any number of named streams over it.** Pair once, then run whatever protocols an application needs, each as a stream with its own flow control and its own grant. The link adds proof and streams and nothing to the protocols: a stream carries their own messages.

- [The link](link.md): the wire, for any language.
- [The program](cli.md): `cabuya`, which pairs, serves, connects and forwards from a command line.

Three rules hold the design together:

- **A proof admits, a grant allows.** Being proven opens no stream. Each open is decided against the peer's record, and each running stream again, so taking a grant away ends its stream.
- **Nothing is negotiated.** An endpoint proves by one method, chosen when it is built; a peer of the other fails the handshake.
- **One wire, many languages.** `vectors/mux.json` holds the frames as data, and the Rust, Dart and JavaScript implementations each run them.

The streams, the serving and the peer store need no cryptography. The `sealed` feature adds the proof and PIN pairing, and `forward` adds TCP forwarding.
