import 'package:material_ui/material_ui.dart';

import 'oo_tokens.dart';

export 'oo_tokens.dart';

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
  final rounded = RoundedRectangleBorder(
    borderRadius: BorderRadius.circular(12),
    side: BorderSide(color: tokens.border),
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
      indicatorColor: tokens.raised,
      selectedIconTheme: IconThemeData(color: tokens.accent),
      unselectedIconTheme: IconThemeData(color: tokens.muted),
      selectedLabelTextStyle: labels?.copyWith(color: tokens.accent),
      unselectedLabelTextStyle: labels?.copyWith(color: tokens.muted),
    ),
    navigationDrawerTheme: NavigationDrawerThemeData(
      backgroundColor: tokens.surface,
      indicatorColor: tokens.raised,
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

/// `bg` or `text`, whichever reads better on [color].
Color _readableOn(Color color, OoTokens tokens) =>
    _contrast(tokens.bg, color) >= _contrast(tokens.text, color)
    ? tokens.bg
    : tokens.text;

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
