import 'dart:io';

/// Writes [text] to the person's Downloads folder (their home when it has none), never over an
/// existing file: `name`, then `name-1`, `name-2` before the extension.
Future<String?> saveText(String name, String text) async {
  final home =
      Platform.environment['USERPROFILE'] ?? Platform.environment['HOME'];
  if (home == null || home.isEmpty) {
    throw StateError('There is no home folder to save into.');
  }
  final separator = Platform.pathSeparator;
  var folder = Directory('$home${separator}Downloads');
  if (!folder.existsSync()) folder = Directory(home);
  final dot = name.lastIndexOf('.');
  final stem = dot > 0 ? name.substring(0, dot) : name;
  final extension = dot > 0 ? name.substring(dot) : '';
  for (var n = 0; ; n++) {
    final file = File(
      '${folder.path}$separator${n == 0 ? name : '$stem-$n$extension'}',
    );
    if (file.existsSync()) continue;
    try {
      // Exclusive: a file that appeared meanwhile is refused, never written over.
      await file.create(exclusive: true);
    } on FileSystemException {
      if (n > 999) rethrow;
      continue;
    }
    await file.writeAsString(text, flush: true);
    return file.path;
  }
}
