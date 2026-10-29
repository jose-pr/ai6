import 'dart:ui' show Brightness, Color, Size;

import 'package:flutter_svg/flutter_svg.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart'
    show
        BuildContext,
        Builder,
        Icon,
        IconTheme,
        IconThemeData,
        Icons,
        MaterialApp,
        Row,
        Scaffold,
        SizedBox,
        TargetPlatform,
        Theme,
        ThemeData;
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/theme/oo_icon.dart';
import 'package:outer_office/theme/oo_theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'contrast.dart';

void main() {
  test('the ratio check fails the kit colours it was written to catch', () {
    // Classic's warning and Night Ops' danger as the design kit shipped them.
    expect(
      contrast(const Color(0xFFA66B21), const Color(0xFFF5E8D2)),
      lessThan(4.5),
    );
    expect(
      contrast(const Color(0xFFD65154), const Color(0xFF0D1C28)),
      lessThan(4.5),
    );
    expect(contrast(const Color(0xFF000000), const Color(0xFFFFFFFF)), 21);
    // Classic's muted and Red File's accent before this check covered `bg` and `raised`.
    expect(
      contrast(const Color(0xFF71685A), const Color(0xFFE6D6BB)),
      lessThan(4.5),
    );
    expect(
      contrast(const Color(0xFFE5555C), const Color(0xFF351725)),
      lessThan(4.5),
    );
  });

  for (final theme in OoThemeId.values) {
    test('${theme.label} meets the ratio of every pair it draws', () {
      for (final tokens in [theme.tokens, theme.tokens.highContrast()]) {
        expect(tokens.series, hasLength(8));
        for (final (name, foreground, background, minimum) in actualUsePairs(
          tokens,
        )) {
          expect(
            contrast(foreground, background),
            greaterThanOrEqualTo(minimum),
            reason:
                '${theme.label}${tokens.decorated ? '' : ' (high contrast)'}: '
                '$name',
          );
        }
      }
    });

    test('${theme.label} chart series stay apart', () {
      expect(
        seriesProblems(theme.tokens.series, theme.tokens.surface),
        isEmpty,
        reason: theme.label,
      );
    });
  }

  test('the series check fails the palette it was written to catch', () {
    // Night Ops' first two series before: its gold accent and an orange.
    final before = [
      const Color(0xFFD6A958),
      const Color(0xFFE69F00),
      ...OoThemeId.nightOps.tokens.series.skip(2),
    ];
    expect(
      seriesProblems(before, OoThemeId.nightOps.tokens.surface),
      contains(startsWith('slots 0 and 1 look alike')),
    );
  });

  test('high contrast draws no ornament in any theme', () {
    const white = Color(0xFFFFFFFF);
    for (final theme in OoThemeId.values) {
      final plain = theme.tokens.highContrast();
      expect(theme.tokens.decorated, isTrue, reason: theme.label);
      expect(plain.decorated, isFalse, reason: theme.label);
      expect(plain.glow(white), isEmpty, reason: theme.label);
      expect(theme.tokens.glow(white), isNotEmpty, reason: theme.label);
      expect(plain.accentGradient, [plain.accent, plain.accent]);
      expect(plain.gradient(plain.raised, plain.surface), [
        plain.raised,
        plain.raised,
      ]);
      expect(plain.hero, isNull, reason: theme.label);
      expect(plain.backdrop, isNull, reason: theme.label);
      // Carried by copyWith; lerp keeps it off from either side.
      expect(plain.copyWith(accent: white).decorated, isFalse);
      for (final t in [0.0, 0.5, 1.0]) {
        expect(theme.tokens.lerp(plain, t).decorated, isFalse);
        expect(plain.lerp(theme.tokens, t).decorated, isFalse);
      }
      expect(theme.tokens.lerp(theme.tokens, 0.5).decorated, isTrue);
    }
  });

  test('only Red File has no wallpaper', () {
    for (final theme in OoThemeId.values) {
      expect(
        theme.tokens.backdrop == null,
        theme == OoThemeId.redFile,
        reason: theme.label,
      );
    }
  });

  test('a stored preference is a theme id or system, else Night Ops', () {
    expect(ooThemePreference('red-file'), 'red-file');
    expect(ooThemePreference('system'), 'system');
    expect(ooThemePreference('dusk-x'), 'night-ops');
    expect(ooThemePreference(null), 'night-ops');
    expect(OoThemeId.byId('system'), isNull);
    expect(
      [for (final theme in OoThemeId.values) theme.id],
      ['night-ops', 'black-ops', 'dusk', 'classic', 'light', 'red-file'],
    );
  });

  test('the Material theme takes every colour from the tokens', () {
    for (final theme in OoThemeId.values) {
      final tokens = theme.tokens;
      final data = ooTheme(theme);
      final scheme = data.colorScheme;
      expect(scheme.brightness, theme.brightness);
      expect(scheme.primary, tokens.accent);
      expect(scheme.onPrimary, tokens.bg);
      expect(scheme.surface, tokens.surface);
      expect(scheme.onSurface, tokens.text);
      expect(scheme.onSurfaceVariant, tokens.muted);
      expect(scheme.surfaceContainer, tokens.raised);
      expect(scheme.outline, tokens.muted);
      expect(scheme.outlineVariant, tokens.border);
      expect(scheme.error, tokens.danger);
      expect(data.scaffoldBackgroundColor, tokens.bg);
      expect(data.extension<OoTokens>()!.accent, tokens.accent);
    }
  });

  test('state colours come from the tokens', () {
    final tokens = OoThemeId.light.tokens;
    expect(tokens.stateColor('working'), tokens.success);
    expect(tokens.stateColor('blocked'), tokens.warning);
    for (final state in ['stalled', 'failed', 'quarantined']) {
      expect(tokens.stateColor(state), tokens.danger, reason: state);
    }
    expect(tokens.stateColor('parked'), tokens.accent);
    for (final state in ['queued', 'done', 'abandoned']) {
      expect(tokens.stateColor(state), tokens.muted, reason: state);
    }
    expect(boardStates, hasLength(9));
  });

  test('high contrast and turning the wallpaper off drop the wallpaper and '
      'the hero art', () {
    final contrasted = ooTheme(
      OoThemeId.dusk,
      highContrast: true,
    ).extension<OoTokens>()!;
    expect(contrasted.muted, OoThemeId.dusk.tokens.text);
    expect(contrasted.border, OoThemeId.dusk.tokens.muted);
    expect(contrasted.backdrop, isNull);
    expect(contrasted.hero, isNull);
    for (final theme in OoThemeId.values) {
      final off = ooTheme(theme, backdrop: false).extension<OoTokens>()!;
      expect(off.backdrop, isNull, reason: theme.label);
      expect(off.hero, isNull, reason: theme.label);
      expect(off.decorated, isTrue, reason: theme.label);
      expect(ooTheme(theme).extension<OoTokens>()!.hero, isNotNull);
    }
    expect(ooTheme(OoThemeId.dusk).extension<OoTokens>()!.backdrop, isNotNull);
  });

  test('reduced motion makes page transitions instant', () {
    final reduced = ooTheme(OoThemeId.nightOps, reduceMotion: true);
    for (final platform in TargetPlatform.values) {
      expect(
        reduced.pageTransitionsTheme.builders[platform]!.transitionDuration,
        Duration.zero,
        reason: '$platform',
      );
    }
    expect(
      ooTheme(OoThemeId.nightOps)
          .pageTransitionsTheme
          .builders[TargetPlatform.windows]!
          .transitionDuration,
      greaterThan(Duration.zero),
    );
  });

  group('the app follows', () {
    Future<OoTokens> tokensIn(WidgetTester tester, String themeId) async {
      late OoTokens tokens;
      await tester.pumpWidget(
        OoMaterialApp(
          themeId: themeId,
          backdrop: true,
          title: 'test',
          home: Builder(
            builder: (BuildContext context) {
              tokens = OoTokens.of(context);
              return const SizedBox();
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      return tokens;
    }

    testWidgets('the system between Light and Night Ops', (tester) async {
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.light;
      expect((await tokensIn(tester, 'system')).bg, OoThemeId.light.tokens.bg);
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
      expect(
        (await tokensIn(tester, 'system')).bg,
        OoThemeId.nightOps.tokens.bg,
      );
      expect(
        (await tokensIn(tester, 'classic')).bg,
        OoThemeId.classic.tokens.bg,
      );
    });

    testWidgets('the system high-contrast setting', (tester) async {
      addTearDown(
        tester.platformDispatcher.clearAccessibilityFeaturesTestValue,
      );
      tester.platformDispatcher.accessibilityFeaturesTestValue =
          const FakeAccessibilityFeatures(highContrast: true);
      final tokens = await tokensIn(tester, 'light');
      expect(tokens.muted, OoThemeId.light.tokens.text);
      expect(tokens.backdrop, isNull);
    });

    testWidgets('the system reduced-motion setting', (tester) async {
      addTearDown(
        tester.platformDispatcher.clearAccessibilityFeaturesTestValue,
      );
      tester.platformDispatcher.accessibilityFeaturesTestValue =
          const FakeAccessibilityFeatures(disableAnimations: true);
      late ThemeData theme;
      await tester.pumpWidget(
        OoMaterialApp(
          themeId: 'dusk',
          backdrop: true,
          title: 'test',
          home: Builder(
            builder: (context) {
              theme = Theme.of(context);
              return const SizedBox();
            },
          ),
        ),
      );
      expect(
        theme
            .pageTransitionsTheme
            .builders[TargetPlatform.android]!
            .transitionDuration,
        Duration.zero,
      );
      // Changing the theme does not animate either.
      expect(
        tester
            .widget<MaterialApp>(find.byType(MaterialApp))
            .themeAnimationDuration,
        Duration.zero,
      );
      tester.platformDispatcher.clearAccessibilityFeaturesTestValue();
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<MaterialApp>(find.byType(MaterialApp))
            .themeAnimationDuration,
        greaterThan(Duration.zero),
      );
    });
  });

  testWidgets(
    'a chosen theme survives a restart; an unknown one is Night Ops',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      final before = AppearancePreferences(SharedPreferences.getInstance());
      final first = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        onThemeChanged: before.saveTheme,
        onBackdropChanged: before.saveBackdrop,
      );
      addTearDown(first.dispose);
      await before.restore(first);
      expect(first.themeId, 'night-ops');
      expect(first.backdrop, isTrue);
      first.setThemeId('red-file');
      first.setBackdrop(false);
      await tester.pump();
      final stored = await SharedPreferences.getInstance();
      expect(stored.getString('oo.theme'), 'red-file');
      expect(stored.getBool('oo.backdrop'), isFalse);
      expect(stored.getKeys(), {'oo.theme', 'oo.backdrop'});

      final restarted = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
      );
      addTearDown(restarted.dispose);
      await AppearancePreferences(SharedPreferences.getInstance())
          .restore(restarted);
      expect(restarted.themeId, 'red-file');
      expect(restarted.backdrop, isFalse);

      SharedPreferences.setMockInitialValues({'oo.theme': 'dusk-x'});
      final unknown = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        themeId: 'dusk',
      );
      addTearDown(unknown.dispose);
      await AppearancePreferences(SharedPreferences.getInstance())
          .restore(unknown);
      expect(unknown.themeId, 'night-ops');
    },
  );

  testWidgets('every kit icon parses and renders at the icon theme size', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    for (final name in OoIconName.values) {
      final picture = await tester.runAsync(
        () => vg.loadPicture(SvgAssetLoader(name.asset), null),
      );
      expect(picture!.size, const Size(24, 24), reason: name.file);
      picture.picture.dispose();
    }
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: IconTheme(
            data: const IconThemeData(size: 20, color: Color(0xFF2A6D84)),
            child: Row(
              children: [
                for (final name in OoIconName.values)
                  OoIcon(name, semanticLabel: name.file),
                const Icon(Icons.check),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(OoIconName.values, hasLength(24));
    for (final name in OoIconName.values) {
      final duotone = await tester.runAsync(
        () => vg.loadPicture(SvgAssetLoader(name.duotoneAsset), null),
      );
      expect(duotone!.size, const Size(24, 24), reason: name.duotoneAsset);
      duotone.picture.dispose();
    }
    for (final name in OoIconName.values) {
      expect(
        tester.getSize(find.bySemanticsLabel(name.file)),
        const Size(20, 20),
        reason: name.file,
      );
    }
    semantics.dispose();
  });
}
