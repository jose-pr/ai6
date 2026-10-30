# `ai6-theater-container` — API header

Header-file-style reference for the `ai6-theater-container` crate: the `ephemeral-container`
Theater provider for AI6, so it can be used without reading its source. It ships inside the
package and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features), `serde` and
`toml`. The crate root exports `ContainerTheaters`, which implements
`ai6::directorates::q::ephemeral::TheaterProvider`, the `Engine` trait, `container_name` and
the label names; the library is both an `rlib` and the Guatiao `cdylib` a Station loads.
Provider ID: `ephemeral-container` (the `container` ID is the transport's). No unsafe code.
The engine's command line client (`docker` or `podman`) must be installed where the Station
runs, the image must be pullable or present, and the Station needs the `container` transport
provider to reach what this one makes.

## Configuration

A template's `access = { ephemeral = { provider = "ephemeral-container", config = { … } } }`;
unknown keys are refused.

| Key | Default | Meaning |
| --- | --- | --- |
| `image` | required | The image each container is made from. |
| `engine` | `docker` | The engine's client; `podman` works the same. |
| `user` | the image's | The user the `container` transport runs commands as. |
| `command` | `["sleep", "infinity"]` | What the container runs to stay up. |
| `run_args` | none | More options for `run`, such as `["--network", "none"]` or a volume. |
| `engine_timeout_ms` | 600000 | How long one engine command may run (pulling an image can take a while); `0` waits for it. |
| `engine_output_bytes` | 1048576 | Bytes of an engine command's output kept. |
| `reader_grace_ms` | 1000 | How long the output readers get to finish once a command has ended. |

## Behaviour

```rust
pub const TEMPLATE_LABEL: &str = "ai6.theater";
pub const MISSION_LABEL: &str = "ai6.mission";
pub const CREATED_LABEL: &str = "ai6.created";     // Unix seconds
pub fn container_name(name: &str) -> String;        // "ai6-<name>"
pub trait Engine: Send + Sync { fn run(&self, args: &[String]) -> Result<Output, ProviderError>; }
impl ContainerTheaters {
    pub fn with_engine(engine: Arc<dyn Engine>, settings_toml: &str) -> Result<Self, ProviderError>;
}
```

- **`capabilities`**: what the `container` transport offers, `run` and `files`, with `os`
  `linux`.
- **`create`**: `<engine> run --detach --name ai6-<name> --label ai6.theater=<template> --label
  ai6.mission=<mission> --label ai6.created=<secs> <run_args>… <image> <command>…`; it answers
  `transport = "container"` with `container`, `engine` and `user` settings. A container that
  already has the name is started again and answered as it is.
- **`destroy`**: `<engine> rm --force ai6-<name>`; one already gone is not an error.
- **`list`**: `ps --all --filter label=ai6.theater=<template>`, then each container's labels
  from `inspect`, oldest first.
- A command runs at most ten minutes, long enough for `run` to pull an image.

## Gotchas

- The real-engine test runs only when `AI6_TEST_THEATER_IMAGE` names an image already present
  (`AI6_TEST_ENGINE` the engine); it never pulls.
- A container has no terminal and no forward, and a harness on another host reaches the
  Station's Cable endpoint through a forward, so a Theater made here works in remote-hands
  mode: set `agents = "remote-hands"` on the template.
- After renaming or removing a provider library, clean the plugin directory: a Station loads
  every provider library it finds.
