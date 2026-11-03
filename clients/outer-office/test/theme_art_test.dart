import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/services.dart' show rootBundle;
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/settings/oo_config.dart';
import 'package:outer_office/theme/oo_art.dart';
import 'package:outer_office/theme/oo_decor.dart';
import 'package:outer_office/theme/oo_theme.dart';

import 'contrast.dart';

/// The darkest and lightest relative luminance of the `surface` panel at
/// [ooArtPanelOpacity] over every pixel of [asset], redrawn by [matrix] first when given.
Future<(double, double)> panelRange(
  String asset,
  Color surface,
  List<double>? matrix,
) async {
  final bytes = await rootBundle.load(asset);
  final codec = await ui.instantiateImageCodec(bytes.buffer.asUint8List());
  final image = (await codec.getNextFrame()).image;
  final pixels = (await image.toByteData())!;
  image.dispose();
  final linear = [
    for (var v = 0; v < 256; v++)
      v / 255 <= 0.04045
          ? v / 255 / 12.92
          : math.pow((v / 255 + 0.055) / 1.055, 2.4).toDouble(),
  ];
  final panel = [surface.r * 255, surface.g * 255, surface.b * 255];
  var low = 1.0;
  var high = 0.0;
  for (var i = 0; i < pixels.lengthInBytes; i += 4) {
    var rgb = [
      pixels.getUint8(i),
      pixels.getUint8(i + 1),
      pixels.getUint8(i + 2),
    ];
    if (matrix != null) {
      rgb = [
        for (var row = 0; row < 3; row++)
          (matrix[row * 5] * rgb[0] +
                  matrix[row * 5 + 1] * rgb[1] +
                  matrix[row * 5 + 2] * rgb[2] +
                  matrix[row * 5 + 4])
              .round()
              .clamp(0, 255),
      ];
    }
    final c = [
      for (var k = 0; k < 3; k++)
        (ooArtPanelOpacity * panel[k] + (1 - ooArtPanelOpacity) * rgb[k])
            .round()
            .clamp(0, 255),
    ];
    final l =
        0.2126 * linear[c[0]] + 0.7152 * linear[c[1]] + 0.0722 * linear[c[2]];
    low = math.min(low, l);
    high = math.max(high, l);
  }
  return (low, high);
}

void main() {
  testWidgets('text on the panels over art reads in every theme', (
    tester,
  ) async {
    final failures = <String>[];
    for (final theme in OoThemeId.values) {
      final t = theme.tokens;
      final ranges = <(String, double, double)>[];
      for (final file in t.hero?.files ?? const <OoArtFile>[]) {
        final (low, high) = (await tester.runAsync(
          () => panelRange(file.asset, t.surface, t.heroMatrix),
        ))!;
        ranges.add(('hero ${file.asset}', low, high));
      }
      for (final (where, low, high) in ranges) {
        for (final (name, color) in [
          ('text', t.text),
          ('muted', t.muted),
          ('accent', t.accent),
        ]) {
          final l = luminance(color);
          final worst = low <= l && l <= high
              ? 1.0
              : math.min(ratio(l, low), ratio(l, high));
          if (worst < 4.5) {
            failures.add(
              '${theme.label}: $name on the $where panel is '
              '${worst.toStringAsFixed(2)}:1',
            );
          }
        }
      }
    }
    expect(failures, isEmpty);
  });

  test('each theme has the art the direction sheet gives it', () {
    String? asset(OoThemeId id) => id.tokens.hero?.files.last.asset;
    expect(asset(OoThemeId.nightOps), 'assets/heroes/map-3840.jpg');
    expect(OoThemeId.nightOps.tokens.heroTint, OoArtTint.none);
    expect(asset(OoThemeId.dusk), 'assets/heroes/skyline-3840.jpg');
    expect(OoThemeId.dusk.tokens.heroTint, OoArtTint.none);
    expect(asset(OoThemeId.blackOps), 'assets/heroes/map-3840.jpg');
    expect(OoThemeId.blackOps.tokens.heroTint, OoArtTint.quiet);
    // One light master: Light as drawn, Classic tinted from its tokens.
    expect(asset(OoThemeId.light), 'assets/heroes/paper-3840.jpg');
    expect(OoThemeId.light.tokens.heroMatrix, isNull);
    expect(asset(OoThemeId.classic), 'assets/heroes/paper-3840.jpg');
    expect(OoThemeId.classic.tokens.heroTint, OoArtTint.paper);
    expect(OoThemeId.redFile.tokens.backdrop, isNull);
    expect(OoThemeId.redFile.tokens.heroTint, OoArtTint.night);
  });

  test('the paper tint maps paper to surface and lines to muted', () {
    final t = OoThemeId.classic.tokens;
    final m = t.heroMatrix!;
    Color redraw(double grey) => Color.from(
      alpha: 1,
      red: ((m[0] + m[1] + m[2]) * grey + m[4]) / 255,
      green: ((m[5] + m[6] + m[7]) * grey + m[9]) / 255,
      blue: ((m[10] + m[11] + m[12]) * grey + m[14]) / 255,
    );
    expect(distance(redraw(240), t.surface), lessThan(1));
    expect(distance(redraw(70), t.muted), lessThan(1));
  });

  testWidgets('a hero whose art is missing keeps its panel on the surface', (
    tester,
  ) async {
    final tokens = OoTokens(
      bg: OoThemeId.light.tokens.bg,
      surface: OoThemeId.light.tokens.surface,
      raised: OoThemeId.light.tokens.raised,
      text: OoThemeId.light.tokens.text,
      muted: OoThemeId.light.tokens.muted,
      accent: OoThemeId.light.tokens.accent,
      border: OoThemeId.light.tokens.border,
      success: OoThemeId.light.tokens.success,
      warning: OoThemeId.light.tokens.warning,
      danger: OoThemeId.light.tokens.danger,
      series: OoThemeId.light.tokens.series,
      hero: const OoArt([OoArtFile('assets/heroes/missing.jpg', 940, 278)]),
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: ooTheme(OoThemeId.light).copyWith(extensions: [tokens]),
        home: const Scaffold(
          body: OoHero(height: 200, child: Text('Command overview')),
        ),
      ),
    );
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(find.text('Command overview'), findsOneWidget);
    expect(find.byType(ColorFiltered), findsNothing);
    expect(
      tester
          .widgetList<ColoredBox>(find.byType(ColoredBox))
          .map((box) => box.color),
      contains(tokens.surface),
    );
  });

  group('the hero files', () {
    final files = {
      for (final theme in OoThemeId.values)
        for (final file in theme.tokens.hero?.files ?? const <OoArtFile>[])
          file.asset: file,
    };

    test('each is within its budget and at its named width', () async {
      expect(files, isNotEmpty);
      for (final file in files.values) {
        final bytes = await File(file.asset).readAsBytes();
        expect(
          bytes.length,
          lessThanOrEqualTo(ooArtBudget(file.width)),
          reason: file.asset,
        );
        expect(file.asset, contains('-${file.width}.'));
      }
    });

    testWidgets('each decodes at the size it says', (tester) async {
      for (final file in files.values) {
        final size = (await tester.runAsync(() async {
          final bytes = await rootBundle.load(file.asset);
          final codec = await ui.instantiateImageCodec(
            bytes.buffer.asUint8List(),
          );
          final image = (await codec.getNextFrame()).image;
          final size = (image.width, image.height);
          image.dispose();
          return size;
        }))!;
        expect(size, (file.width, file.height), reason: file.asset);
      }
    });
  });

  group('the hero picker', () {
    test('every shipped hero gives a 4K window its 3840 file', () {
      for (final theme in OoThemeId.values) {
        final art = theme.tokens.hero;
        if (art == null) continue;
        OoArt.forget();
        expect(art.pick(const Size(3226, 270)).width, 3840, reason: theme.name);
      }
    });

    // Widths the art will ship at, at the map's aspect.
    const art = OoArt([
      OoArtFile('a-940.jpg', 940, 278),
      OoArtFile('a-1536.jpg', 1536, 454),
      OoArtFile('a-2560.jpg', 2560, 757),
      OoArtFile('a-3840.jpg', 3840, 1136),
    ]);

    setUp(OoArt.forget);
    tearDown(() => OoArtLimits.current = const OoArtLimits());

    // The boxes the layouts draw, in physical pixels.
    for (final (box, width, why) in const [
      (Size(1306, 270), 1536, '1920x1080 at 1'),
      (Size(1946, 270), 2560, '2560x1440 at 1'),
      (Size(3226, 270), 3840, '3840x2160 at 1'),
      (Size(2612, 540), 3840, '1920x1080 at 2'),
      (Size(1652, 540), 2560, '1440x900 at 2: height decides'),
      (Size(1576, 440), 2560, '820x1180 at 2'),
      (Size(1098, 510), 2560, '390x844 at 3: height decides'),
      (Size(600, 170), 940, 'small'),
    ]) {
      test('$why picks $width', () {
        expect(art.pick(box).width, width);
      });
    }

    test('larger than every file takes the largest', () {
      expect(art.pick(const Size(5000, 1000)).width, 3840);
    });

    test('max_width caps it', () {
      expect(art.pick(const Size(3226, 270), maxWidth: 1920).width, 1536);
      OoArt.forget();
      OoArtLimits.current = const OoArtLimits(maxWidth: 1536);
      expect(art.pick(const Size(3226, 270)).width, 1536);
      expect(OoArtLimits.defaultMaxWidth, 3840);
      expect(OoConfig.parse('[oo.art]\nmax_width = 1920\n').art.maxWidth, 1920);
      expect(
        () => OoConfig.parse('[oo.art]\nmax_width = 0\n'),
        throwsFormatException,
      );
    });

    test('a smaller window never steps down', () {
      expect(art.pick(const Size(3226, 270)).width, 3840);
      expect(art.pick(const Size(600, 170)).width, 3840);
    });

    test('it decodes what the box needs, no more than the file holds', () {
      const file = OoArtFile('a-3840.jpg', 3840, 1136);
      expect(OoArt.decodeWidth(file, const Size(1000, 100)), 1000);
      expect(OoArt.decodeWidth(file, const Size(100, 568)), 1920);
      expect(OoArt.decodeWidth(file, const Size(9000, 100)), 3840);
    });

    testWidgets('the hero decodes under ResizeImage at its box', (
      tester,
    ) async {
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        MaterialApp(
          theme: ooTheme(OoThemeId.light),
          home: const Scaffold(
            body: Center(
              child: SizedBox(
                width: 500,
                height: 300,
                child: OoHero(height: null, child: Text('Command overview')),
              ),
            ),
          ),
        ),
      );
      final image = tester.widget<Image>(find.byType(Image)).image;
      expect(image, isA<ResizeImage>());
      final resized = image as ResizeImage;
      expect(resized.imageProvider, isA<AssetImage>());
      // 298 high inside its border, at 940x278, needs 1009 px across; the 1536 file covers it.
      expect(
        (resized.imageProvider as AssetImage).assetName,
        'assets/heroes/paper-1536.jpg',
      );
      expect(resized.width, 1009);
    });

    testWidgets('wallpaper off and high contrast draw no hero', (tester) async {
      for (final tokens in [
        OoThemeId.nightOps.tokens.copyWith(clearBackdrop: true),
        OoThemeId.nightOps.tokens.highContrast(),
      ]) {
        await tester.pumpWidget(
          MaterialApp(
            theme: ooTheme(OoThemeId.nightOps).copyWith(extensions: [tokens]),
            home: const Scaffold(
              body: Column(children: [OoHero(height: 100, child: Text('a'))]),
            ),
          ),
        );
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 50)),
        );
        await tester.pump();
        expect(find.byType(Image), findsNothing);
      }
    });
  });
}
