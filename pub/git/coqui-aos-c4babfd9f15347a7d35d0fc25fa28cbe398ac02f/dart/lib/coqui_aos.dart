/// The Agent Overlay Specification (AOS) in pure Dart.
///
/// Reads an overlay's manifest, validates it, hashes a tree, composes overlays and builds the
/// environment they ask for. The core reads no file itself, so it runs on the VM and on the web;
/// `package:coqui_aos/io.dart` adds the `dart:io` filesystem.
library;

export 'src/apply.dart';
export 'src/catalog.dart';
export 'src/compose.dart';
export 'src/defaults.dart';
export 'src/digest.dart';
export 'src/environment.dart';
export 'src/error.dart';
export 'src/front_matter.dart';
export 'src/fs.dart';
export 'src/hooks.dart';
export 'src/kinds.dart';
export 'src/knowledge_core.dart';
export 'src/knowledge_search.dart';
export 'src/knowledge_envsubst.dart';
export 'src/knowledge_mustache.dart'
    show MustachePartial, MustachePartials, renderMustache;
export 'src/limits.dart';
export 'src/names.dart';
export 'src/overlay.dart';
export 'src/prepare.dart';
export 'src/read.dart';
export 'src/skills.dart';
export 'src/source.dart';
export 'src/subst.dart';
export 'src/template.dart';
export 'src/text.dart' show decodeUtf8, encodeUtf8;
export 'src/tree.dart';
export 'src/validate.dart';
export 'src/value.dart';
