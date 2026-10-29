import 'package:material_ui/material_ui.dart';

/// The preference value that follows the operating system: Light when it is light, Night
/// Ops when it is dark.
const ooSystemTheme = 'system';

/// The Outer Office themes, by the id `oo.theme` stores.
enum OoThemeId {
  nightOps('night-ops', 'Night Ops', Brightness.dark),
  blackOps('black-ops', 'Black Ops', Brightness.dark),
  dusk('dusk', 'Dusk', Brightness.dark),
  classic('classic', 'Classic', Brightness.light),
  light('light', 'Light', Brightness.light),
  redFile('red-file', 'Red File', Brightness.dark);

  const OoThemeId(this.id, this.label, this.brightness);

  /// The stored id, such as `night-ops`.
  final String id;
  final String label;
  final Brightness brightness;

  /// The theme with this id, or null for `system`, an unknown id or none.
  static OoThemeId? byId(String? id) =>
      values.where((theme) => theme.id == id).firstOrNull;

  /// This theme's colours.
  OoTokens get tokens => switch (this) {
    nightOps => _nightOps,
    blackOps => _blackOps,
    dusk => _dusk,
    classic => _classic,
    light => _light,
    redFile => _redFile,
  };
}

/// A stored theme preference made valid: a theme id or [ooSystemTheme]; absent or unknown
/// is Night Ops.
String ooThemePreference(String? stored) =>
    stored == ooSystemTheme || OoThemeId.byId(stored) != null
    ? stored!
    : OoThemeId.nightOps.id;

/// The opacity of the `surface` panel that holds text over art (`OoHero`, `OoBackdrop`).
const ooArtPanelOpacity = 0.92;

/// The colours of one Outer Office theme, read with [OoTokens.of].
///
/// `border` separates surfaces and carries no meaning; everything a person must read uses
/// `text`, `muted`, `accent` or a status colour, each at least 4.5:1 on `bg`, `surface`
/// and `raised` (status colours on `surface`).
@immutable
class OoTokens extends ThemeExtension<OoTokens> {
  const OoTokens({
    required this.bg,
    required this.surface,
    required this.raised,
    required this.text,
    required this.muted,
    required this.accent,
    required this.border,
    required this.success,
    required this.warning,
    required this.danger,
    required this.series,
    this.backdrop,
    this.hero,
    this.heroDuotone = false,
    this.decorated = true,
  });

  final Color bg;
  final Color surface;
  final Color raised;
  final Color text;
  final Color muted;
  final Color accent;
  final Color border;
  final Color success;
  final Color warning;
  final Color danger;

  /// The theme's wallpaper, painted only by `OoBackdrop`; null when it has none or it is
  /// turned off.
  final ImageProvider? backdrop;

  /// Eight categorical colours for charts, in a fixed order, each at least 3:1 on
  /// `surface`. Neighbours, and the first colour beside any other (a pie's last slice
  /// touches its first), stay apart for people with red-green colour blindness too.
  final List<Color> series;

  /// The art of the Overview hero, painted only by `OoHero`; null when it is turned off.
  final ImageProvider? hero;

  /// Whether [hero] is drawn in this theme's colours ([heroMatrix]) rather than as drawn.
  final bool heroDuotone;

  /// Whether ornament is drawn: glows, decorative gradients, the wallpaper and the hero
  /// art. False under high contrast ([highContrast]), where every one of them is flat.
  final bool decorated;

  /// Whether this is a dark theme: light text on a dark background.
  bool get dark => bg.computeLuminance() < text.computeLuminance();

  /// The two stops of the accent gradient (primary buttons, the wordmark), top first: a
  /// step lighter than `accent` on a dark theme and a step darker on a light one, so `bg`
  /// reads on both stops at least as well as on `accent`. Solid `accent` when not
  /// [decorated].
  List<Color> get accentGradient => !decorated
      ? [accent, accent]
      : dark
      ? [_lightness(accent, 0.1), accent]
      : [accent, _lightness(accent, -0.06)];

  /// A decorative gradient from [from] to [to], or solid [from] when not [decorated].
  List<Color> gradient(Color from, Color to) =>
      decorated ? [from, to] : [from, from];

  /// The duotone icon colours: the gradient's light and dark stops and the ink of details.
  ({Color hi, Color lo, Color ink}) get duotone => dark
      ? (
          hi: _lightness(accent, 0.12),
          lo: _lightness(accent, -0.14),
          ink: _setLightness(accent, 0.12),
        )
      : (
          hi: _lightness(accent, 0.24),
          lo: accent,
          ink: _setLightness(accent, 0.12),
        );

  /// The fill of a selected rail entry: `raised` washed with `accent`.
  Color get selectedFill =>
      Color.alphaBlend(accent.withValues(alpha: 0.16), raised);

  /// A soft glow of [color] around a selected entry or the primary action; none when not
  /// [decorated].
  List<BoxShadow> glow(Color color, {double radius = 8}) => !decorated
      ? const []
      : [
          BoxShadow(
            color: color.withValues(alpha: dark ? 0.45 : 0.3),
            blurRadius: radius,
          ),
        ];

  /// A 4x5 colour matrix that redraws a dark, mostly blue-grey picture in this theme: its
  /// darkest parts become `surface`, its brightest `accent`.
  List<double> get heroMatrix {
    const gain = 1.6;
    double c(double v) => v * 255;
    List<double> row(double from, double to) {
      final span = (to - from) * gain;
      return [span * 0.2126, span * 0.7152, span * 0.0722, 0, c(from)];
    }

    return [
      ...row(surface.r, accent.r),
      ...row(surface.g, accent.g),
      ...row(surface.b, accent.b),
      0,
      0,
      0,
      1,
      0,
    ];
  }

  /// The tokens of the nearest theme.
  static OoTokens of(BuildContext context) =>
      Theme.of(context).extension<OoTokens>() ?? _nightOps;

  /// These tokens for a person who asked for more contrast: muted text becomes text,
  /// separators become as visible as muted text, and nothing is [decorated].
  OoTokens highContrast() => OoTokens(
    bg: bg,
    surface: surface,
    raised: raised,
    text: text,
    muted: text,
    accent: accent,
    border: muted,
    success: success,
    warning: warning,
    danger: danger,
    series: series,
    decorated: false,
  );

  /// The colour of a Mission or Session state; the state's name is always shown beside it.
  Color stateColor(String state) => switch (state) {
    'working' => success,
    'blocked' => warning,
    'stalled' || 'failed' || 'quarantined' => danger,
    'parked' => accent,
    _ => muted,
  };

  @override
  OoTokens copyWith({
    Color? bg,
    Color? surface,
    Color? raised,
    Color? text,
    Color? muted,
    Color? accent,
    Color? border,
    Color? success,
    Color? warning,
    Color? danger,
    List<Color>? series,
    ImageProvider? backdrop,
    bool? decorated,
    bool clearBackdrop = false,
  }) => OoTokens(
    bg: bg ?? this.bg,
    surface: surface ?? this.surface,
    raised: raised ?? this.raised,
    text: text ?? this.text,
    muted: muted ?? this.muted,
    accent: accent ?? this.accent,
    border: border ?? this.border,
    success: success ?? this.success,
    warning: warning ?? this.warning,
    danger: danger ?? this.danger,
    series: series ?? this.series,
    backdrop: clearBackdrop ? null : backdrop ?? this.backdrop,
    hero: clearBackdrop ? null : hero,
    heroDuotone: heroDuotone,
    decorated: decorated ?? this.decorated,
  );

  @override
  OoTokens lerp(OoTokens? other, double t) {
    if (other == null) return this;
    Color mix(Color a, Color b) => Color.lerp(a, b, t)!;
    return OoTokens(
      bg: mix(bg, other.bg),
      surface: mix(surface, other.surface),
      raised: mix(raised, other.raised),
      text: mix(text, other.text),
      muted: mix(muted, other.muted),
      accent: mix(accent, other.accent),
      border: mix(border, other.border),
      success: mix(success, other.success),
      warning: mix(warning, other.warning),
      danger: mix(danger, other.danger),
      series: [
        for (var i = 0; i < series.length && i < other.series.length; i++)
          mix(series[i], other.series[i]),
      ],
      backdrop: t < 0.5 ? backdrop : other.backdrop,
      hero: t < 0.5 ? hero : other.hero,
      heroDuotone: t < 0.5 ? heroDuotone : other.heroDuotone,
      // A change into or out of high contrast draws no ornament on the way.
      decorated: decorated && other.decorated,
    );
  }
}

// The design kit's tokens. Where a kit colour fell below WCAG AA where text is drawn with
// it (`test/theme_test.dart`), only its HSL lightness moved, by the smallest 1% step that
// passes. The series take the accent first where it has enough chroma to name a series,
// then hues ordered so that neighbours differ in colour-blind simulations too.
const _nightOps = OoTokens(
  bg: Color(0xFF07111B),
  surface: Color(0xFF0D1C28),
  raised: Color(0xFF142A39),
  text: Color(0xFFF1E9D8),
  muted: Color(0xFF9BAEBB),
  accent: Color(0xFFD6A958),
  border: Color(0xFF24404E),
  success: Color(0xFF32C995),
  warning: Color(0xFFE9A845),
  danger: Color(0xFFD8595C),
  backdrop: AssetImage('assets/backdrops/night-ops.jpg'),
  hero: AssetImage('assets/heroes/map.jpg'),
  series: [
    Color(0xFFD6A958), // accent
    Color(0xFF3987E5), // blue
    Color(0xFFC44608), // orange
    Color(0xFFA298FE), // violet
    Color(0xFF069468), // aqua
    Color(0xFFA468C2), // purple
    Color(0xFF5BC5FA), // sky
    Color(0xFFD55181), // magenta
  ],
);

const _blackOps = OoTokens(
  bg: Color(0xFF080A0D),
  surface: Color(0xFF14191F),
  raised: Color(0xFF1E252E),
  text: Color(0xFFF0F2F2),
  muted: Color(0xFF9BA4AC),
  accent: Color(0xFFCBD1D0),
  border: Color(0xFF323941),
  success: Color(0xFF65C59D),
  warning: Color(0xFFE1B76C),
  danger: Color(0xFFDD6A6A),
  backdrop: AssetImage('assets/backdrops/black-ops.jpg'),
  hero: AssetImage('assets/heroes/map.jpg'),
  heroDuotone: true,
  series: [
    Color(0xFF3987E5), // blue
    Color(0xFFD95926), // orange
    Color(0xFF4EC494), // aqua
    Color(0xFF9A6502), // yellow
    Color(0xFFF56E9C), // magenta
    Color(0xFF008300), // green
    Color(0xFFA5CF45), // lime
    Color(0xFFDE9FFE), // purple
  ],
);

const _dusk = OoTokens(
  bg: Color(0xFF180E17),
  surface: Color(0xFF281723),
  raised: Color(0xFF3A222D),
  text: Color(0xFFFFF0E0),
  muted: Color(0xFFC7AAB1),
  accent: Color(0xFFECAC70),
  border: Color(0xFF624048),
  success: Color(0xFF6DCCA5),
  warning: Color(0xFFF1BA69),
  danger: Color(0xFFEC616E),
  backdrop: AssetImage('assets/backdrops/dusk.jpg'),
  hero: AssetImage('assets/heroes/skyline.jpg'),
  series: [
    Color(0xFFECAC70), // accent
    Color(0xFF3987E5), // blue
    Color(0xFF018059), // aqua
    Color(0xFFA298FE), // violet
    Color(0xFFD55181), // magenta
    Color(0xFF46B2E6), // sky
    Color(0xFF729405), // lime
    Color(0xFFA468C2), // purple
  ],
);

const _classic = OoTokens(
  bg: Color(0xFFE6D6BB),
  surface: Color(0xFFF5E8D2),
  raised: Color(0xFFFFF4DF),
  text: Color(0xFF30291F),
  muted: Color(0xFF635B4F),
  accent: Color(0xFF76582C),
  border: Color(0xFFC4AB85),
  success: Color(0xFF367253),
  warning: Color(0xFF915D1D),
  danger: Color(0xFF9F3D34),
  backdrop: AssetImage('assets/backdrops/classic.jpg'),
  hero: AssetImage('assets/heroes/map.jpg'),
  heroDuotone: true,
  series: [
    Color(0xFF7A6DF5), // violet
    Color(0xFFD6551C), // orange
    Color(0xFF0B8C8C), // teal
    Color(0xFF9F0117), // red
    Color(0xFF02587C), // sky
    Color(0xFF689001), // lime
    Color(0xFFAB446E), // magenta
    Color(0xFF026502), // green
  ],
);

const _light = OoTokens(
  bg: Color(0xFFF1F3F1),
  surface: Color(0xFFFFFFFF),
  raised: Color(0xFFEAF0F1),
  text: Color(0xFF152530),
  muted: Color(0xFF5C6F7B),
  accent: Color(0xFF2A6D84),
  border: Color(0xFFD4DFE2),
  success: Color(0xFF258262),
  warning: Color(0xFF9D691A),
  danger: Color(0xFFBA4F55),
  backdrop: AssetImage('assets/backdrops/light.jpg'),
  hero: AssetImage('assets/heroes/map.jpg'),
  heroDuotone: true,
  series: [
    Color(0xFF3481E0), // blue
    Color(0xFFEB6834), // orange
    Color(0xFF026502), // green
    Color(0xFFD36992), // magenta
    Color(0xFF9F0117), // red
    Color(0xFF06A572), // aqua
    Color(0xFF7B289D), // purple
    Color(0xFFA56F00), // yellow
  ],
);

// Red File has no wallpaper; its hero is the map in its own crimson.
const _redFile = OoTokens(
  bg: Color(0xFF17090E),
  surface: Color(0xFF240F18),
  raised: Color(0xFF351725),
  text: Color(0xFFFCEDEF),
  muted: Color(0xFFC9A4AF),
  accent: Color(0xFFE65960),
  border: Color(0xFF622A3C),
  success: Color(0xFF5ECEA0),
  warning: Color(0xFFE6B04C),
  danger: Color(0xFFFF6C72),
  hero: AssetImage('assets/heroes/map.jpg'),
  heroDuotone: true,
  series: [
    Color(0xFFE65960), // accent
    Color(0xFF3987E5), // blue
    Color(0xFF59CE9D), // aqua
    Color(0xFF6557B7), // violet
    Color(0xFF50BBF0), // sky
    Color(0xFFE3B738), // gold
    Color(0xFF199E9E), // teal
    Color(0xFFDE9FFE), // purple
  ],
);

/// [color] with its HSL lightness moved by [delta], kept within 0–1.
Color _lightness(Color color, double delta) {
  final hsl = HSLColor.fromColor(color);
  return hsl.withLightness((hsl.lightness + delta).clamp(0, 1)).toColor();
}

/// [color] at HSL [lightness], hue and saturation kept.
Color _setLightness(Color color, double lightness) =>
    HSLColor.fromColor(color).withLightness(lightness).toColor();
