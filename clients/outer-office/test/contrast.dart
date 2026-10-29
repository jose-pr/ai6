import 'dart:math' as math;
import 'dart:ui' show Color;

import 'package:outer_office/theme/oo_theme.dart';

/// WCAG 2.2 relative luminance of an sRGB colour.
double luminance(Color color) =>
    0.2126 * _linear(color.r) +
    0.7152 * _linear(color.g) +
    0.0722 * _linear(color.b);

/// WCAG 2.2 contrast ratio of two colours.
double contrast(Color a, Color b) => ratio(luminance(a), luminance(b));

/// WCAG 2.2 contrast ratio of two relative luminances.
double ratio(double a, double b) =>
    (math.max(a, b) + 0.05) / (math.min(a, b) + 0.05);

double _linear(double c) =>
    c <= 0.04045 ? c / 12.92 : math.pow((c + 0.055) / 1.055, 2.4).toDouble();

/// [color] at [opacity] over [below], as the compositor blends it.
Color over(Color color, double opacity, Color below) =>
    Color.alphaBlend(color.withValues(alpha: opacity), below);

/// One pair a theme draws, as (name, foreground, background, minimum ratio).
typedef Pair = (String, Color, Color, double);

/// The pairs text, meaningful icons and control outlines are actually drawn with: text at
/// 4.5:1, icons, outlines and large text at 3:1. Decorative borders are not listed.
List<Pair> actualUsePairs(OoTokens t) {
  final warningNote = over(t.warning, 0.14, t.raised);
  return [
    for (final (name, ground) in [
      ('bg', t.bg),
      ('surface', t.surface),
      ('raised', t.raised),
    ]) ...[
      ('text on $name', t.text, ground, 4.5),
      ('muted on $name', t.muted, ground, 4.5),
      // Section labels and text buttons sit on pages, cards and raised panels alike.
      ('accent (section labels, text buttons) on $name', t.accent, ground, 4.5),
    ],
    ('success text on surface', t.success, t.surface, 4.5),
    ('warning text on surface', t.warning, t.surface, 4.5),
    ('danger text on surface', t.danger, t.surface, 4.5),
    // The selected rail entry: its label on the fill and on the gradient's other stop,
    // and the one 3:1 exemption, its accent icon and bar.
    ('text on the selected fill', t.text, t.selectedFill, 4.5),
    ('selected rail icon on the selected fill', t.accent, t.selectedFill, 3),
    ('selected rail bar on surface', t.accent, t.surface, 3),
    // Primary buttons: the label on both stops (solid accent without decoration).
    for (final (index, stop) in t.accentGradient.indexed) ...[
      ('primary label on accent gradient stop $index', t.bg, stop, 4.5),
      ('wordmark stop $index on surface (large text)', stop, t.surface, 3),
    ],
    ('primary label on solid accent', t.bg, t.accent, 4.5),
    ('danger button label', ooReadableOn(t.danger, t), t.danger, 4.5),
    // Status chips: the label on `raised`, the state mark beside it.
    for (final state in ['working', 'blocked', 'failed', 'parked', 'queued'])
      ('$state mark on a chip', t.stateColor(state), t.raised, 3),
    for (final state in ['working', 'blocked', 'failed', 'parked', 'queued'])
      ('$state dot on surface', t.stateColor(state), t.surface, 3),
    ('text on a warning note', t.text, warningNote, 4.5),
    ('warning icon on a warning note', t.warning, warningNote, 3),
    (
      'text on the error container',
      t.text,
      over(t.danger, 0.18, t.surface),
      4.5,
    ),
    (
      'text on the success container',
      t.text,
      over(t.success, 0.18, t.surface),
      4.5,
    ),
    ('snack bar text', t.bg, t.text, 4.5),
    ('outlined button edge (muted) on surface', t.muted, t.surface, 3),
  ];
}

/// [color] in OKLab, scaled by 100, optionally as a person with [blindness] sees it.
List<double> oklab(Color color, [List<List<double>>? blindness]) {
  var rgb = [_linear(color.r), _linear(color.g), _linear(color.b)];
  if (blindness != null) {
    rgb = [
      for (final row in blindness)
        (row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]).clamp(0.0, 1.0),
    ];
  }
  double cbrt(double v) =>
      (v < 0 ? -math.pow(-v, 1 / 3) : math.pow(v, 1 / 3)).toDouble();
  final l = cbrt(
    0.4122214708 * rgb[0] + 0.5363325363 * rgb[1] + 0.0514459929 * rgb[2],
  );
  final m = cbrt(
    0.2119034982 * rgb[0] + 0.6806995451 * rgb[1] + 0.1073969566 * rgb[2],
  );
  final s = cbrt(
    0.0883024619 * rgb[0] + 0.2817188376 * rgb[1] + 0.6299787005 * rgb[2],
  );
  return [
    100 * (0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s),
    100 * (1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s),
    100 * (0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s),
  ];
}

/// Protanopia and deuteranopia at severity 1.0 (Machado, Oliveira and Fernandes 2009),
/// applied to linear RGB.
const protanopia = [
  [0.152286, 1.052583, -0.204868],
  [0.114503, 0.786281, 0.099216],
  [-0.003882, -0.048116, 1.051998],
];
const deuteranopia = [
  [0.367322, 0.860646, -0.227968],
  [0.280085, 0.672501, 0.047413],
  [-0.011820, 0.042940, 0.968881],
];

/// The OKLab distance (×100) of two colours, optionally under [blindness].
double distance(Color a, Color b, [List<List<double>>? blindness]) {
  final x = oklab(a, blindness);
  final y = oklab(b, blindness);
  return math.sqrt(
    math.pow(x[0] - y[0], 2) +
        math.pow(x[1] - y[1], 2) +
        math.pow(x[2] - y[2], 2),
  );
}

/// Pairs of series slots that can touch in a chart: neighbours (bars, stacks, lines) and
/// the first slot beside every other (a pie's last slice touches its first).
List<(int, int)> touchingSlots(int count) => [
  for (var i = 0; i + 1 < count; i++) (i, i + 1),
  for (var k = 2; k < count; k++) (0, k),
];

/// What is wrong with [series] on [surface], or nothing: every slot 3:1 on it, touching
/// slots at least 15 apart in OKLab and 8 apart for protanopes and deuteranopes, and no
/// two slots closer than 12.
List<String> seriesProblems(List<Color> series, Color surface) => [
  for (final (index, color) in series.indexed)
    if (contrast(color, surface) < 3) 'slot $index is below 3:1 on surface',
  for (final (a, b) in touchingSlots(series.length)) ...[
    if (distance(series[a], series[b]) < 15)
      'slots $a and $b look alike (${distance(series[a], series[b]).toStringAsFixed(1)})',
    for (final (name, blindness) in [
      ('protanopia', protanopia),
      ('deuteranopia', deuteranopia),
    ])
      if (distance(series[a], series[b], blindness) < 8)
        'slots $a and $b look alike with $name',
  ],
  for (var a = 0; a < series.length; a++)
    for (var b = a + 1; b < series.length; b++)
      if (distance(series[a], series[b]) < 12)
        'slots $a and $b are nearly the same colour',
];
