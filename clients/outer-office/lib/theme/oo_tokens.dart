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

/// The colours of one Outer Office theme, read with [OoTokens.of].
///
/// `border` separates surfaces and carries no meaning; everything a person must read uses
/// `text`, `muted`, `accent` or a status colour, each at least 4.5:1 on `surface`.
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

  /// Eight categorical colours for charts, each at least 3:1 on `surface`.
  final List<Color> series;

  /// The art of the Overview hero, painted only by `OoHero`; null when it is turned off.
  final ImageProvider? hero;

  /// Whether [hero] is drawn in this theme's colours ([heroMatrix]) rather than as drawn.
  final bool heroDuotone;

  /// Whether this is a dark theme: light text on a dark background.
  bool get dark => bg.computeLuminance() < text.computeLuminance();

  /// The two stops of the accent gradient (primary buttons, the wordmark), top first: a
  /// step lighter than `accent` on a dark theme and a step darker on a light one, so `bg`
  /// reads on both stops at least as well as on `accent`.
  List<Color> get accentGradient => dark
      ? [_lightness(accent, 0.1), accent]
      : [accent, _lightness(accent, -0.06)];

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

  /// A soft glow of [color] around a dot, tile or selected entry; none under high contrast.
  List<BoxShadow> glow(Color color, {double radius = 8}) => border == muted
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

  /// These tokens for a person who asked for more contrast: muted text becomes text and
  /// separators become as visible as muted text; no wallpaper.
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
    );
  }
}

// The design kit's tokens. Where a kit colour fell below WCAG AA on `surface` (or `bg` on
// `accent`), only its HSL lightness moved, by the smallest 1% step that passes.
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
    Color(0xFFD6A958),
    Color(0xFFE69F00),
    Color(0xFF56B4E9),
    Color(0xFF009E73),
    Color(0xFFF0E442),
    Color(0xFF0072B2),
    Color(0xFFD55E00),
    Color(0xFFCC79A7),
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
    Color(0xFFCBD1D0),
    Color(0xFFE69F00),
    Color(0xFF56B4E9),
    Color(0xFF009E73),
    Color(0xFFF0E442),
    Color(0xFF0072B2),
    Color(0xFFD55E00),
    Color(0xFFCC79A7),
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
    Color(0xFFECAC70),
    Color(0xFFE69F00),
    Color(0xFF56B4E9),
    Color(0xFF009E73),
    Color(0xFFF0E442),
    Color(0xFF0072B2),
    Color(0xFFD55E00),
    Color(0xFFCC79A7),
  ],
);

const _classic = OoTokens(
  bg: Color(0xFFE6D6BB),
  surface: Color(0xFFF5E8D2),
  raised: Color(0xFFFFF4DF),
  text: Color(0xFF30291F),
  muted: Color(0xFF71685A),
  accent: Color(0xFF76582C),
  border: Color(0xFFC4AB85),
  success: Color(0xFF367253),
  warning: Color(0xFF915D1D),
  danger: Color(0xFF9F3D34),
  backdrop: AssetImage('assets/backdrops/classic.jpg'),
  hero: AssetImage('assets/heroes/map.jpg'),
  heroDuotone: true,
  series: [
    Color(0xFF76582C),
    Color(0xFFAE7800),
    Color(0xFF1B8DCE),
    Color(0xFF00946C),
    Color(0xFF8E850B),
    Color(0xFF0072B2),
    Color(0xFFD55E00),
    Color(0xFFC46398),
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
    Color(0xFF2A6D84),
    Color(0xFFC28600),
    Color(0xFF209CE2),
    Color(0xFF009E73),
    Color(0xFFA1960D),
    Color(0xFF0072B2),
    Color(0xFFD55E00),
    Color(0xFFCC79A7),
  ],
);

// Red File has no wallpaper; its hero is the map in its own crimson.
const _redFile = OoTokens(
  bg: Color(0xFF17090E),
  surface: Color(0xFF240F18),
  raised: Color(0xFF351725),
  text: Color(0xFFFCEDEF),
  muted: Color(0xFFC9A4AF),
  accent: Color(0xFFE5555C),
  border: Color(0xFF622A3C),
  success: Color(0xFF5ECEA0),
  warning: Color(0xFFE6B04C),
  danger: Color(0xFFFF6C72),
  hero: AssetImage('assets/heroes/map.jpg'),
  heroDuotone: true,
  series: [
    Color(0xFFE5555C),
    Color(0xFFE69F00),
    Color(0xFF56B4E9),
    Color(0xFF009E73),
    Color(0xFFF0E442),
    Color(0xFF0072B2),
    Color(0xFFD55E00),
    Color(0xFFCC79A7),
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
