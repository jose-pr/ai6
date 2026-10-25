# `ai6-transport-adb` — API header

Header-file-style reference for the `ai6-transport-adb` crate: the `adb` transport provider
for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features), `serde` and
`toml`. The crate root exports `Adb`, which implements
`ai6::directorates::q::transport::Transport`, and the `Runner` trait; the library is both an
`rlib` and the Guatiao `cdylib` a Station loads. Provider ID: `adb`. No unsafe code. The
`adb` executable must be installed where the Station runs.

## Configuration

A Theater's `access = { transport = { provider = "adb", config = { … } } }`; unknown keys are
refused.

| Key | Default | Meaning |
| --- | --- | --- |
| `serial` | none | The device as `adb devices` lists it (`emulator-5554`, `192.168.240.112:5555`). Without it adb takes the only device and refuses when there are several. Passed as `-s`. |
| `executable` | `adb` | The adb executable. |

## Capabilities

`run`, `files` and `forward`; `os` is `android` and `arch` comes from `probe()`
(`ro.product.cpu.abi`, as `aarch64`, `arm`, `x86_64` or `x86`). No `terminal`. An Android
device cannot host the harness, so give its Theater `agents = "remote-hands"`.

## Behaviour

- **`run`**: `adb shell '<line>'` with `<line>` the kit's command line
  (`cd '<cwd>' && exec env 'K=V' 'argv0' 'arg'…`, every word single-quoted for Android's
  `sh`). It relies on the shell protocol of adb 1.0.36 and Android 7 or later for a separate
  stderr and the program's exit code; an older pair reports `0` for everything. Standard input
  is passed. A timeout kills the local `adb` client; the program on the device is ended only
  if the device's shell hangs up on it.
- **Files**: built from `wc`, `tail`, `head`, `cat` and `mkdir` run by `adb shell` (the kit's
  shell file operations), not `push` and `pull`, so they need no temporary files and support
  offsets. A name holding a tab or a newline is not listed correctly. Paths are the device's;
  an app cannot see another app's files, and `/data/local/tmp` is the usual writable place.
- **`forward`**: `adb reverse tcp:<remote_port> tcp:<target port>`; the device's `localhost`
  reaches the Station's loopback. `target` must be `127.0.0.1:PORT` or `localhost:PORT`.
  `remote_port` `0` lets the device choose (adb prints the port). Closing removes the reverse.
- **`probe`**: `adb get-state` must say `device`; anything else is an error naming the state.

## `Runner`

`Adb::with_runner(Arc<dyn Runner>)` builds the transport over any implementation of
`Runner::adb(&self, args, &ProcessInput) -> Result<Output, ProviderError>`, which is how a
test stands in for the adb command line. The configured provider uses the process runner.

## Gotchas

- A test against a real device runs only when `AI6_TEST_ADB_SERIAL` names it; it never picks a
  device itself.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
