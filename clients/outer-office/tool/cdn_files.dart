// Files the web app would otherwise fetch from a CDN, listed in `tool/cdn_files.json`.
//
//   dart run tool/cdn_files.dart fill [--deps DIR] [--web build/web]
//     Puts every file the web build serves itself (the Noto fallback fonts) into the build,
//     each checked against its SHA-256, and checks the build's own engine files. Run it
//     after `flutter build web`. With --deps, files come from DIR/cdn/<host>/<path> (a
//     checkout of the repository's `deps` branch); otherwise they are downloaded.
//   dart run tool/cdn_files.dart mirror --out DIR [--deps DIR]
//     Writes every listed file to DIR/<host>/<path>: the `cdn/` directory of `deps`.
//   dart run tool/cdn_files.dart update --sdk FLUTTER_SDK [--web build/web] [--extra FILE]
//     Rebuilds the list for the SDK's engine: its fallback fonts, the engine files of the
//     build, and the URLs in FILE (one per line), downloading each once to hash it.

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data' show BytesBuilder;

import 'package:crypto/crypto.dart';

const manifestPath = 'tool/cdn_files.json';

/// Where the default build serves the fallback fonts; `web/flutter_bootstrap.js` names the
/// same base.
const fallbackBase = 'assets/fonts/fallback/';

const fontsHost = 'fonts.gstatic.com';
const engineHost = 'www.gstatic.com';

Future<void> main(List<String> arguments) async {
  if (arguments.isEmpty) return usage();
  final options = <String, String>{};
  for (var i = 1; i < arguments.length; i += 2) {
    if (!arguments[i].startsWith('--') || i + 1 >= arguments.length) {
      return usage();
    }
    options[arguments[i].substring(2)] = arguments[i + 1];
  }
  try {
    switch (arguments.first) {
      case 'fill':
        await fill(web: options['web'] ?? 'build/web', deps: options['deps']);
      case 'mirror':
        final out = options['out'];
        if (out == null) return usage();
        await mirror(out: out, deps: options['deps']);
      case 'update':
        final sdk = options['sdk'];
        if (sdk == null) return usage();
        await update(
          sdk: sdk,
          web: options['web'] ?? 'build/web',
          extra: options['extra'],
        );
      default:
        return usage();
    }
  } on CdnFileError catch (error) {
    stderr.writeln(error.message);
    exitCode = 1;
  } finally {
    _client.close(force: true);
  }
}

void usage() {
  stderr.writeln(
    'usage: dart run tool/cdn_files.dart fill [--deps DIR] [--web DIR]\n'
    '       dart run tool/cdn_files.dart mirror --out DIR [--deps DIR]\n'
    '       dart run tool/cdn_files.dart update --sdk FLUTTER_SDK [--web DIR] '
    '[--extra FILE]',
  );
  exitCode = 64;
}

class CdnFileError implements Exception {
  CdnFileError(this.message);

  final String message;
}

class NotServed extends CdnFileError {
  NotServed(super.message);
}

/// One file a CDN serves: where it lives there, its digest and size, its licence, and
/// where the web build serves it (null when the build already holds it, or never needs it).
class CdnFile {
  CdnFile({
    required this.host,
    required this.path,
    required this.sha256,
    required this.bytes,
    required this.licence,
    this.serve,
  });

  factory CdnFile.fromJson(Map<String, dynamic> json) => CdnFile(
    host: json['host'] as String,
    path: json['path'] as String,
    sha256: json['sha256'] as String,
    bytes: json['bytes'] as int,
    licence: json['licence'] as String,
    serve: json['serve'] as String?,
  );

  final String host;
  final String path;
  final String sha256;
  final int bytes;
  final String licence;
  final String? serve;

  Uri get url => Uri.https(host, path);

  Map<String, dynamic> toJson() => {
    'host': host,
    'path': path,
    'sha256': sha256,
    'bytes': bytes,
    'licence': licence,
    'serve': ?serve,
  };
}

class Manifest {
  Manifest(this.engineRevision, this.files);

  factory Manifest.read() {
    final json = jsonDecode(
      File(manifestPath).readAsStringSync(),
    ) as Map<String, dynamic>;
    return Manifest(json['engine_revision'] as String, [
      for (final file in json['files'] as List<dynamic>)
        CdnFile.fromJson(file as Map<String, dynamic>),
    ]);
  }

  final String engineRevision;
  final List<CdnFile> files;

  void write() {
    final text = const JsonEncoder.withIndent('  ').convert({
      'engine_revision': engineRevision,
      'files': [for (final file in files) file.toJson()],
    });
    File(manifestPath).writeAsStringSync('$text\n');
  }
}

/// The engine revision a web build loads, from its `flutter_bootstrap.js`.
String buildEngineRevision(String web) {
  final bootstrap = File('$web/flutter_bootstrap.js');
  if (!bootstrap.existsSync()) {
    throw CdnFileError(
      'No $web/flutter_bootstrap.js: run flutter build web first.',
    );
  }
  final match = RegExp(r'"engineRevision":"([0-9a-f]{40})"')
      .firstMatch(bootstrap.readAsStringSync());
  if (match == null) {
    throw CdnFileError('$web/flutter_bootstrap.js names no engine revision.');
  }
  return match.group(1)!;
}

final _client = HttpClient()..connectionTimeout = const Duration(seconds: 30);

Future<List<int>> download(Uri url) async {
  for (var attempt = 1; ; attempt++) {
    try {
      final response = await (await _client.getUrl(url)).close();
      final body = await response.fold<BytesBuilder>(
        BytesBuilder(copy: false),
        (builder, chunk) => builder..add(chunk),
      );
      if (response.statusCode == 404) throw NotServed('$url answered 404.');
      if (response.statusCode != 200) {
        throw CdnFileError('$url answered ${response.statusCode}.');
      }
      return body.takeBytes();
    } on IOException catch (error) {
      if (attempt == 3) throw CdnFileError('$url could not be fetched: $error');
    }
  }
}

/// The file's bytes, from the deps checkout when given and holding it, else downloaded;
/// refused unless they match the listed digest.
Future<List<int>> obtain(CdnFile file, String? deps) async {
  final local = deps == null
      ? null
      : File('$deps/cdn/${file.host}${file.path}');
  final bytes = local != null && local.existsSync()
      ? await local.readAsBytes()
      : await download(file.url);
  final digest = sha256.convert(bytes).toString();
  if (digest != file.sha256) {
    throw CdnFileError(
      '${file.url}: SHA-256 $digest, the list says ${file.sha256}.',
    );
  }
  return bytes;
}

/// Runs [action] over [items], [width] at a time.
Future<void> pool<T>(
  List<T> items,
  Future<void> Function(T) action, {
  int width = 8,
}) async {
  var next = 0;
  Future<void> worker() async {
    while (next < items.length) {
      await action(items[next++]);
    }
  }

  await Future.wait([for (var i = 0; i < width; i++) worker()]);
}

Future<void> fill({required String web, String? deps}) async {
  final manifest = Manifest.read();
  final revision = buildEngineRevision(web);
  // Another engine still serves its own CanvasKit; only fonts the engine added since the
  // list was made would be missing, so the build goes on and says so.
  final sameEngine = revision == manifest.engineRevision;
  if (!sameEngine) {
    stderr.writeln(
      'warning: the build is engine $revision but $manifestPath lists '
      '${manifest.engineRevision}; run the update command for this SDK.',
    );
  }
  final engineFiles = sameEngine
      ? manifest.files.where((file) => file.host == engineHost).toList()
      : const <CdnFile>[];
  for (final file in engineFiles) {
    final relative = file.path.split('/$revision/').last;
    final built = File('$web/canvaskit/$relative');
    if (!built.existsSync()) {
      throw CdnFileError('$web/canvaskit/$relative is missing from the build.');
    }
    final digest = sha256.convert(await built.readAsBytes()).toString();
    if (digest != file.sha256) {
      throw CdnFileError(
        '$web/canvaskit/$relative differs from the engine on $engineHost.',
      );
    }
  }
  final served = manifest.files.where((file) => file.serve != null).toList();
  var bytes = 0;
  await pool(served, (file) async {
    final target = File('$web/${file.serve}');
    await target.parent.create(recursive: true);
    await target.writeAsBytes(await obtain(file, deps));
    bytes += file.bytes;
  });
  stdout.writeln(
    'Placed ${served.length} files (${(bytes / 1048576).toStringAsFixed(1)} MiB) '
    'under $web/$fallbackBase; ${engineFiles.length} engine files match.',
  );
}

Future<void> mirror({required String out, String? deps}) async {
  final manifest = Manifest.read();
  await pool(manifest.files, (file) async {
    final target = File('$out/${file.host}${file.path}');
    await target.parent.create(recursive: true);
    await target.writeAsBytes(await obtain(file, deps));
  });
  stdout.writeln('Mirrored ${manifest.files.length} files into $out.');
}

Future<void> update({
  required String sdk,
  required String web,
  String? extra,
}) async {
  final revision = buildEngineRevision(web);
  final source = File(
    '$sdk/engine/src/flutter/lib/web_ui/lib/src/engine/font_fallback_data.dart',
  );
  if (!source.existsSync()) {
    throw CdnFileError(
      '${source.path} is missing: --sdk names a Flutter checkout.',
    );
  }
  final fonts = RegExp(
    r"'([a-z0-9]+/v[0-9]+/[A-Za-z0-9_\-.]+\.(?:woff2|ttf|otf))'",
  ).allMatches(source.readAsStringSync()).map((match) => match.group(1)!);
  final wanted = <(String, String, String, String?)>[
    for (final font in {...fonts})
      (fontsHost, '/s/$font', 'OFL-1.1', '$fallbackBase$font'),
    for (final entity in Directory('$web/canvaskit').listSync(recursive: true))
      if (entity is File)
        (
          engineHost,
          '/flutter-canvaskit/$revision/'
              '${entity.path.substring('$web/canvaskit/'.length).replaceAll(r'\', '/')}',
          'BSD-3-Clause',
          null,
        ),
    if (extra != null)
      for (final line in File(extra).readAsLinesSync())
        if (line.trim().isNotEmpty)
          (
            Uri.parse(line.trim()).host,
            Uri.parse(line.trim()).path,
            'see host',
            null,
          ),
  ];
  final seen = <String>{};
  final skipped = <String>[];
  final files = <CdnFile>[];
  await pool(wanted, (entry) async {
    final (host, path, licence, serve) = entry;
    if (!seen.add('$host$path')) return;
    final List<int> bytes;
    try {
      bytes = await download(Uri.https(host, path));
    } on NotServed {
      // The build ships files the CDN does not serve (the engine's debug symbols).
      if (host == engineHost) {
        skipped.add(path);
        return;
      }
      rethrow;
    }
    files.add(
      CdnFile(
        host: host,
        path: path,
        sha256: sha256.convert(bytes).toString(),
        bytes: bytes.length,
        licence: licence,
        serve: serve,
      ),
    );
  });
  files.sort((a, b) => '${a.host}${a.path}'.compareTo('${b.host}${b.path}'));
  Manifest(revision, files).write();
  final total = files.fold(0, (sum, file) => sum + file.bytes);
  stdout.writeln(
    'Listed ${files.length} files, ${(total / 1048576).toStringAsFixed(1)} MiB, '
    'for engine $revision; not on $engineHost: ${skipped.join(', ')}.',
  );
}
