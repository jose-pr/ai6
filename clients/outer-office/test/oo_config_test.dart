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
    show configPathFor;
import 'package:shared_preferences/shared_preferences.dart';

/// The same settings in each format OO.cfg takes.
const _documents = {
  'toml':
      '[unex]\nurl = "https://unex.example"\n[appearance]\ntheme = "red-file"\n'
      '[assets]\ncdn = "fonts"\n',
  'json':
      '#!json\n{"unex": {"url": "https://unex.example"}, '
      '"appearance": {"theme": "red-file"}, "assets": {"cdn": "all"}}\n',
  'yaml':
      '#!/usr/bin/env yaml\nunex:\n  url: https://unex.example\n'
      'appearance:\n  theme: red-file\n',
  'yml': '#!yml\nappearance:\n  theme: red-file\nunex:\n  url: https://unex.example\n',
  '%YAML': '%YAML 1.2\n---\nappearance:\n  theme: red-file\nunex:\n  url: https://unex.example\n',
};

MissionBoard _board(OoConfig config) => MissionBoard(
  apiOrigin: config.unexUrl ?? Uri.parse('http://127.0.0.1:7082'),
  pollInterval: null,
  themeId: config.defaultTheme(),
);

void main() {
  test('each format names the same settings and [assets] is left alone', () {
    for (final MapEntry(key: name, value: text) in _documents.entries) {
      final config = OoConfig.parse(text);
      expect(config.unexUrl, Uri.parse('https://unex.example'), reason: name);
      expect(config.theme, 'red-file', reason: name);
    }
  });

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
    expect(() => OoConfig.parse('[unex]\nport = 1\n'), throwsFormatException);
    expect(() => OoConfig.parse('[windows]\n'), throwsFormatException);
    expect(
      () => OoConfig.parse('[unex]\nurl = "not an origin"\n'),
      throwsFormatException,
    );
    expect(OoConfig.parse('').unexUrl, isNull);
  });

  testWidgets(
    'OO.cfg sets the default theme in every format and oo.theme wins over it',
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
      expect(warnings.single, contains('paisley'));
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

  testWidgets('the settings view saves the address back to OO.cfg', (
    tester,
  ) async {
    var file = '#!json\n{"appearance": {"theme": "dusk"}}\n';
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ConnectionSettings(
            read: () => file,
            write: (text) async => file = text,
            path: 'OO.cfg',
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

  testWidgets('the web reads OO.cfg beside the page and stored choices win', (
    tester,
  ) async {
    final page = Uri.parse('https://oo.example/app/index.html');
    final served = MockClient((request) async {
      expect(request.url, Uri.parse('https://oo.example/app/OO.cfg'));
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

  test('the desktop file is OO_CONFIG, else OO/OO.cfg in the configuration directory', () {
    expect(
      configPathFor({'OO_CONFIG': '/etc/oo.cfg', 'HOME': '/h'}, 'linux'),
      '/etc/oo.cfg',
    );
    expect(
      configPathFor({'APPDATA': r'D:\Profiles\a\Roaming'}, 'windows'),
      r'D:\Profiles\a\Roaming\OO\OO.cfg',
    );
    expect(
      configPathFor({'HOME': '/Users/a'}, 'macos'),
      '/Users/a/Library/Application Support/OO/OO.cfg',
    );
    expect(
      configPathFor({'XDG_CONFIG_HOME': '/x', 'HOME': '/h'}, 'linux'),
      '/x/OO/OO.cfg',
    );
    expect(configPathFor({'HOME': '/h'}, 'linux'), '/h/.config/OO/OO.cfg');
    expect(configPathFor({}, 'linux'), isNull);
  });
}
