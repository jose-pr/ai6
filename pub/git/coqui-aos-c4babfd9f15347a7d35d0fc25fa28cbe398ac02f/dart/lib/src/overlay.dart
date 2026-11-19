/// Reading an overlay: its one manifest, its context and its problems.
library;

import 'dart:typed_data';

import 'defaults.dart';
import 'error.dart';
import 'front_matter.dart';
import 'limits.dart';
import 'read.dart';
import 'text.dart';
import 'tree.dart';
import 'validate.dart';
import 'value.dart';

/// What an overlay's files say: the manifest, the context it names, and the JSON pointers of its
/// problems.
class OverlayRead {
  /// Creates the reading.
  const OverlayRead({
    this.manifestFile,
    required this.manifest,
    required this.context,
    required this.contextIgnored,
    required this.issues,
    this.harnessContext,
  });

  /// `OVERLAY.md` or `overlay.cfg`; null when the tree holds neither.
  final String? manifestFile;

  /// The manifest's data model.
  final Document manifest;

  /// The context files the overlay names.
  final List<String> context;

  /// The context files it carries and does not use.
  final List<String> contextIgnored;

  /// RFC 6901 pointers, sorted.
  final List<String> issues;

  /// Per harness that lists context, the files it names; null when the manifest has no
  /// `harness` table.
  final Map<String, List<String>>? harnessContext;
}

bool _isBlank(int byte) =>
    byte == 0x20 || byte == 0x09 || byte == 0x0d || byte == 0x0a;

/// Reads an overlay from a tree holding the bytes of `OVERLAY.md` or `overlay.cfg`. Refuses two
/// manifests (`two-manifests`), a manifest that does not read, and a context body past
/// `contextBytes` or not UTF-8.
OverlayRead readOverlay(OverlayTree tree, Settings settings) {
  if (tree.contains('OVERLAY.md') && tree.contains('overlay.cfg')) {
    throw AosError('two-manifests', 'OVERLAY.md and overlay.cfg');
  }
  final String? manifestFile = tree.contains('OVERLAY.md')
      ? 'OVERLAY.md'
      : tree.contains('overlay.cfg')
      ? 'overlay.cfg'
      : null;
  Uint8List bytesOf(String name) => tree.bytes(name) ?? Uint8List(0);
  Document manifest = <String, Object>{};
  var body = Uint8List(0);
  var bodyLine = 1;
  if (manifestFile == 'overlay.cfg') {
    manifest = readDocument(bytesOf('overlay.cfg'), settings);
  } else if (manifestFile != null) {
    final data = bytesOf('OVERLAY.md');
    final read = readFrontMatter(data, settings);
    final error = read.error;
    if (error != null) throw error;
    manifest = read.data ?? <String, Object>{};
    final offset = read.bodyOffset ?? 0;
    body = Uint8List.sublistView(
      data,
      offset < data.length ? offset : data.length,
    );
    bodyLine = read.bodyLine ?? 1;
  }
  final context = contextOf(manifest, tree);
  final hints = manifest['harness'];
  final harnesses = hints is Map<String, Object> ? hints : null;
  bool lists(Object? entry) =>
      entry is Map<String, Object> && entry.containsKey('context');
  final named = [...context];
  if (harnesses != null) {
    for (final entry in harnesses.entries) {
      if (lists(entry.value)) {
        named.addAll(harnessContextOf(manifest, entry.key, tree));
      }
    }
  }
  if (named.contains('OVERLAY.md') && body.any((byte) => !_isBlank(byte))) {
    if (!allows(settings.contextBytes, body.length)) {
      throw AosError(
        'too-large',
        'the body of OVERLAY.md',
      ).over('context_bytes').onLine(bodyLine);
    }
    try {
      decodeUtf8(body);
    } on AosError {
      throw AosError('invalid-utf8', 'the body of OVERLAY.md').onLine(bodyLine);
    }
  }
  final issues = validate(manifest, tree).map((issue) => issue.at).toList();
  Map<String, List<String>>? harnessContext;
  if (harnesses != null) {
    harnessContext = {};
    final keys = harnesses.keys.toList()..sort(compareText);
    for (final harness in keys) {
      if (lists(harnesses[harness])) {
        harnessContext[harness] = harnessContextOf(manifest, harness, tree);
      }
    }
  }
  return OverlayRead(
    manifestFile: manifestFile,
    manifest: manifest,
    context: context,
    contextIgnored: contextIgnored(manifest, tree),
    issues: issues,
    harnessContext: harnessContext,
  );
}
