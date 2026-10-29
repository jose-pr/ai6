import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// Hosts the web app must not reach unless a deployment opts in.
const cdnHosts = [
  'gstatic.com',
  'googleapis.com',
  'unpkg.com',
  'jsdelivr.net',
  'cdnjs.cloudflare.com',
  'cloudflare.com',
  'googleusercontent.com',
];

/// The engine origins by `[assets] cdn` value, as `web/flutter_bootstrap.js` declares them.
Map<String, dynamic> origins(String bootstrap) {
  final match = RegExp(
    r'const ooOrigins = (\{.*?\n\});',
    dotAll: true,
  ).firstMatch(bootstrap);
  expect(match, isNotNull, reason: 'flutter_bootstrap.js declares ooOrigins');
  return jsonDecode(match!.group(1)!) as Map<String, dynamic>;
}

bool local(Object? url) =>
    url is String && !url.contains('//') && !url.contains(':');

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final bootstrap = File('web/flutter_bootstrap.js').readAsStringSync();

  test('by default the web engine loads only from its own origin', () {
    final byValue = origins(bootstrap);
    expect(byValue.keys, ['none', 'fonts', 'all']);
    final none = byValue['none'] as Map<String, dynamic>;
    expect(local(none['canvasKitBaseUrl']), isTrue);
    expect(local(none['fontFallbackBaseUrl']), isTrue);
    final fonts = byValue['fonts'] as Map<String, dynamic>;
    expect(local(fonts['canvasKitBaseUrl']), isTrue);
    expect(fonts.containsKey('fontFallbackBaseUrl'), isFalse);
    expect(byValue['all'], isEmpty);
    // Every path that cannot read a valid value falls back to none.
    expect(
      RegExp(r"return 'none';").allMatches(bootstrap).length,
      greaterThanOrEqualTo(3),
    );
    expect(bootstrap, contains('config: ooOrigins[cdn]'));
  });

  test('the page and its web files name no CDN host', () {
    for (final file in Directory('web').listSync(recursive: true)) {
      if (file is! File || !RegExp(r'\.(html|js|json)$').hasMatch(file.path)) {
        continue;
      }
      final text = file.readAsStringSync();
      for (final host in cdnHosts) {
        expect(text, isNot(contains(host)), reason: '${file.path}: $host');
      }
    }
    final index = File('web/index.html').readAsStringSync();
    expect(
      RegExp(r'''(src|href)\s*=\s*["'](https?:)?//''').hasMatch(index),
      isFalse,
      reason: 'index.html loads nothing from another origin',
    );
  });

  test('the engine finds its default font in the bundle', () async {
    final manifest = jsonDecode(
      await rootBundle.loadString('FontManifest.json'),
    ) as List<dynamic>;
    final roboto = manifest.cast<Map<String, dynamic>>().singleWhere(
      (family) => family['family'] == 'Roboto',
    );
    expect(roboto['fonts'], hasLength(3));
    expect(
      await rootBundle.loadString('assets/fonts/LICENSE-Roboto.txt'),
      contains('Apache License'),
    );
    expect(
      File('pubspec.lock').readAsStringSync(),
      isNot(contains('google_fonts')),
    );
  });

  final built = File('build/web/flutter_bootstrap.js');
  test(
    'a web build serves the engine itself and keeps the default',
    () {
      final output = built.readAsStringSync();
      expect(origins(output), origins(bootstrap));
      expect(output, contains('config: ooOrigins[cdn]'));
      expect(File('build/web/canvaskit/canvaskit.wasm').existsSync(), isTrue);
      expect(File('build/web/canvaskit/skwasm.wasm').existsSync(), isTrue);
      final index = File('build/web/index.html').readAsStringSync();
      for (final host in cdnHosts) {
        expect(index, isNot(contains(host)), reason: host);
      }
    },
    skip: built.existsSync()
        ? false
        : 'no build/web: run flutter build web first',
  );
}
