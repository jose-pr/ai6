import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/services.dart' show rootBundle;
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart'
    show
        AssetImage,
        Color,
        ColoredBox,
        ColorFiltered,
        MaterialApp,
        Scaffold,
        Text;
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
      for (final (where, art, matrix) in [
        ('hero', t.hero, t.heroMatrix),
        ('wallpaper', t.backdrop, null),
      ]) {
        if (art == null) continue;
        final (low, high) = (await tester.runAsync(
          () => panelRange((art as AssetImage).assetName, t.surface, matrix),
        ))!;
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
    String? asset(OoThemeId id) => (id.tokens.hero as AssetImage?)?.assetName;
    expect(asset(OoThemeId.nightOps), 'assets/heroes/map.jpg');
    expect(OoThemeId.nightOps.tokens.heroTint, OoArtTint.none);
    expect(asset(OoThemeId.dusk), 'assets/heroes/skyline.jpg');
    expect(OoThemeId.dusk.tokens.heroTint, OoArtTint.none);
    expect(asset(OoThemeId.blackOps), 'assets/heroes/map.jpg');
    expect(OoThemeId.blackOps.tokens.heroTint, OoArtTint.quiet);
    // One light master: Light as drawn, Classic tinted from its tokens.
    expect(asset(OoThemeId.light), 'assets/heroes/paper.jpg');
    expect(OoThemeId.light.tokens.heroMatrix, isNull);
    expect(asset(OoThemeId.classic), 'assets/heroes/paper.jpg');
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
      hero: const AssetImage('assets/heroes/missing.jpg'),
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
}
