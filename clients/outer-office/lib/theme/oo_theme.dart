import 'package:material_ui/material_ui.dart';

import 'oo_tokens.dart';

export 'oo_tokens.dart';

/// The display face of the wordmark and section labels: Cinzel (SIL OFL 1.1), bundled in
/// `assets/fonts/`. Body text stays Roboto.
const ooDisplayFamily = 'Cinzel';

/// A section label such as "RECENT ACTIVITY": Cinzel capitals in `accent`, which reads at
/// 4.5:1 on `surface` and `raised`.
TextStyle ooSectionLabel(BuildContext context) => TextStyle(
  fontFamily: ooDisplayFamily,
  fontWeight: FontWeight.w700,
  fontSize: 13,
  letterSpacing: 2.2,
  color: OoTokens.of(context).accent,
);

/// A filled button for a destructive step: `danger` with the readable one of `bg` and
/// `text` on it.
ButtonStyle ooDangerButton(OoTokens tokens) => FilledButton.styleFrom(
  backgroundColor: tokens.danger,
  foregroundColor: ooReadableOn(tokens.danger, tokens),
  shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
).copyWith(backgroundBuilder: (context, states, child) => child!);

/// Whichever of `bg`, `surface` and `text` reads best on [color].
Color ooReadableOn(Color color, OoTokens tokens) => _readableOn(color, tokens);

/// The Material theme of one Outer Office theme.
///
/// [highContrast] applies [OoTokens.highContrast]; [reduceMotion] makes page transitions
/// instant; without [backdrop] no wallpaper is painted.
ThemeData ooTheme(
  OoThemeId id, {
  bool highContrast = false,
  bool reduceMotion = false,
  bool backdrop = true,
}) {
  var tokens = highContrast ? id.tokens.highContrast() : id.tokens;
  if (!backdrop) tokens = tokens.copyWith(clearBackdrop: true);
  final onDanger = _readableOn(tokens.danger, tokens);
  final scheme = ColorScheme(
    brightness: id.brightness,
    primary: tokens.accent,
    onPrimary: tokens.bg,
    primaryContainer: tokens.raised,
    onPrimaryContainer: tokens.text,
    secondary: tokens.accent,
    onSecondary: tokens.bg,
    secondaryContainer: tokens.raised,
    onSecondaryContainer: tokens.text,
    tertiary: tokens.success,
    onTertiary: _readableOn(tokens.success, tokens),
    tertiaryContainer: Color.alphaBlend(
      tokens.success.withValues(alpha: 0.18),
      tokens.surface,
    ),
    onTertiaryContainer: tokens.text,
    error: tokens.danger,
    onError: onDanger,
    errorContainer: Color.alphaBlend(
      tokens.danger.withValues(alpha: 0.18),
      tokens.surface,
    ),
    onErrorContainer: tokens.text,
    surface: tokens.surface,
    onSurface: tokens.text,
    surfaceDim: tokens.bg,
    surfaceBright: tokens.raised,
    surfaceContainerLowest: tokens.bg,
    surfaceContainerLow: tokens.surface,
    surfaceContainer: tokens.raised,
    surfaceContainerHigh: tokens.raised,
    surfaceContainerHighest: tokens.raised,
    onSurfaceVariant: tokens.muted,
    outline: tokens.muted,
    outlineVariant: tokens.border,
    shadow: const Color(0xFF000000),
    scrim: const Color(0xFF000000),
    inverseSurface: tokens.text,
    onInverseSurface: tokens.bg,
    inversePrimary: tokens.accent,
    surfaceTint: const Color(0x00000000),
  );
  // Cards and panels: a border washed towards the accent, like the reference's gilt edges.
  final rounded = RoundedRectangleBorder(
    borderRadius: BorderRadius.circular(14),
    side: BorderSide(color: Color.lerp(tokens.border, tokens.accent, 0.22)!),
  );
  final buttonShape = RoundedRectangleBorder(
    borderRadius: BorderRadius.circular(10),
  );
  final selectedShape = RoundedRectangleBorder(
    borderRadius: BorderRadius.circular(10),
    side: BorderSide(color: tokens.accent.withValues(alpha: 0.6)),
  );
  // The rail merges its label styles with no text theme, so they start from one.
  final labels = ThemeData(colorScheme: scheme).textTheme.labelMedium;
  return ThemeData(
    useMaterial3: true,
    colorScheme: scheme,
    scaffoldBackgroundColor: tokens.bg,
    canvasColor: tokens.surface,
    dividerColor: tokens.border,
    dividerTheme: DividerThemeData(color: tokens.border),
    cardTheme: CardThemeData(
      color: tokens.surface,
      margin: EdgeInsets.zero,
      elevation: 0,
      shape: rounded,
    ),
    appBarTheme: AppBarTheme(
      backgroundColor: tokens.surface,
      foregroundColor: tokens.text,
      surfaceTintColor: const Color(0x00000000),
      shape: Border(bottom: BorderSide(color: tokens.border)),
    ),
    navigationRailTheme: NavigationRailThemeData(
      backgroundColor: tokens.surface,
      indicatorColor: tokens.selectedFill,
      indicatorShape: selectedShape,
      selectedIconTheme: IconThemeData(color: tokens.accent),
      unselectedIconTheme: IconThemeData(color: tokens.muted),
      selectedLabelTextStyle: labels?.copyWith(color: tokens.accent),
      unselectedLabelTextStyle: labels?.copyWith(color: tokens.muted),
    ),
    navigationDrawerTheme: NavigationDrawerThemeData(
      backgroundColor: tokens.surface,
      indicatorColor: tokens.selectedFill,
      indicatorShape: selectedShape,
      tileHeight: 60,
    ),
    // Primary buttons are the accent gradient with `bg` text; disabled ones are flat.
    filledButtonTheme: FilledButtonThemeData(
      style: ButtonStyle(
        shape: WidgetStatePropertyAll(buttonShape),
        backgroundColor: WidgetStateProperty.resolveWith(
          (states) => states.contains(WidgetState.disabled)
              ? tokens.raised
              : tokens.accent,
        ),
        foregroundColor: WidgetStateProperty.resolveWith(
          (states) =>
              states.contains(WidgetState.disabled) ? tokens.muted : tokens.bg,
        ),
        backgroundBuilder: (context, states, child) =>
            states.contains(WidgetState.disabled)
            ? child!
            : DecoratedBox(
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(10),
                  gradient: LinearGradient(
                    begin: Alignment.topCenter,
                    end: Alignment.bottomCenter,
                    colors: tokens.accentGradient,
                  ),
                  boxShadow: tokens.glow(tokens.accent, radius: 10),
                ),
                child: child,
              ),
      ),
    ),
    // Ghost buttons: `text` on the page, outlined in `muted`.
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        foregroundColor: tokens.text,
        side: BorderSide(color: tokens.muted),
        shape: buttonShape,
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(
        foregroundColor: tokens.accent,
        shape: buttonShape,
      ),
    ),
    badgeTheme: BadgeThemeData(
      backgroundColor: tokens.danger,
      textColor: onDanger,
    ),
    chipTheme: ChipThemeData(
      backgroundColor: tokens.raised,
      side: BorderSide(color: Color.lerp(tokens.border, tokens.accent, 0.3)!),
      labelStyle: TextStyle(color: tokens.text),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18)),
    ),
    progressIndicatorTheme: ProgressIndicatorThemeData(
      color: tokens.accent,
      linearTrackColor: tokens.border,
    ),
    drawerTheme: DrawerThemeData(backgroundColor: tokens.surface),
    dialogTheme: DialogThemeData(backgroundColor: tokens.surface),
    tabBarTheme: TabBarThemeData(
      labelColor: tokens.accent,
      unselectedLabelColor: tokens.muted,
      indicatorColor: tokens.accent,
      dividerColor: tokens.border,
    ),
    snackBarTheme: SnackBarThemeData(
      backgroundColor: tokens.text,
      contentTextStyle: TextStyle(color: tokens.bg),
    ),
    pageTransitionsTheme: reduceMotion
        ? PageTransitionsTheme(
            builders: {
              for (final platform in TargetPlatform.values)
                platform: const _InstantTransitions(),
            },
          )
        : const PageTransitionsTheme(),
    extensions: [tokens],
  );
}

/// Whichever of `bg`, `surface` and `text` reads best on [color].
Color _readableOn(Color color, OoTokens tokens) =>
    [tokens.bg, tokens.surface, tokens.text].reduce(
      (best, next) =>
          _contrast(next, color) > _contrast(best, color) ? next : best,
    );

double _contrast(Color a, Color b) {
  final la = a.computeLuminance();
  final lb = b.computeLuminance();
  return la > lb ? (la + 0.05) / (lb + 0.05) : (lb + 0.05) / (la + 0.05);
}

class _InstantTransitions extends PageTransitionsBuilder {
  const _InstantTransitions();

  @override
  Duration get transitionDuration => Duration.zero;

  @override
  Widget buildTransitions<T>(
    PageRoute<T> route,
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
    Widget child,
  ) => child;
}

/// A [MaterialApp] in the theme a person chose.
///
/// [themeId] is a stored preference ([ooThemePreference]): with [ooSystemTheme] the app
/// follows the operating system between Light and Night Ops. The system's high-contrast and
/// reduced-motion settings apply to every theme.
class OoMaterialApp extends StatelessWidget {
  const OoMaterialApp({
    required this.themeId,
    required this.backdrop,
    required this.title,
    required this.home,
    super.key,
  });

  final String themeId;
  final bool backdrop;
  final String title;
  final Widget home;

  @override
  Widget build(BuildContext context) {
    final highContrast = MediaQuery.maybeHighContrastOf(context) ?? false;
    final reduceMotion = MediaQuery.maybeDisableAnimationsOf(context) ?? false;
    final chosen = OoThemeId.byId(themeId);
    ThemeData theme(OoThemeId id) => ooTheme(
      id,
      highContrast: highContrast,
      reduceMotion: reduceMotion,
      backdrop: backdrop,
    );
    return MaterialApp(
      title: title,
      debugShowCheckedModeBanner: false,
      theme: theme(chosen ?? OoThemeId.light),
      darkTheme: theme(chosen ?? OoThemeId.nightOps),
      themeMode: chosen == null ? ThemeMode.system : ThemeMode.light,
      themeAnimationDuration: reduceMotion
          ? Duration.zero
          : kThemeAnimationDuration,
      home: home,
    );
  }
}
