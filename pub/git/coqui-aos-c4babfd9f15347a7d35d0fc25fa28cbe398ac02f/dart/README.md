# coqui_aos (Dart)

[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-aos/blob/main/LICENSE)

**The Agent Overlay Specification (AOS) in pure Dart:** read an overlay's manifest, validate it, hash a tree, compose overlays and build the environment they ask for. No Flutter, no native code, no `dart:io` in the core, so it runs on the Dart VM and on the web. The core reads no file itself: a directory is read through a `FileSystem` the caller supplies, and `package:coqui_aos/io.dart` provides one over `dart:io`.

This is the Dart implementation of [coqui-aos](https://github.com/Boriken-Dev/coqui-aos). It passes every one of the specification's conformance vectors in `../vectors/`, none skipped, on the VM and in Chrome, the same ones the Rust, Python and TypeScript implementations pass.

## Installation

Not published. The package has `publish_to: none`; use it from a checkout:

```yaml
dependencies:
  coqui_aos:
    git:
      url: https://github.com/Boriken-Dev/coqui-aos
      path: dart
```

Runtime dependencies: `yaml` (its parser's events expose tags, anchors and aliases before anything is composed) and `crypto` (SHA-256). TOML 1.0 and JSON are read by the package itself.

## Quick start

```dart
import 'package:coqui_aos/coqui_aos.dart';

void main() {
  final manifest = readDocument(
    encodeUtf8('name = "docs"\npriority = 450\n'),
    Settings(),
  );
  print(manifest['priority']); // 450: an integer is a BigInt, a float a double
  print(validate(manifest).length); // 0
}
```

Reading a directory:

```dart
import 'package:coqui_aos/coqui_aos.dart';
import 'package:coqui_aos/io.dart';

Future<void> main() async {
  final settings = Settings();
  final (:tree, :digest) = await readTree(
    const IoFileSystem(),
    '/path/to/overlay',
    settings,
  );
  final overlay = readOverlay(tree, settings);
  print('${digest.digest} ${overlay.issues}');
}
```

## API overview

| Area | Start with |
| --- | --- |
| Reading | `readDocument`, `readFrontMatter`, `readOverlay`, `parseDotenv`, `readEnvScriptOutput` |
| Validation | `validate`, `applies`, `namedFiles`, `skillFlags` |
| Trees and digests | `OverlayTree`, `FileSystem`, `readTree`, `treeDigest`, `digestEntries`, `Digest` |
| Composition | `resolve`, `compose`, `environments`, `effective`, `Facts`, `Environment`, `preparation` |
| Catalogs, skills, hooks | `readCatalog`, `listCatalogs`, `resolveSkills`, `fetchPlanned`, `hookPlan`, `fireEvent` |
| Knowledge through MCP | `kbRead`, `kbList`, `knowledgeCall`, `knowledgeTools`, `KnowledgeOverlay`, `KnowledgeSettings`, `renderMustache`, `renderEnvsubst` |
| Bounds | `Settings`, `Settings.nonDefaults()` |

[`AGENTS.md`](AGENTS.md) is the API header: every public item with its contract and gotchas.

## Development

```sh
dart pub get
dart format --output=none --set-exit-if-changed .
dart analyze --fatal-infos
dart test                                                  # the vectors and the tests beside them, on the VM
dart run tool/stage_vectors.dart && dart test -p chrome    # the web
```

The browser run fetches the vectors from the test server, which serves only this directory, so `tool/stage_vectors.dart` copies them to the untracked `test/vectors/` first.

## License

MPL-2.0, see [LICENSE](LICENSE).
