/// The filesystem a caller supplies: the core reads files only through it.
library;

import 'dart:async';
import 'dart:typed_data';

/// What a path is, without following a link.
enum FileKind {
  /// A regular file.
  file,

  /// A directory.
  directory,

  /// A symbolic link or, on Windows, a reparse point.
  symlink,

  /// Anything else: a device, a socket, a pipe.
  other,
}

/// A path's kind and, for a file, its size in bytes.
class Metadata {
  /// Creates the metadata.
  const Metadata(this.kind, this.len);

  /// What the path is.
  final FileKind kind;

  /// The size in bytes.
  final int len;
}

/// Three operations, never following a link in [metadata]. The core joins paths with `/`.
/// An implementation throws what its platform throws; the core reports it as `io`.
abstract interface class FileSystem {
  /// What [path] is, without following a link: a link is [FileKind.symlink].
  FutureOr<Metadata> metadata(String path);

  /// The first [max] bytes of the file at [path].
  FutureOr<Uint8List> read(String path, int max);

  /// The names in a directory, single path components in any order. A name that is not Unicode
  /// is its bytes (a [Uint8List]).
  FutureOr<List<Object>> list(String path);
}

/// [root] and a child [name] as one path.
String joinPath(String root, String name) =>
    root.endsWith('/') || root.endsWith(r'\') ? '$root$name' : '$root/$name';
