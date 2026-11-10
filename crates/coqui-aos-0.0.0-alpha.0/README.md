# coqui-aos

[![Version](https://img.shields.io/crates/v/coqui-aos.svg)](https://crates.io/crates/coqui-aos)
[![Rust version](https://img.shields.io/crates/msrv/coqui-aos.svg)](https://crates.io/crates/coqui-aos)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-aos/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-aos)](https://docs.rs/coqui-aos)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-aos/test.yml)](https://github.com/Boriken-Dev/coqui-aos/actions/workflows/test.yml)

**One reader for the Agent Overlay Specification (AOS) that reads nothing itself:** it takes
bytes, or a `FileSystem` the caller supplies, and returns data or a refusal named by the class the
specification's conformance vectors use. It reads manifests (TOML, JSON and restricted YAML),
validates them, computes tree digests, orders overlays and builds the environment an overlay
declares. Documentation: <https://boriken-dev.github.io/coqui-aos/>.

## Features

- **Manifests in three formats, refusing what the data model has no type for**: a TOML date, a
  YAML tag, anchor, alias, merge key or directive, a null, a duplicate key and an integer past 64
  bits are refused at the parser's events, before anything is expanded. `OVERLAY.md`'s front
  matter is split on bytes and read the same way.
- **No I/O of its own.** A directory is read through `FileSystem` (file kind without following a
  link, bounded reads, listing); `StdFs` is `std::fs` behind the default `fs` feature, and a build
  with `--no-default-features` compiles for `wasm32-unknown-unknown`.
- **Every limit is a setting.** `Settings` holds each bound with its default and no floor, is
  described once by guatiao's derives, and `Settings::non_defaults` lists what differs from the
  default.
- **Catalogs, skills from elsewhere and hooks, without a network or a process**: a catalog and its icons are read, a store's listing chosen from; the core decides which skills to fetch and checks what the caller's `SkillSource` returned against its pin; hooks are read, planned and combined, and the caller runs them.
- **Validation, names, sources, digests, composition and the environment**: the manifest's schema
  and semantic rules, overlay identities, `[git+]<location>[@<ref>][#<path>]` sources, the tree
  digest, `requires` and the composition order, defaults from an overlay's files, the order of
  application with substitution, and setup phases and package environments as pure functions.
- **Held to the specification's vectors.** `vectors/` carries them byte for byte.

## Installation

```sh
cargo add coqui-aos --git https://github.com/Boriken-Dev/coqui-aos
```

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `fs` | `StdFs`, the operating system's files as a `FileSystem` | reading a directory with `std::fs` | on |

## Quick start

Read and validate a manifest:

```rust
use coqui_aos::{Settings, read_document, validate};

let manifest = read_document(b"name = \"docs\"\npriority = 450\n", &Settings::default())?;
assert!(validate(&manifest, None).is_empty());
# Ok::<(), coqui_aos::Error>(())
```

Name an overlay and parse where it comes from:

```rust
use coqui_aos::{Source, SourceKind, normalize_name, valid_overlay_name};

assert!(valid_overlay_name("Docs_Tools"));
assert_eq!(normalize_name("Docs_Tools"), "docs-tools");
let source = Source::parse("git+https://example.org/o.git@v1#overlays/docs")?;
assert_eq!(source.kind, SourceKind::Git);
assert_eq!(source.path.as_deref(), Some("overlays/docs"));
# Ok::<(), coqui_aos::Error>(())
```

Digest a tree that is not on disk:

```rust
use coqui_aos::{TreeEntry, digest_entries};

let entries = [(vec![b"overlay.cfg".as_slice()], TreeEntry::File(b"name = \"x\"\n".to_vec()))];
let tree = digest_entries(entries.iter().map(|(path, entry)| (path.clone(), entry)))?;
assert!(tree.digest.to_string().starts_with("sha256:"));
# Ok::<(), coqui_aos::Error>(())
```

## API overview

| Item | Purpose |
| --- | --- |
| `read_document`, `read_front_matter`, `split_front_matter`, `context_file_text` | bytes to the data model, and the split of `OVERLAY.md` |
| `read_dotenv`, `parse_dotenv`, `read_env_script_output`, `substitute` | env files, script output and `${NAME}` substitution |
| `validate`, `applies`, `Issue` | the manifest's problems, by JSON pointer |
| `valid_overlay_name`, `normalize_name`, `root_variable`, `Source` | identities and sources |
| `FileSystem`, `StdFs`, `tree_digest`, `read_tree`, `digest_entries`, `Digest` | tree digests and a whole directory read |
| `OverlayTree`, `read_overlay`, `rules_of`, `context_of`, `paths_of`, `skills_of` | what an overlay's files contribute |
| `compose`, `resolve`, `composition_order`, `environments` | `requires`, order and package environments |
| `effective`, `Facts`, `Environment`, `preparation` | the environment, setup, runtime and needs for one overlay |
| `read_catalog`, `list_catalogs`, `read_icon` | catalogs, their listing and icons |
| `resolve_skills`, `read_fetched_skill`, `SkillSource` | skills through catalogs, pins and fetching |
| `hook_plan`, `read_hook_output`, `fire_event` | hooks: which, with what, and what applies |
| `Settings`, `Limit`, `Error` | every limit, and the one error type |

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo check --target wasm32-unknown-unknown --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
```

The tests read the specification's vectors from `../vectors`.

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-aos/blob/main/CHANGELOG.md). Pushing a tag
matching `v*` needs the owner's consent for that release.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-aos/blob/main/LICENSE).
