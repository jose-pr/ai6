/// An overlay's files as the core sees them.
library;

import 'dart:typed_data';

import 'error.dart';
import 'names.dart';
import 'text.dart';

/// An overlay's files as the core sees them: the paths it holds, and the bytes of those it was
/// given. Immutable: [withText] and [withBytes] return a changed copy.
class OverlayTree {
  OverlayTree._(this._paths, this._bytes);

  /// A tree of these paths, none of whose bytes are held.
  factory OverlayTree.fromPaths(Iterable<String> paths) =>
      OverlayTree._(Set.of(paths), const {});

  /// A tree of these files and their text.
  factory OverlayTree.fromTexts(Iterable<(String, String)> files) {
    final bytes = <String, Uint8List>{};
    for (final (path, text) in files) {
      bytes[path] = encodeUtf8(text);
    }
    return OverlayTree._(bytes.keys.toSet(), bytes);
  }

  /// A tree of these files and their bytes, which are copied.
  factory OverlayTree.fromBytes(Iterable<(String, Uint8List)> files) {
    final bytes = <String, Uint8List>{};
    for (final (path, data) in files) {
      bytes[path] = Uint8List.fromList(data);
    }
    return OverlayTree._(bytes.keys.toSet(), bytes);
  }

  /// An empty tree.
  factory OverlayTree.empty() => OverlayTree._(<String>{}, const {});

  final Set<String> _paths;
  final Map<String, Uint8List> _bytes;

  /// A copy with the text of a path the tree holds; a path it does not hold is ignored.
  OverlayTree withText(String path, String text) {
    if (!_paths.contains(path)) return this;
    return OverlayTree._(_paths, {..._bytes, path: encodeUtf8(text)});
  }

  /// A copy that holds [path] and a copy of its bytes.
  OverlayTree withBytes(String path, Uint8List bytes) => OverlayTree._(
    {..._paths, path},
    {..._bytes, path: Uint8List.fromList(bytes)},
  );

  /// The paths, in byte order.
  List<String> paths() => _paths.toList()..sort(compareText);

  /// Whether the tree holds [path].
  bool contains(String path) => _paths.contains(path);

  /// The text of a path whose bytes were read, if they are UTF-8.
  String? text(String path) {
    final bytes = _bytes[path];
    if (bytes == null) return null;
    try {
      return decodeUtf8(bytes);
    } on AosError {
      return null;
    }
  }

  /// The bytes of a path that was read; they are the tree's own, so do not change them.
  Uint8List? bytes(String path) => _bytes[path];

  /// The paths an overlay may name, in byte order: valid relative paths.
  List<String> members() => paths().where(validRelativePath).toList();

  /// Whether [path] is a path the tree holds and an overlay may name.
  bool hasMember(String path) =>
      _paths.contains(path) && validRelativePath(path);
}
