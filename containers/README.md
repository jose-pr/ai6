# Build containers for Outer Office

Dockerfiles for an app builder: an image with every SDK the Outer Office Flutter app needs, this
branch's `pub/` and `cdn/`, and Flutter from a git repository at a tag, branch or hash. Take any
Flutter or Linux image as the base, apply this branch, and point at the Flutter you want: the
one Outer Office is tested on, `main` for the newest multi-window support, or a fork of your own.

Nothing is published. An image is built by whoever needs one. Publishing one is a separate
decision for that image and name.

Supported app targets are Linux, Windows and the web. There is no macOS image to start from.

## Build

From a checkout of this branch (`git clone --branch deps --depth 1 --single-branch
https://github.com/jose-pr/ai6 ai6-deps`), with Podman or Docker:

```sh
podman build -t oo-builder -f containers/Dockerfile.linux containers
```

That is the default: every argument at what `tools/sdk/sdk.lock` names on `main`, and it reports
that exact commit. Another Flutter is one argument:

```sh
FLUTTER_REF=main containers/build.sh -t oo-builder-main
FLUTTER_REF=3.47.9 containers/build.sh -t oo-builder-3.47
FLUTTER_GIT_URL=https://github.com/you/flutter.git FLUTTER_REF=your-branch containers/build.sh -t oo-builder-fork
```

| Build argument | What | Default |
| --- | --- | --- |
| `BASE_IMAGE` | The base layer: any image with a Flutter or a plain Linux, with `apt-get` or `dnf` | an image of [flutter-docker-image](https://github.com/gmeligio/flutter-docker-image), by digest |
| `FLUTTER_GIT_URL` | The Flutter repository | the upstream one |
| `FLUTTER_REF` | A tag, a branch or a hash | the commit `sdk.lock` names |
| `DEPS_GIT_URL`, `DEPS_REF` | This branch's repository and commit | this repository, the commit `sdk.lock` names |
| `RUST_VERSION`, `RUSTUP_VERSION`, `RUSTUP_SHA256_*` | The Rust toolchain and how to verify rustup-init | what `sdk.lock` names |

The default base image is x86-64 only, so on an ARM64 machine pass one that has your architecture
(Debian's and Fedora's do). Running it emulated is not a way round that: under qemu-user on an
ARM64 Linux, rustup crashed with a segmentation fault.

```sh
containers/build.sh -t oo-builder \
  --build-arg BASE_IMAGE=docker.io/library/debian:13-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4
```

On that base, on ARM64, the image is about 3.4 GiB: distribution packages about 1 GiB, Rust
0.7 GiB, `pub/` and `cdn/` 0.25 GiB, Flutter with its Linux and web engines 1.5 GiB. A base with its
own Flutter adds what that one weighs.

### What changes a rebuild

- **A floating `FLUTTER_REF`** changes what a rebuild produces. The image records what it got:
  `build.sh` resolves the ref first and sets the label `ai6.flutter.commit`, and every build prints
  `ai6: FLUTTER_REF <ref> is commit <hash>` and writes the hash to `/opt/ai6/flutter-commit`. With
  plain `podman build`, the label `ai6.flutter.ref` holds what you asked for, and a moved branch
  can be answered from an old cached layer unless you pass `--no-cache`.

  ```sh
  podman image inspect --format '{{ index .Labels "ai6.flutter.commit" }}' oo-builder-main
  ```

- **A `BASE_IMAGE` by tag** instead of by digest is allowed, and is weaker: the tag can move under
  you, and whatever it brings comes with it. `BASE_IMAGE=debian:13-slim` works, and rebuilds are
  then no longer the same.
- **Distribution packages** (clang, GTK and so on) are what the base's package manager offers on
  the day of the build. Pin them by the base's digest and a package snapshot if that matters.
- **Flutter's engine downloads carry no checksum**: `flutter precache` fetches them from
  storage.googleapis.com, or `FLUTTER_STORAGE_BASE_URL`. A fork's engine changes have no artifacts
  there unless you host them.

A ref that does not exist fails the build, naming it. So does a ref whose Flutter does not run,
since the build ends with `flutter --version`.

An image takes `pub/` and `cdn/` from this branch and leaves `crates/` out. The Rust it builds
fetches its crates, or uses `crates/` the way the branch's README says.

`pub/` matches the Flutter it was built with. A Flutter whose own pinned packages differ needs
network on the first `flutter pub get`, or a refreshed `pub/` (the branch's README).

## Use it

From a checkout of `main`:

```sh
podman run --rm -v "$PWD:/src:Z" -w /src/clients/outer-office oo-builder sh -c '
  flutter pub get --offline &&
  dart run tool/build_guatiao_wasm.dart &&
  flutter build web --release &&
  dart run tool/cdn_files.dart fill --deps "$AI6_SDK_DEPS" &&
  flutter build linux --release'
```

The image runs as root, whatever its base's user was. Everything under `/opt/ai6` is writable by
any user, so `--user` works too, with a `HOME` that user can write.

Multi-window is already on (`FLUTTER_WINDOWING=true`); it needs a Flutter from `main`, not stable,
which is why `FLUTTER_REF` exists.

## Windows

`Dockerfile.windows` takes the same arguments, apart from `RUSTUP_SHA256`, which is one hash for
x64. It installs the Visual Studio Build Tools while it builds, because they cannot be
redistributed: the image is your own, and is not pushed to a public registry.

**Unverified.** It has not been built: it needs a Windows container host that can run x64 Windows
containers, which an ARM64 machine cannot. Expect to fix something the first time.

```powershell
docker build -t oo-builder-windows -f containers\Dockerfile.windows containers
```

## Changing the defaults

A new Flutter revision is one change: refresh this branch for the new engine, then update
`tools/sdk/sdk.lock` on `main` and the `ARG` defaults here together. `tools/sdk/check_lock.py` on
`main` fails when they differ.
