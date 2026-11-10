# Agent Overlay Spec conformance vectors

The `*.json` files here are the conformance vectors of the Agent Overlay Spec (AOS), copied
unchanged from `vectors/` of
[Boriken-Dev/coqui-aos](https://github.com/Boriken-Dev/coqui-aos) at commit
`c73cf23ace7dd43c6bd14e9f2f4cb74741aaa3a9`, which holds
[Boriken-Dev/agent-overlay-spec](https://github.com/Boriken-Dev/agent-overlay-spec)'s `tests/vectors/` at
`268fbe4931a9d9aeb33191f0a33e363dab6997cf` (edition `0.1.0-draft`). They are © 2026 Boriken Dev
and distributed under the MIT License, whose text is in [`LICENSE`](LICENSE) beside them; the
rest of this repository is under its own license.

`ai6`'s `tests/aos_vectors` reads them as data and runs every case of the eight files against the
`coqui-aos` crate's types: `reading.json`, `validation.json`, `names.json`, `source.json`,
`digest.json`, `compose.json`, `effective.json` and `catalog.json`. None is skipped.

To take a newer edition, copy the files again and change the commits above in the same change.
