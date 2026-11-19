/// The `dart:io` filesystem for the VM: [IoFileSystem] and [currentOs]. Import it only where
/// `dart:io` exists; the rest of the package runs on the web as well.
library;

import 'dart:io';
import 'dart:typed_data';

import 'coqui_aos.dart';

/// Reads real files through `dart:io`.
///
/// [read] and [list] open through links, as the platform does: a link swapped in between
/// [metadata] and [read] is followed, so read only trees no one else can write.
class IoFileSystem implements FileSystem {
  /// Creates the filesystem.
  const IoFileSystem();

  @override
  Metadata metadata(String path) {
    final stat = FileStat.statSync(path);
    final type = FileSystemEntity.typeSync(path, followLinks: false);
    switch (type) {
      case FileSystemEntityType.link:
        return const Metadata(FileKind.symlink, 0);
      case FileSystemEntityType.file:
        return Metadata(FileKind.file, stat.size);
      case FileSystemEntityType.directory:
        return const Metadata(FileKind.directory, 0);
      case FileSystemEntityType.notFound:
        throw FileSystemException('no such file or directory', path);
      default:
        return const Metadata(FileKind.other, 0);
    }
  }

  @override
  Uint8List read(String path, int max) {
    final file = File(path).openSync();
    try {
      final length = file.lengthSync();
      return file.readSync(length < max ? length : max);
    } finally {
      file.closeSync();
    }
  }

  @override
  List<Object> list(String path) => [
    for (final entity in Directory(path).listSync(followLinks: false))
      entity.uri.pathSegments.lastWhere((segment) => segment.isNotEmpty),
  ];
}

/// The running operating system's name as the specification writes it: `windows`, `macos`,
/// `linux`, ... (`Platform.operatingSystem`, with `android` read as `linux`).
String currentOs() {
  switch (Platform.operatingSystem) {
    case 'android':
      return 'linux';
    default:
      return Platform.operatingSystem;
  }
}
