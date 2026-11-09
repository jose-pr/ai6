# Agent Overlay Spec conformance vectors

The `*.json` files here are the conformance vectors of the Agent Overlay Spec (AOS), copied
unchanged from `vectors/` of
[Boriken-Dev/coqui-aos](https://github.com/Boriken-Dev/coqui-aos) at commit
`c73cf23ace7dd43c6bd14e9f2f4cb74741aaa3a9`, which holds
[Boriken-Dev/agent-overlay-spec](https://github.com/Boriken-Dev/agent-overlay-spec)'s `tests/vectors/` at
`268fbe4931a9d9aeb33191f0a33e363dab6997cf` (edition `0.1.0-draft`). They are © 2026 Boriken Dev
and distributed under the MIT License, whose text is in [`LICENSE`](LICENSE) beside them; the
rest of this repository is under its own license.

`ai6`'s overlay tests read them as data (`directorates::q::overlays`): every case of
`reading.json`, `validation.json`, `names.json`, `source.json`, `digest.json`, `compose.json`
and `effective.json` must pass, except the cases `NOT_IMPLEMENTED` in `vectors.rs` names: the
overlay code reads an earlier edition, and the test counts those as skipped. `catalog.json` and the `catalog` cases of `reading.json` test
the Store class (catalogs and icons), which AI6 does not implement; the test skips them by name
and counts them.

To take a newer edition, copy the files again and change the commits above in the same change.
