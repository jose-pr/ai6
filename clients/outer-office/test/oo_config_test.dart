import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart'
    show MaterialApp, Scaffold, TextButton, TextField;
import 'package:outer_office/main.dart';
import 'package:outer_office/settings/connection_settings.dart';
import 'package:outer_office/settings/oo_config.dart';
import 'package:outer_office/settings/oo_config_file.dart';
import 'package:outer_office/settings/oo_config_file_io.dart'
    show caFilePath, configPathFor, trustCaFile;
import 'package:shared_preferences/shared_preferences.dart';

/// The same settings in each format ai6.cfg takes.
const _documents = {
  'toml':
      '[unex]\nurl = "https://unex.example"\n[oo.appearance]\ntheme = "red-file"\n'
      '[oo.assets]\ncdn = "fonts"\n',
  'json':
      '#!json\n{"unex": {"url": "https://unex.example"}, '
      '"oo": {"appearance": {"theme": "red-file"}, "assets": {"cdn": "all"}}}\n',
  'yaml':
      '#!/usr/bin/env yaml\nunex:\n  url: https://unex.example\n'
      'oo:\n  appearance:\n    theme: red-file\n',
  'yml': '#!yml\noo:\n  appearance:\n    theme: red-file\nunex:\n  url: https://unex.example\n',
  '%YAML': '%YAML 1.2\n---\noo:\n  appearance:\n    theme: red-file\nunex:\n  url: https://unex.example\n',
};

/// A value a person could keep in a file, and what no message may repeat.
const _planted = 'pl4nted-s3cret-value';

/// A whole file, as a host running a Station and a gateway keeps it, with [_planted] in every other
/// program's secret.
const _whole = '''
# A host's one file.
[provider]
location = "postgres://ai6@db/ai6?sslrootcert=/etc/ai6/db-ca.pem"
password = "pl4nted-s3cret-value"
poll = 5

[registry]
password = { file = "db-password" }

[registry.limits.sessions]
records = 100000

[unex]
url = "https://ai6.example"
ca_file = "enterprise-ca.pem"
station_token = "pl4nted-s3cret-value"
listen = "127.0.0.1:7082"
public_origin = "https://ai6.example"
proxy_secret = { env = "PROXY_SECRET" }
state_dir = "/var/lib/unex"

[unex.plugins]
paths = ["/opt/ai6/lib/unex"]

[unex.access]
chief = ["account:root"]

[unex.pages]
dir = "/var/lib/unex/pages"

[station]
id = "hq-1"
listen = "127.0.0.1:7081"

[station.plugins]
paths = ["/opt/ai6/lib/station"]

[station.identity]
dir = "/var/lib/ai6"

[station.secrets]
dir = "secrets"

[oo.appearance]
theme = "red-file"

[oo.chat]
width = 480
''';

MissionBoard _board(OoConfig config) => MissionBoard(
  apiOrigin: config.unexUrl ?? Uri.parse('http://127.0.0.1:7082'),
  pollInterval: null,
  themeId: config.defaultTheme(),
);

void main() {
  test('the chat reconnects by the settings in [oo.chat]', () {
    const defaults = ChatSettings();
    expect(defaults.retryDelay, const Duration(seconds: 1));
    expect(defaults.retryMaxDelay, const Duration(seconds: 16));
    final chat = OoConfig.parse(
      '[oo.chat]\nretry_delay = 0.5\nretry_max_delay = 4\n',
    ).chat;
    expect(chat.retryDelay, const Duration(milliseconds: 500));
    expect(chat.retryMaxDelay, const Duration(seconds: 4));
    expect(
      () => OoConfig.parse('[oo.chat]\nretry_delay = 0\n'),
      throwsFormatException,
    );
  });

  test('renewing the sign-in has its settings, and 0 turns it off', () {
    const none = OoConfig();
    expect(none.limits.refresh.renewBefore, const Duration(seconds: 600));
    expect(none.limits.refresh.renewRetry, const Duration(seconds: 30));
    final set = OoConfig.parse(
      '[oo.refresh]\nrenew_before = 0\nrenew_retry = 7\n',
    );
    expect(set.limits.refresh.renewBefore, isNull);
    expect(set.limits.refresh.renewRetry, const Duration(seconds: 7));
    expect(
      () => OoConfig.parse('[oo.refresh]\nrenew_retry = 0\n'),
      throwsFormatException,
    );
  });

  test(
    'the settings of the queue, states and proposal cards default as before',
    () {
      const none = OoConfig();
      expect(none.limits.refresh.slow, const Duration(seconds: 15));
      expect(none.limits.refresh.silentIntervals, 3);
      expect(none.limits.controls.confirming, const Duration(seconds: 30));
      expect(none.limits.pages.summary, 5);
      final set = OoConfig.parse(
        '[oo.refresh]\nslow = 0\nsilent_intervals = 5\n[oo.controls]\nconfirming = 12\n'
        '[oo.pages]\nsummary = 8\n',
      );
      expect(set.limits.refresh.slow, isNull);
      expect(set.limits.refresh.silentIntervals, 5);
      expect(set.limits.controls.confirming, const Duration(seconds: 12));
      expect(set.limits.pages.summary, 8);
      expect(
        () => OoConfig.parse('[oo.controls]\nconfirming = 0\n'),
        throwsFormatException,
      );
      expect(
        () => OoConfig.parse('[oo.refresh]\nsilent_intervals = 0\n'),
        throwsFormatException,
      );
    },
  );

  test('[oo.chat] sets Moneypenny\'s chat sizes; each defaults on its own', () {
    final config = OoConfig.parse(
      '[oo.chat]\nmin_width = 280\nmax_width = 900\nwidth = 500\nwidth_step = 8\n'
      'min_page_width = 300\ntext_width = 1100\nwindow_width = 600\n'
      'window_height = 800\ntransition_ms = 0\n',
    );
    final chat = config.chat;
    expect(
      [
        chat.minWidth,
        chat.maxWidth,
        chat.width,
        chat.widthStep,
        chat.minPageWidth,
        chat.textWidth,
        chat.windowWidth,
        chat.windowHeight,
      ],
      [280, 900, 500, 8, 300, 1100, 600, 800],
    );
    expect(chat.transitionMs, 0);
    final partial = OoConfig.parse('#!json\n{"oo": {"chat": {"width": 640}}}')
        .chat;
    expect(partial.width, 640);
    expect(partial.maxWidth, const ChatSettings().maxWidth);
    expect(OoConfig.none.chat.width, 400);
    // The example file shows the defaults.
    final example = OoConfig.parse(
      File('../../examples/config/ai6.cfg').readAsStringSync(),
    ).chat;
    const defaults = ChatSettings();
    expect(
      [
        example.minWidth,
        example.maxWidth,
        example.width,
        example.widthStep,
        example.minPageWidth,
        example.textWidth,
        example.windowWidth,
        example.windowHeight,
        example.transitionMs,
      ],
      [
        defaults.minWidth,
        defaults.maxWidth,
        defaults.width,
        defaults.widthStep,
        defaults.minPageWidth,
        defaults.textWidth,
        defaults.windowWidth,
        defaults.windowHeight,
        defaults.transitionMs,
      ],
    );
    for (final bad in [
      '[oo.chat]\nwidth = 900\n',
      '[oo.chat]\nmin_width = 800\nmax_width = 700\nwidth = 750\n',
      '[oo.chat]\nwidth = -1\n',
      '[oo.chat]\nwidth = "wide"\n',
      '[oo.chat]\ntransition_ms = 1.5\n',
      '[oo.chat]\nheight = 3\n',
    ]) {
      expect(() => OoConfig.parse(bad), throwsFormatException, reason: bad);
    }
  });

  test('each format names the same settings and [oo.assets] is left alone', () {
    for (final MapEntry(key: name, value: text) in _documents.entries) {
      final config = OoConfig.parse(text);
      expect(config.unexUrl, Uri.parse('https://unex.example'), reason: name);
      expect(config.theme, 'red-file', reason: name);
    }
  });

  test(
    '[oo.routes] text_bytes sets the longest route text, 128 bytes unset',
    () {
      expect(OoConfig.none.routeLimits.textBytes, 128);
      expect(
        OoConfig.parse('[oo.routes]\ntext_bytes = 512\n').routeLimits.textBytes,
        512,
      );
      expect(
        OoConfig.parse('#!json\n{"oo": {"routes": {"text_bytes": 64}}}\n')
            .routeLimits
            .textBytes,
        64,
      );
      for (final bad in ['0', '-1', '"128"', '1.5']) {
        expect(
          () => OoConfig.parse('[oo.routes]\ntext_bytes = $bad\n'),
          throwsFormatException,
          reason: bad,
        );
      }
      expect(
        () => OoConfig.parse('[oo.routes]\nlength = 1\n'),
        throwsFormatException,
      );
    },
  );

  test('a document is read as the format it names, never guessed', () {
    expect(declaredFormat('{"unex": {}}'), OoConfigFormat.toml);
    expect(() => OoConfig.parse('{"unex": {}}'), throwsFormatException);
    expect(declaredFormat('#!.TOML\n'), OoConfigFormat.toml);
    OoConfig unknown() => OoConfig.parse('#!ini\n[unex]\n');
    expect(
      unknown,
      throwsA(
        isA<FormatException>().having(
          (error) => error.message,
          'message',
          allOf(contains('json'), contains('toml'), contains('yaml')),
        ),
      ),
    );
    expect(
      () => OoConfig.parse('[oo.chat]\nport = 1\n'),
      throwsFormatException,
    );
    expect(() => OoConfig.parse('[oo.windows]\n'), throwsFormatException);
    expect(() => OoConfig.parse('[windows]\n'), throwsFormatException);
    // anycfg answers only to a format's name and fixed extensions, so cfg is no format.
    expect(() => declaredFormat('#!cfg\n'), throwsFormatException);
    expect(
      () => OoConfig.parse('[unex]\nurl = "not an origin"\n'),
      throwsFormatException,
    );
    expect(OoConfig.parse('').unexUrl, isNull);
  });

  testWidgets(
    'ai6.cfg sets the default theme in every format and oo.theme wins over it',
    (tester) async {
      for (final text in _documents.values) {
        SharedPreferences.setMockInitialValues({});
        final board = _board(OoConfig.parse(text));
        addTearDown(board.dispose);
        await AppearancePreferences(SharedPreferences.getInstance())
            .restore(board);
        expect(board.themeId, 'red-file');
        expect(board.apiOrigin, Uri.parse('https://unex.example'));

        SharedPreferences.setMockInitialValues({'oo.theme': 'dusk'});
        final chosen = _board(OoConfig.parse(text));
        addTearDown(chosen.dispose);
        await AppearancePreferences(SharedPreferences.getInstance())
            .restore(chosen);
        expect(chosen.themeId, 'dusk');
      }
      final warnings = <String>[];
      expect(
        const OoConfig(theme: 'paisley').defaultTheme(warn: warnings.add),
        'night-ops',
      );
      expect(warnings.single, contains('oo.appearance.theme'));
      expect(warnings.single, isNot(contains('paisley')));
    },
  );

  test(
    'the address is written back in the declared format with its #! line',
    () {
      final url = Uri.parse('https://new.example');
      for (final MapEntry(key: name, value: text) in _documents.entries) {
        final written = withUnexUrl(text, url);
        final firstLine = text.split('\n').first;
        if (firstLine.startsWith('#!')) {
          expect(written.split('\n').first, firstLine, reason: name);
        }
        final config = OoConfig.parse(written);
        expect(config.unexUrl, url, reason: name);
        expect(config.theme, 'red-file', reason: name);
      }
      expect(OoConfig.parse(withUnexUrl('', url)).unexUrl, url);
      final yaml = withUnexUrl(
        '#!yaml\n# keep me\nunex:\n  url: https://a.example\n',
        url,
      );
      expect(yaml, contains('# keep me'));
    },
  );

  testWidgets('the settings view saves the address back to ai6.cfg', (
    tester,
  ) async {
    var file = '#!json\n{"oo": {"appearance": {"theme": "dusk"}}}\n';
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ConnectionSettings(
            read: () => file,
            write: (text) async => file = text,
            path: 'ai6.cfg',
          ),
        ),
      ),
    );
    await tester.enterText(find.byType(TextField), 'https://unex.example');
    await tester.tap(find.widgetWithText(TextButton, 'Save address'));
    await tester.pumpAndSettle();
    expect(file.split('\n').first, '#!json');
    expect(OoConfig.parse(file).unexUrl, Uri.parse('https://unex.example'));
    expect(OoConfig.parse(file).theme, 'dusk');
    expect(find.textContaining('next start'), findsOneWidget);
  });

  testWidgets('the web reads ai6.cfg beside the page and stored choices win', (
    tester,
  ) async {
    final page = Uri.parse('https://oo.example/app/index.html');
    final served = MockClient((request) async {
      expect(request.url, Uri.parse('https://oo.example/app/ai6.cfg'));
      return http.Response(_documents['json']!, 200);
    });
    final config = await loadWebConfig(page, client: served);
    expect(config.theme, 'red-file');
    SharedPreferences.setMockInitialValues({'oo.theme': 'classic'});
    final board = _board(config);
    addTearDown(board.dispose);
    await AppearancePreferences(SharedPreferences.getInstance()).restore(board);
    expect(board.themeId, 'classic');

    final missing = MockClient((_) async => http.Response('', 404));
    expect((await loadWebConfig(page, client: missing)).theme, isNull);
    final broken = MockClient((_) async => http.Response('#!ini\n', 200));
    expect((await loadWebConfig(page, client: broken)).theme, isNull);
  });

  test('the desktop file is found as the Rust programs find it', () {
    expect(
      configPathFor({'AI6_CONFIG': '/x/named.cfg', 'HOME': '/h'}, 'linux'),
      '/x/named.cfg',
    );
    expect(
      configPathFor({'APPDATA': r'D:\Profiles\a\Roaming'}, 'windows'),
      r'D:\Profiles\a\Roaming\ai6\ai6.cfg',
    );
    expect(
      configPathFor({'HOME': '/Users/a'}, 'macos'),
      '/Users/a/.config/ai6/ai6.cfg',
    );
    expect(
      configPathFor({'XDG_CONFIG_HOME': '/x', 'HOME': '/h'}, 'linux'),
      '/x/ai6/ai6.cfg',
    );
    expect(configPathFor({'HOME': '/h'}, 'linux'), '/h/.config/ai6/ai6.cfg');
    expect(configPathFor({}, 'linux'), isNull);
    // The first that exists; the system's when the user has none.
    expect(
      configPathFor(
        {'HOME': '/h'},
        'linux',
        exists: (path) => path == '/etc/ai6/ai6.cfg',
      ),
      '/etc/ai6/ai6.cfg',
    );
    expect(
      configPathFor(
        {'USERPROFILE': r'D:\Profiles\a', 'ProgramData': r'C:\ProgramData'},
        'windows',
        exists: (path) => path == r'C:\ProgramData\ai6\ai6.cfg',
      ),
      r'C:\ProgramData\ai6\ai6.cfg',
    );
    expect(
      configPathFor({'HOME': '/h'}, 'linux', exists: (path) => true),
      '/h/.config/ai6/ai6.cfg',
    );
  });

  test('a whole ai6.cfg gives Outer Office only its own, and asks nothing of the rest', () {
    final config = OoConfig.parse(_whole);
    expect(config.unexUrl, Uri.parse('https://ai6.example'));
    expect(config.caFile, 'enterprise-ca.pem');
    expect(config.theme, 'red-file');
    expect(config.chat.width, 480);
    // A key of another program's section is not this program's to judge.
    OoConfig.parse('$_whole\n[station.anything]\nat = "all"\n');
    expect(
      () => OoConfig.parse('$_whole\n[nobody]\nat = "all"\n'),
      throwsA(
        isA<FormatException>().having(
          (error) => error.message,
          'message',
          contains('[nobody]'),
        ),
      ),
    );
    expect(
      () => OoConfig.parse('oo = "$_planted"\n'),
      throwsA(
        isA<FormatException>().having(
          (error) => error.toString(),
          'message',
          allOf(contains('[oo]'), isNot(contains(_planted))),
        ),
      ),
    );
  });

  test('a refused file is named by key or position and is never quoted', () {
    for (final (document, path, names) in [
      ('[provider]\npassword = "$_planted\n', 'ai6.cfg', 'line '),
      ('#!json\n{"unex": {"url": "$_planted}\n', 'ai6.cfg', 'json'),
      ('{"unex": $_planted}', 'ai6.json', 'line 1'),
      ('#!yaml\nunex: "$_planted\n', 'ai6.cfg', 'yaml'),
      ('unex: [$_planted\n', 'ai6.yaml', 'yaml'),
      ('[unex]\nurl = "$_planted"\n', 'ai6.cfg', 'unex.url'),
      (
        '[oo.appearance]\ntheme = 1\nnote = "$_planted"\n',
        'ai6.cfg',
        'oo.appearance.note',
      ),
      ('[$_planted]\nx = 1\n', 'ai6.toml', ''),
    ]) {
      Object? caught;
      try {
        OoConfig.parse(document, path: path);
      } on Object catch (error) {
        caught = error;
      }
      expect(caught, isA<FormatException>(), reason: document);
      final shown = caught.toString();
      if (!document.startsWith('[$_planted]')) {
        expect(shown, isNot(contains(_planted)), reason: document);
      }
      expect(shown, contains(names), reason: document);
    }
  });

  test(
    'a file\'s extension fixes its format, as the Rust programs read it',
    () {
      const json = '{"oo": {"appearance": {"theme": "red-file"}}}';
      expect(OoConfig.parse(json, path: '/x/ai6.json').theme, 'red-file');
      expect(
        OoConfig.parse(
          'oo:\n  appearance:\n    theme: red-file\n',
          path: 'ai6.yml',
        ).theme,
        'red-file',
      );
      expect(
        () => OoConfig.parse(json, path: 'ai6.cfg'),
        throwsFormatException,
      );
      expect(
        () => OoConfig.parse('#!yaml\n$json', path: 'ai6.json'),
        throwsFormatException,
        reason: 'a fixed extension does not read a first line',
      );
      expect(() => OoConfig.parse('', path: 'ai6.ini'), throwsFormatException);
      expect(() => OoConfig.parse('', path: 'ai6'), throwsFormatException);
    },
  );

  test(
    'saving the address keeps every other program\'s section and comment',
    () {
      final url = Uri.parse('https://new.example');
      final written = withUnexUrl(_whole, url);
      expect(OoConfig.parse(written).unexUrl, url);
      expect(
        written
            .split('\n')
            .where((line) => !line.startsWith('url = '))
            .toList(),
        _whole.split('\n').where((line) => !line.startsWith('url = ')).toList(),
        reason: 'only the url line changes, comments included',
      );
      final added = withUnexUrl(
        '# mine\n[unex]\nlisten = "127.0.0.1:1"\n',
        url,
      );
      expect(
        added,
        '# mine\n[unex]\nurl = "https://new.example"\nlisten = "127.0.0.1:1"\n',
      );
      final appended = withUnexUrl('# mine\n[station]\nid = "a"\n', url);
      expect(decodeOoConfig(appended)['station'], {'id': 'a'});
      expect(OoConfig.parse(appended).unexUrl, url);
      expect(appended, startsWith('# mine\n'));
      final crlf = withUnexUrl('[unex]\r\nurl = "https://a.example"\r\n', url);
      expect(crlf, '[unex]\r\nurl = "https://new.example"\r\n');
      // A spelling a line edit cannot change is refused, not rewritten.
      expect(
        () => withUnexUrl('unex.url = "https://a.example"\n', url),
        throwsFormatException,
      );
      expect(
        OoConfig.parse(
          withUnexUrl(
            '{"unex": {"url": "https://a.example"}, "station": {"id": "a"}}',
            url,
            path: 'ai6.json',
          ),
          path: 'ai6.json',
        ).unexUrl,
        url,
      );
    },
  );

  test('ca_file is trusted beside the file that names it, and a bad one is refused', () {
    final pem = File('test/fixtures/test_ca.pem');
    expect(pem.existsSync(), isTrue);
    expect(
      caFilePath('ca.pem', '/etc/ai6/ai6.cfg'),
      endsWith('ai6${Platform.pathSeparator}ca.pem'),
    );
    expect(
      caFilePath(pem.absolute.path, '/etc/ai6/ai6.cfg'),
      pem.absolute.path,
    );
    final context = SecurityContext(withTrustedRoots: false);
    trustCaFile('test_ca.pem', 'test/fixtures/ai6.cfg', context: context);
    expect(
      () =>
          trustCaFile('no_such.pem', 'test/fixtures/ai6.cfg', context: context),
      throwsA(anything),
    );
  });

  test(
    'the public file ai6 config public writes is read as Outer Office\'s own',
    () {
      final config = OoConfig.parse(
        File('test/fixtures/public_ai6.cfg').readAsStringSync(),
      );
      expect(
        config.unexUrl,
        Uri.parse('https://ai6.example'),
        reason: 'the page reaches the gateway at its public origin',
      );
      expect(config.theme, 'night-ops');
      expect(
        config.caFile,
        isNull,
        reason: 'a host file name is not the public\'s business',
      );
      expect(config.limits.refresh.sessions, const Duration(seconds: 3));
      final bootstrap = File('web/flutter_bootstrap.js').readAsStringSync();
      expect(bootstrap, contains("fetch('ai6.cfg'"));
      expect(bootstrap, contains("table === 'oo.assets'"));
      expect(
        File('test/fixtures/public_ai6.cfg').readAsStringSync(),
        contains('[oo.assets]'),
        reason: 'the page finds the key the way the file spells it',
      );
    },
  );

  test('a web build bundles no host\'s file', () {
    expect(
      Directory('web')
          .listSync(recursive: true)
          .whereType<File>()
          .where(
            (file) =>
                RegExp(r'\.cfg$', caseSensitive: false).hasMatch(file.path),
          ),
      isEmpty,
    );
    expect(
      File('pubspec.yaml').readAsStringSync(),
      isNot(contains('ai6.cfg\n')),
    );
  });
}
