import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/forms/form_host.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/settings/oo_config.dart';
import 'package:outer_office/settings/oo_schema.dart';
import 'package:outer_office/settings/oo_settings_section.dart';
import 'package:outer_office/theme/oo_art.dart';
import 'package:outer_office/theme/oo_tokens.dart';

/// How the app holds each `[oo.table] key`, read back from a file with nothing in it.
Map<String, Object?> _held(OoConfig config) {
  final chat = config.chat;
  final limits = config.limits;
  final refresh = limits.refresh;
  final controls = limits.controls;
  final pages = limits.pages;
  final terminal = limits.terminal;
  final inputs = limits.inputs;
  return {
    'appearance.theme': config.theme,
    'chat.min_width': chat.minWidth,
    'chat.max_width': chat.maxWidth,
    'chat.width': chat.width,
    'chat.width_step': chat.widthStep,
    'chat.min_page_width': chat.minPageWidth,
    'chat.text_width': chat.textWidth,
    'chat.window_width': chat.windowWidth,
    'chat.window_height': chat.windowHeight,
    'chat.transition_ms': chat.transitionMs,
    'chat.connect_timeout': chat.connectTimeout,
    'chat.retry_delay': chat.retryDelay,
    'chat.retry_max_delay': chat.retryMaxDelay,
    'routes.text_bytes': config.routeLimits.textBytes,
    'art.max_width': config.art.maxWidth,
    'refresh.sessions': refresh.sessions,
    'refresh.overview': refresh.overview,
    'refresh.timeline': refresh.timeline,
    'refresh.pools': refresh.pools,
    'refresh.slow': refresh.slow,
    'refresh.profile': refresh.profile,
    'refresh.hidden': refresh.hidden,
    'refresh.silent_intervals': refresh.silentIntervals,
    'refresh.renew_before': refresh.renewBefore,
    'refresh.renew_retry': refresh.renewRetry,
    'controls.wait': controls.wait,
    'controls.poll': controls.poll,
    'controls.confirming': controls.confirming,
    'pages.timeline': pages.timeline,
    'pages.audit': pages.audit,
    'pages.summary': pages.summary,
    'pages.search': pages.search,
    'terminal.scrollback_lines': terminal.scrollbackLines,
    'terminal.connect_timeout': terminal.connectTimeout,
    'terminal.retry_delay': terminal.retryDelay,
    'terminal.retry_max_delay': terminal.retryMaxDelay,
    'limits.mission_title_chars': inputs.missionTitleChars,
    'limits.mission_prompt_chars': inputs.missionPromptChars,
    'limits.reason_chars': inputs.reasonChars,
  };
}

/// A schema's default as the app holds it.
Object? _asHeld(String table, String key) {
  final field = OoSchema.field(table, key);
  final declared = field['default'];
  if (field['format'] == 'seconds') {
    return Duration(
      microseconds: ((declared as num) * Duration.microsecondsPerSecond)
          .round(),
    );
  }
  return declared;
}

void main() {
  test('every default the app holds is the one its schema states', () {
    final held = _held(OoConfig.parse(''));
    final declared = <String>{
      for (final table in OoSchema.tables)
        // The web page's bootstrap reads [oo.assets], before the app runs.
        if (table != 'assets')
          for (final key in OoSchema.keysOf(table)) '$table.$key',
    };
    expect(held.keys.toSet(), declared);
    for (final path in declared) {
      final [table, key] = path.split('.');
      expect(held[path], _asHeld(table, key), reason: path);
    }
    // The const constructors read the same constants.
    expect(_held(OoConfig.none), held);
    expect(const ChatSettings().width, held['chat.width']);
    expect(const OoRouteLimits().textBytes, held['routes.text_bytes']);
    expect(const OoArtLimits().maxWidth, held['art.max_width']);
    expect(const RefreshSettings().renewRetry, held['refresh.renew_retry']);
    expect(
      const TerminalSettings().scrollbackLines,
      held['terminal.scrollback_lines'],
    );
  });

  test('the themes the schema offers are the ones there are', () {
    final offered = OoSchema.field('appearance', 'theme')['examples'] as List;
    expect(offered.toSet(), {
      ...OoThemeId.values.map((one) => one.id),
      'system',
    });
  });

  group('a value is held to its schema, by name and never quoted', () {
    const planted = 'p1anted-secret';
    for (final (toml, name) in [
      ('[oo.chat]\nwidth = "$planted"\n', 'oo.chat.width'),
      ('[oo.chat]\nwidth = 0\n', 'oo.chat.width'),
      ('[oo.chat]\ntransition_ms = 1.5\n', 'oo.chat.transition_ms'),
      ('[oo.refresh]\nsessions = -1\n', 'oo.refresh.sessions'),
      ('[oo.refresh]\nrenew_retry = 0\n', 'oo.refresh.renew_retry'),
      ('[oo.refresh]\nsilent_intervals = 0\n', 'oo.refresh.silent_intervals'),
      ('[oo.controls]\npoll = 0\n', 'oo.controls.poll'),
      ('[oo.pages]\naudit = 0\n', 'oo.pages.audit'),
      ('[oo.limits]\nreason_chars = -1\n', 'oo.limits.reason_chars'),
      ('[oo.routes]\ntext_bytes = 0\n', 'oo.routes.text_bytes'),
      ('[oo.art]\nmax_width = 0\n', 'oo.art.max_width'),
      ('[oo.appearance]\ntheme = 7\n', 'oo.appearance.theme'),
      ('[oo.chat]\n$planted = 1\n', 'unknown key'),
      ('[oo.$planted]\nx = 1\n', 'unknown table'),
    ]) {
      test(name, () {
        expect(
          () => OoConfig.parse(toml),
          throwsA(
            isA<FormatException>().having(
              (error) => error.message,
              'message',
              name.startsWith('unknown')
                  ? contains(name)
                  : allOf(contains(name), isNot(contains(planted))),
            ),
          ),
        );
      });
    }
    test('what the schema allows reads, 0 where 0 means something', () {
      final read = OoConfig.parse(
        '[oo.refresh]\nsessions = 0\n[oo.limits]\nreason_chars = 0\n'
        '[oo.controls]\nwait = 0\n[oo.chat]\ntransition_ms = 0\n',
      );
      expect(read.limits.refresh.sessions, isNull);
      expect(read.limits.inputs.reasonChars, 0);
      expect(read.limits.controls.wait, Duration.zero);
      expect(read.chat.transitionMs, 0);
    });
  });

  group('writing [oo] back', () {
    const others =
        '[provider]\npassword = { file = "pass" } # keep me\n'
        '[unex]\nurl = "https://ai6.example"\n';
    final oo = {
      'chat': {'width': 500.0},
      'pages': {'audit': 40},
    };

    test('TOML keeps every other section and its comments', () {
      final written = withOoSettings(
        '$others[oo.chat]\nmax_width = 800 # gone with the table\n[station]\nid = "a"\n',
        oo,
        path: 'ai6.cfg',
      );
      expect(written, contains('# keep me'));
      expect(written, contains('[station]\nid = "a"'));
      expect(written, isNot(contains('max_width')));
      final read = OoConfig.parse(written, path: 'ai6.cfg');
      expect(read.chat.width, 500);
      expect(read.limits.pages.audit, 40);
      expect(read.unexUrl.toString(), 'https://ai6.example');
    });

    test('JSON and YAML say the same, and nothing left removes the table', () {
      for (final (text, path) in [
        (
          '{"unex": {"url": "https://a.example"}, "oo": {"pages": {"audit": 7}}}',
          'a.json',
        ),
        (
          '# keep me\nunex:\n  url: https://a.example\noo:\n  pages:\n    audit: 7\n',
          'a.yaml',
        ),
      ]) {
        final written = withOoSettings(text, oo, path: path);
        expect(decodeOoConfig(written, path: path)['oo'], oo, reason: path);
        final emptied = withOoSettings(text, const {}, path: path);
        expect(decodeOoConfig(emptied, path: path).containsKey('oo'), isFalse);
        expect(
          OoConfig.parse(emptied, path: path).unexUrl.toString(),
          'https://a.example',
        );
      }
      expect(
        withOoSettings(others, const {}, path: 'ai6.cfg'),
        contains('# keep me'),
      );
    });

    test('only what differs from its default is kept', () {
      expect(
        OoSchema.withoutDefaults({
          'chat': {
            'width': OoSchema.defaultOf('chat', 'width'),
            'text_width': 800.0,
          },
          'pages': {'audit': OoSchema.defaultOf('pages', 'audit')},
          'nothing': {'at': 1},
        }),
        {
          'chat': {'text_width': 800.0},
        },
      );
    });
  });

  group('the settings form', () {
    Future<void> show(WidgetTester tester, Widget section) async {
      tester.view.physicalSize = const Size(800, 1600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        MaterialApp(
          localizationsDelegates: const [flutterMaterialLocalizations],
          home: Scaffold(
            body: Padding(
              padding: const EdgeInsets.all(8),
              child: SingleChildScrollView(child: section),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Outer Office settings'));
      await tester.pumpAndSettle();
    }

    test('fits its schema, judged by guatiao_intake, for every table', () {
      final model = ooFormModel(schema: OoSchema.schema, form: OoSchema.form);
      addTearDown(model.close);
      expect(model.schema.member('chat')?.title, "Moneypenny's chat");
      final fields = {for (final group in model.layout('')) ...group.fields};
      expect(fields, containsAll(OoSchema.tables));
    });

    testWidgets('shows what the file says, and saves a change to it', (
      tester,
    ) async {
      var file =
          '[unex]\nurl = "https://ai6.example"\n[oo.pages]\naudit = 40\n';
      await show(
        tester,
        OoSettingsSection(
          read: () => file,
          write: (text) async => file = text,
          path: 'ai6.cfg',
        ),
      );
      expect(find.text('How much a page asks for'), findsWidgets);
      await tester.ensureVisible(find.text('Save settings'));
      await tester.tap(find.text('Save settings'));
      await tester.pumpAndSettle();
      expect(find.textContaining('Saved.'), findsOneWidget);
      expect(file, contains('url = "https://ai6.example"'));
      final read = OoConfig.parse(file, path: 'ai6.cfg');
      expect(read.limits.pages.audit, 40);
      expect(read.limits.pages.search, const PageSettings().search);
    });

    testWidgets('changes nothing where the file is the deployment\'s', (
      tester,
    ) async {
      await show(
        tester,
        OoSettingsSection(read: () => '[oo.pages]\naudit = 40\n'),
      );
      expect(find.textContaining("this deployment's"), findsOneWidget);
      expect(find.text('Save settings'), findsNothing);
    });

    testWidgets('says so when a file cannot be read', (tester) async {
      await show(
        tester,
        OoSettingsSection(read: () => '[oo\n', path: 'ai6.cfg'),
      );
      expect(find.textContaining('cannot be shown'), findsOneWidget);
    });
  });
}
