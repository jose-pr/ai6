import 'dart:math' as math;
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

/// WCAG 2.2 relative luminance of an sRGB colour.
double luminance(Color color) {
  double channel(double c) =>
      c <= 0.04045 ? c / 12.92 : math.pow((c + 0.055) / 1.055, 2.4).toDouble();
  return 0.2126 * channel(color.r) +
      0.7152 * channel(color.g) +
      0.0722 * channel(color.b);
}

/// WCAG 2.2 contrast ratio of two colours.
double contrast(Color a, Color b) {
  final la = luminance(a);
  final lb = luminance(b);
  return (math.max(la, lb) + 0.05) / (math.min(la, lb) + 0.05);
}

/// Every pair the tokens promise, as (name, foreground, background, minimum).
List<(String, Color, Color, double)> promisedPairs(OoTokens t) => [
  ('text on bg', t.text, t.bg, 4.5),
  ('text on surface', t.text, t.surface, 4.5),
  ('text on raised', t.text, t.raised, 4.5),
  ('muted on surface', t.muted, t.surface, 4.5),
  ('muted on raised', t.muted, t.raised, 4.5),
  ('accent on surface', t.accent, t.surface, 4.5),
  ('accent on raised (the selected rail icon and bar)', t.accent, t.raised, 3),
  ('bg on accent', t.bg, t.accent, 4.5),
  ('success on surface', t.success, t.surface, 4.5),
  ('warning on surface', t.warning, t.surface, 4.5),
  ('danger on surface', t.danger, t.surface, 4.5),
  ('outline (muted) on surface', t.muted, t.surface, 3),
  // The polish pass: primary button labels on both gradient stops, the wordmark's stops as
  // large text, the selected rail entry and her person bubble, the bell's count badge.
  for (final (index, stop) in t.accentGradient.indexed) ...[
    ('bg on accent gradient stop $index', t.bg, stop, 4.5),
    ('accent gradient stop $index on surface (large text)', stop, t.surface, 3),
  ],
  ('text on the selected fill', t.text, t.selectedFill, 4.5),
  ('badge label on danger', _readable(t.danger, t), t.danger, 4.5),
  ('accent on raised (labels in tiles and chips)', t.accent, t.raised, 3),
  for (final (index, color) in t.series.indexed)
    ('series $index on surface', color, t.surface, 3),
];

/// The label colour the theme puts on [color].
Color _readable(Color color, OoTokens t) => ooReadableOn(color, t);

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
  });

  for (final theme in OoThemeId.values) {
    test('${theme.label} meets every promised contrast ratio', () {
      for (final tokens in [theme.tokens, theme.tokens.highContrast()]) {
        expect(tokens.series, hasLength(8));
        for (final (name, foreground, background, minimum) in promisedPairs(
          tokens,
        )) {
          expect(
            contrast(foreground, background),
            greaterThanOrEqualTo(minimum),
            reason: '${theme.label}: $name',
          );
        }
      }
    });
  }

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

  test('high contrast and turning the wallpaper off drop the wallpaper', () {
    final contrasted = ooTheme(
      OoThemeId.dusk,
      highContrast: true,
    ).extension<OoTokens>()!;
    expect(contrasted.muted, OoThemeId.dusk.tokens.text);
    expect(contrasted.border, OoThemeId.dusk.tokens.muted);
    expect(contrasted.backdrop, isNull);
    expect(
      ooTheme(OoThemeId.dusk, backdrop: false).extension<OoTokens>()!.backdrop,
      isNull,
    );
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
