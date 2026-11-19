/// The tree digest: which files an overlay is, hashed in a form no platform changes.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:crypto/crypto.dart' as crypto;

import 'error.dart';
import 'fs.dart';
import 'limits.dart';
import 'names.dart';
import 'text.dart';
import 'tree.dart';

final RegExp _hex = RegExp(r'^[0-9a-f]{64}$', caseSensitive: false);

/// A SHA-256 digest. Written `sha256:` and 64 hexadecimal digits.
class Digest {
  Digest._(this._bytes);

  /// The digest of [bytes].
  factory Digest.of(Uint8List bytes) =>
      Digest._(Uint8List.fromList(crypto.sha256.convert(bytes).bytes));

  /// Reads `sha256:<64 hex digits>`, or refuses with `invalid-digest`.
  factory Digest.parse(String text) {
    final hex = text.startsWith('sha256:')
        ? text.substring('sha256:'.length)
        : '';
    if (!_hex.hasMatch(hex)) {
      throw AosError(
        'invalid-digest',
        'a digest is sha256: and 64 hexadecimal digits',
      );
    }
    final bytes = Uint8List(32);
    for (var index = 0; index < 32; index += 1) {
      bytes[index] = int.parse(
        hex.substring(index * 2, index * 2 + 2),
        radix: 16,
      );
    }
    return Digest._(bytes);
  }

  final Uint8List _bytes;

  /// The 32 bytes.
  Uint8List bytes() => Uint8List.fromList(_bytes);

  /// Whether [other] is the same digest.
  bool equals(Digest other) => toString() == other.toString();

  @override
  bool operator ==(Object other) => other is Digest && equals(other);

  @override
  int get hashCode => toString().hashCode;

  @override
  String toString() =>
      'sha256:${_bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join()}';

  /// The text form.
  String toJson() => toString();
}

/// A member of a tree as the digest sees it.
sealed class TreeEntry {
  const TreeEntry();
}

/// A regular file and its bytes.
final class FileEntry extends TreeEntry {
  /// Creates the entry.
  const FileEntry(this.bytes);

  /// The file's bytes.
  final Uint8List bytes;
}

/// A directory.
final class DirectoryEntry extends TreeEntry {
  /// Creates the entry.
  const DirectoryEntry();
}

/// A symbolic link.
final class SymlinkEntry extends TreeEntry {
  /// Creates the entry.
  const SymlinkEntry();
}

/// Anything else: a device, a socket, a pipe.
final class SpecialEntry extends TreeEntry {
  /// Creates the entry.
  const SpecialEntry();
}

/// An overlay's digest and each member's.
class TreeDigest {
  /// Creates the result.
  const TreeDigest(this.digest, this.files);

  /// The overlay's digest.
  final Digest digest;

  /// Member path to digest, in byte order of the path.
  final Map<String, Digest> files;

  /// The bytes that were hashed: per member `path NUL digest LF`.
  List<Uint8List> lines() => [
    for (final entry in files.entries) _lineOf(entry.key, entry.value),
  ];
}

const Utf8Decoder _lossy = Utf8Decoder(allowMalformed: true);

AosError _refused(String cls, String subject) => AosError(cls, subject);

String _shown(List<Uint8List> components) =>
    components.map(_lossy.convert).join('/');

Uint8List _lineOf(String path, Digest digest) {
  final text = encodeUtf8(path);
  final hash = encodeUtf8(digest.toString());
  final line = Uint8List(text.length + 1 + hash.length + 1);
  line.setRange(0, text.length, text);
  line.setRange(text.length + 1, text.length + 1 + hash.length, hash);
  line[line.length - 1] = 0x0a;
  return line;
}

TreeDigest _finish(Map<String, Digest> found) {
  final paths = found.keys.toList()..sort(compareText);
  final files = {for (final path in paths) path: found[path]!};
  final result = TreeDigest(Digest.of(Uint8List(0)), files);
  final text = concatBytes(result.lines());
  return TreeDigest(Digest.of(text), files);
}

final List<Uint8List> _excludedBytes = excludedComponents
    .map(encodeUtf8)
    .toList();

bool _sameBytes(Uint8List a, Uint8List b) {
  if (a.length != b.length) return false;
  for (var at = 0; at < a.length; at += 1) {
    if (a[at] != b[at]) return false;
  }
  return true;
}

bool _excluded(List<Uint8List> components, bool directory) {
  if (components.any(
    (component) => _excludedBytes.any((name) => _sameBytes(name, component)),
  )) {
    return true;
  }
  if (components.isEmpty) return false;
  final last = components.last;
  return !directory &&
      last.length >= 4 &&
      _lossy.convert(Uint8List.sublistView(last, last.length - 4)) == '.pyc';
}

String _memberPath(List<Uint8List> components) {
  const strict = Utf8Decoder();
  final List<String> names;
  try {
    names = components.map(strict.convert).toList();
  } on FormatException {
    throw _refused('non-utf8-name', _shown(components));
  }
  final path = names.join('/');
  if (!validRelativePath(path)) {
    throw _refused('invalid-name', _shown(components));
  }
  return path;
}

void _checkCase(Map<String, Digest> files) {
  final folded = <String, String>{};
  for (final path in files.keys) {
    final key = asciiLower(path);
    final other = folded[key];
    folded[key] = path;
    if (other != null && other != path) throw _refused('case-collision', path);
  }
}

/// The digest of entries given as path components (a `Uint8List` or a `String` each) and what
/// each is, in any order. A directory and an excluded name are skipped; a symlink is `symlink`,
/// anything else but a file `special-file`; a path twice is `duplicate-member`.
TreeDigest digestEntries(Iterable<(List<Object>, TreeEntry)> entries) {
  final files = <String, Digest>{};
  for (final (given, entry) in entries) {
    final components = [
      for (final part in given)
        part is String ? encodeUtf8(part) : part as Uint8List,
    ];
    final directory = entry is DirectoryEntry;
    if (directory || _excluded(components, directory)) continue;
    if (entry is SymlinkEntry) throw _refused('symlink', _shown(components));
    if (entry is! FileEntry) throw _refused('special-file', _shown(components));
    final path = _memberPath(components);
    if (files.containsKey(path)) throw _refused('duplicate-member', path);
    files[path] = Digest.of(entry.bytes);
  }
  _checkCase(files);
  return _finish(files);
}

/// Refuses entries past the tree limits before any is hashed: for trees a caller hands over
/// rather than the library walks.
void checkEntries(List<(Object?, TreeEntry)> entries, Settings settings) {
  if (!allows(settings.treeFiles, entries.length)) {
    throw AosError(
      'too-large',
      'more than ${limitText(settings.treeFiles)} files and directories',
    ).over('tree_files');
  }
  var total = 0;
  for (final (_, entry) in entries) {
    if (entry is FileEntry) {
      if (!allows(settings.fileBytes, entry.bytes.length)) {
        throw AosError(
          'too-large',
          'a file larger than ${limitText(settings.fileBytes)} bytes',
        ).over('file_bytes');
      }
      total += entry.bytes.length;
    }
  }
  if (!allows(settings.treeBytes, total)) {
    throw AosError(
      'too-large',
      'more than ${limitText(settings.treeBytes)} bytes',
    ).over('tree_bytes');
  }
}

/// The digest of a directory read through [files]. Nothing is followed: a link is refused.
Future<TreeDigest> treeDigest(
  FileSystem files,
  String root,
  Settings settings,
) async => (await _walkTree(files, root, settings, false)).digest;

/// Every member's bytes and the digest of one walk.
Future<({OverlayTree tree, TreeDigest digest})> readTree(
  FileSystem files,
  String root,
  Settings settings,
) async {
  final walked = await _walkTree(files, root, settings, true);
  return (
    tree: OverlayTree.fromBytes([
      for (final entry in walked.contents.entries) (entry.key, entry.value),
    ]),
    digest: walked.digest,
  );
}

AosError _ioError(Object error, [String prefix = '']) {
  final message = error is AosError ? error.message : error.toString();
  return _refused('io', '$prefix$message');
}

Future<({TreeDigest digest, Map<String, Uint8List> contents})> _walkTree(
  FileSystem files,
  String root,
  Settings settings,
  bool keep,
) async {
  final Metadata metadata;
  try {
    metadata = await files.metadata(root);
  } on Object catch (error) {
    throw _ioError(error);
  }
  if (metadata.kind != FileKind.directory) {
    throw _refused(
      'io',
      'the root is not a directory (a link is never followed)',
    );
  }
  final walk = _Walk(files, settings, keep);
  await walk.directory(root, [], 1);
  _checkCase(walk.found);
  return (digest: _finish(walk.found), contents: walk.contents);
}

class _Walk {
  _Walk(this.files, this.settings, this.keep);

  final Map<String, Digest> found = {};
  final Map<String, Uint8List> contents = {};
  int _bytes = 0;
  int _entries = 0;
  final FileSystem files;
  final Settings settings;
  final bool keep;

  Future<void> directory(
    String path,
    List<Uint8List> components,
    int depth,
  ) async {
    if (!allows(settings.treeDepth, depth)) {
      throw _refused(
        'too-large',
        'directories nested deeper than ${limitText(settings.treeDepth)}',
      ).over('tree_depth');
    }
    final List<Object> listed;
    try {
      listed = await files.list(path);
    } on Object catch (error) {
      throw _ioError(error);
    }
    final names = [
      for (final name in listed)
        (
          name: name,
          bytes: name is String ? encodeUtf8(name) : Uint8List.fromList([0xff]),
        ),
    ]..sort((a, b) => _compareBytes(a.bytes, b.bytes));
    for (final (:name, :bytes) in names) {
      if (bytes.isEmpty ||
          (bytes.length == 1 && bytes[0] == 0x2e) ||
          (bytes.length == 2 && bytes[0] == 0x2e && bytes[1] == 0x2e) ||
          bytes.any((byte) => byte == 0x2f || byte == 0x5c || byte == 0)) {
        throw _refused('invalid-name', _shown([bytes]));
      }
      if (name is! String) {
        throw _refused('non-utf8-name', _shown([...components, bytes]));
      }
      components.add(bytes);
      try {
        await _entry(joinPath(path, name), components, depth);
      } finally {
        components.removeLast();
      }
    }
  }

  void _count() {
    _entries += 1;
    if (!allows(settings.treeFiles, _entries)) {
      throw _refused(
        'too-large',
        'more than ${limitText(settings.treeFiles)} files and directories',
      ).over('tree_files');
    }
  }

  Future<void> _entry(
    String path,
    List<Uint8List> components,
    int depth,
  ) async {
    final Metadata metadata;
    try {
      metadata = await files.metadata(path);
    } on Object catch (error) {
      throw _ioError(error);
    }
    final isDirectory = metadata.kind == FileKind.directory;
    if (_excluded(components, isDirectory)) return;
    if (metadata.kind == FileKind.symlink) {
      throw _refused('symlink', _shown(components));
    }
    if (isDirectory) {
      _count();
      await directory(path, components, depth + 1);
      return;
    }
    if (metadata.kind != FileKind.file) {
      throw _refused('special-file', _shown(components));
    }
    final member = _memberPath(components);
    _count();
    final fileBytes = settings.fileBytes;
    final most = fileBytes == null ? 9007199254740991 : fileBytes + 1;
    final Uint8List bytes;
    try {
      bytes = await files.read(path, most);
    } on Object catch (error) {
      throw _ioError(error, '$member: ');
    }
    if (!allows(settings.fileBytes, bytes.length)) {
      throw _refused(
        'too-large',
        '$member is larger than ${limitText(settings.fileBytes)} bytes',
      ).over('file_bytes');
    }
    _bytes += bytes.length;
    if (!allows(settings.treeBytes, _bytes)) {
      throw _refused(
        'too-large',
        'more than ${limitText(settings.treeBytes)} bytes',
      ).over('tree_bytes');
    }
    found[member] = Digest.of(bytes);
    if (keep) contents[member] = bytes;
  }
}

int _compareBytes(Uint8List a, Uint8List b) {
  final common = a.length < b.length ? a.length : b.length;
  for (var at = 0; at < common; at += 1) {
    final difference = a[at] - b[at];
    if (difference != 0) return difference;
  }
  return a.length - b.length;
}
