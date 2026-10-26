# `ai6-transport-container` — API header

Header-file-style reference for the `ai6-transport-container` crate: the `container`
transport provider for AI6, so it can be used without reading its source. It ships inside the
package and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features), `serde` and
`toml`. The crate root exports `Container`, which implements
`ai6::directorates::q::transport::Transport`, and the `Runner` trait; the library is both an
`rlib` and the Guatiao `cdylib` a Station loads. Provider ID: `container`. No unsafe code. The
engine's command line client (`docker` or `podman`) must be installed where the Station runs,
and the container must already be running.

## Configuration

A Theater's `access = { transport = { provider = "container", config = { … } } }`; unknown
keys are refused.

| Key | Default | Meaning |
| --- | --- | --- |
| `container` | required | The running container, by name or ID. |
| `engine` | `docker` | The engine's client; `podman` works the same. |
| `user` | the container's | The user to run commands as (`-u`). |

## Capabilities

`run` and `files`. No `terminal` and no `forward` (an engine has no reverse port forward).
`probe()` checks that the container is running (`inspect`), reads `os` and `arch` from
`uname -sm` inside it, and sets `files` to `false`, with a note, when the container has no
`sh`.

## Behaviour

- **`run`**: `<engine> exec [-i] [-u user] [-w cwd] [-e K=V]… <container> <argv>…`. The
  program is started directly, with no shell in between, and the engine's own `-w` and `-e`
  carry the working directory and environment. `-i` is added only when there is standard
  input. A timeout kills the local engine client; whether the program inside stops depends
  on the engine.
- **Files**: the kit's shell file operations run with `exec <container> sh -c`, so the
  container needs a POSIX `sh` with `wc`, `tail`, `head`, `cat` and `mkdir` (BusyBox has
  them). A distroless image offers `run` only. A name holding a tab or a newline is not
  listed correctly.

## `Runner`

`Container::with_runner(Arc<dyn Runner>, container, user)` builds the transport over any
implementation of `Runner::engine(&self, args, &ProcessInput) -> Result<Output, ProviderError>`,
which is how a test stands in for the engine. The configured provider uses the process runner.

## Gotchas

- A test against a real engine runs only when `AI6_TEST_CONTAINER` names a running container
  (`AI6_TEST_ENGINE` the engine). `AI6_TEST_CONTAINER_MEMORY_MB`, the container's
  `--memory` limit in MiB, also checks that its resources report that limit.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
