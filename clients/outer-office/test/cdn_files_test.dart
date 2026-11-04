import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';

import '../tool/cdn_files.dart' as cdn;

final locked = 'a' * 40;
final other = 'b' * 40;
const fontPath = '/s/notosans/v1/slice.woff2';

void main() {
  late Directory root;
  late String web;
  late String deps;
  late String manifest;
  final warnings = <String>[];
  final engineBytes = utf8.encode('the locked engine');
  final fontBytes = utf8.encode('a font');

  String digest(List<int> bytes) => sha256.convert(bytes).toString();

  Map<String, Object> engineFile(String revision, List<int> bytes) => {
    'host': cdn.engineHost,
    'path': '${cdn.enginePrefix(revision)}canvaskit.wasm',
    'sha256': digest(bytes),
    'bytes': bytes.length,
    'licence': 'BSD-3-Clause',
  };

  /// A build of engine [revision], and a list of [engines] in which each has its own
  /// `canvaskit.wasm` and the font serves them all.
  void lay({required String revision, required List<String> engines}) {
    File('$web/flutter_bootstrap.js')
      ..createSync(recursive: true)
      ..writeAsStringSync(
        '_flutter.buildConfig = {"engineRevision":"$revision"};',
      );
    File('$web/canvaskit/canvaskit.wasm')
      ..createSync(recursive: true)
      ..writeAsBytesSync(engineBytes);
    File('$deps/cdn/fonts.gstatic.com$fontPath')
      ..createSync(recursive: true)
      ..writeAsBytesSync(fontBytes);
    File(manifest).writeAsStringSync(
      jsonEncode({
        'engine_revisions': engines,
        'files': [
          {
            'host': cdn.fontsHost,
            'path': fontPath,
            'sha256': digest(fontBytes),
            'bytes': fontBytes.length,
            'licence': 'OFL-1.1',
            'serve': '${cdn.fallbackBase}${fontPath.substring(3)}',
          },
          for (final engine in engines)
            engineFile(
              engine,
              engine == locked ? engineBytes : utf8.encode('another engine'),
            ),
        ],
      }),
    );
  }

  Future<void> fill() => cdn.fill(
    web: web,
    deps: deps,
    manifestFile: manifest,
    warn: warnings.add,
  );

  File served() => File('$web/${cdn.fallbackBase}${fontPath.substring(3)}');

  setUp(() {
    root = Directory.systemTemp.createTempSync('cdn_files_test');
    web = '${root.path}/web';
    deps = '${root.path}/deps';
    manifest = '${root.path}/cdn_files.json';
    warnings.clear();
  });
  tearDown(() => root.deleteSync(recursive: true));

  test(
    'a build of one engine listed among several fills without a warning',
    () async {
      lay(revision: locked, engines: [locked, other]);
      await fill();
      expect(warnings, isEmpty);
      expect(served().readAsBytesSync(), fontBytes);
    },
  );

  test('the build is checked against its own engine\'s files', () async {
    lay(revision: locked, engines: [locked, other]);
    File('$web/canvaskit/canvaskit.wasm').writeAsStringSync('changed');
    await expectLater(fill(), throwsA(isA<cdn.CdnFileError>()));
  });

  test('a build of an engine not listed fills with one warning', () async {
    lay(revision: 'c' * 40, engines: [locked, other]);
    await fill();
    expect(warnings, hasLength(1));
    expect(warnings.single, contains('c' * 40));
    expect(served().readAsBytesSync(), fontBytes);
  });

  test('an engine listed with no files of its own is refused', () async {
    lay(revision: locked, engines: [locked]);
    final listed = jsonDecode(File(manifest).readAsStringSync()) as Map;
    (listed['files'] as List).removeLast();
    File(manifest).writeAsStringSync(jsonEncode(listed));
    await expectLater(fill(), throwsA(isA<cdn.CdnFileError>()));
  });

  test('a list naming one engine_revision is refused', () {
    File(
      manifest,
    ).writeAsStringSync(jsonEncode({'engine_revision': locked, 'files': []}));
    expect(() => cdn.Manifest.read(manifest), throwsA(isA<cdn.CdnFileError>()));
  });
}
