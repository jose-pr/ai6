# Agent Overlay Spec conformance vectors

The `*.json` files here are the conformance vectors of the Agent Overlay Spec (AOS), copied
unchanged from `tests/vectors/` of
[Boriken-Dev/agent-overlay-spec](https://github.com/Boriken-Dev/agent-overlay-spec) at commit
`061b8f5534fff36926fd06c3d3b510f0513f29e9` (edition `0.1.0-draft`). They are © 2026 Boriken Dev
and distributed under the MIT License, whose text is in [`LICENSE`](LICENSE) beside them; the
rest of this repository is under its own license.

`ai6`'s overlay tests read them as data (`directorates::q::overlays`): every case of
`reading.json`, `validation.json`, `names.json`, `source.json`, `digest.json`, `compose.json`
and `effective.json` must pass. `catalog.json` and the `catalog` cases of `reading.json` test
the Store class (catalogs and icons), which AI6 does not implement; the test skips them by name
and counts them.

To take a newer edition, copy the files again and change the commit above in the same change.
