import 'package:material_ui/material_ui.dart';

import 'oo_theme.dart';

/// The appearance settings: the six themes and System, each a miniature drawn in its own
/// tokens beside its name and character, and the Wallpaper switch.
///
/// A choice applies at once through [onTheme] and [onBackdrop]. The selected theme carries
/// a check mark and its name, so the choice never rests on colour; each row is a button
/// reachable by keyboard that announces whether it is selected. Under the system's high
/// contrast the switch shows off and is disabled, and the stored choice is left alone.
class OoAppearanceChooser extends StatelessWidget {
  const OoAppearanceChooser({
    required this.themeId,
    required this.backdrop,
    required this.onTheme,
    required this.onBackdrop,
    super.key,
  });

  /// The stored theme preference: a theme id or [ooSystemTheme].
  final String themeId;

  /// The stored wallpaper preference.
  final bool backdrop;
  final ValueChanged<String> onTheme;
  final ValueChanged<bool> onBackdrop;

  /// The Wallpaper switch's subtitle.
  static const wallpaperHint =
      'Art behind sign-in, the Overview hero and empty pages';

  /// The subtitle while the system asks for high contrast.
  static const wallpaperOffHint = 'Off while high contrast is on';

  @override
  Widget build(BuildContext context) {
    final highContrast = MediaQuery.maybeHighContrastOf(context) ?? false;
    OoTokens shown(OoThemeId id) =>
        highContrast ? id.tokens.highContrast() : id.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Semantics(header: true, child: const Text('Theme')),
        const SizedBox(height: 8),
        for (final theme in OoThemeId.values)
          _Choice(
            label: theme.label,
            character: theme.character,
            preview: [shown(theme)],
            selected: themeId == theme.id,
            onTap: () => onTheme(theme.id),
          ),
        _Choice(
          label: 'System',
          character: 'Light or Night Ops, as the system is set',
          preview: [shown(OoThemeId.light), shown(OoThemeId.nightOps)],
          selected: themeId == ooSystemTheme,
          onTap: () => onTheme(ooSystemTheme),
        ),
        const SizedBox(height: 8),
        SwitchListTile.adaptive(
          contentPadding: EdgeInsets.zero,
          title: const Text('Wallpaper'),
          subtitle: Text(highContrast ? wallpaperOffHint : wallpaperHint),
          value: backdrop && !highContrast,
          onChanged: highContrast ? null : onBackdrop,
        ),
      ],
    );
  }
}

/// One theme: its miniature, name and character, and a check mark when selected.
class _Choice extends StatelessWidget {
  const _Choice({
    required this.label,
    required this.character,
    required this.preview,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final String character;

  /// The tokens drawn in the miniature, side by side.
  final List<OoTokens> preview;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final radius = BorderRadius.circular(10);
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Semantics(
        container: true,
        button: true,
        selected: selected,
        inMutuallyExclusiveGroup: true,
        label: '$label theme. $character',
        excludeSemantics: true,
        child: Material(
          type: MaterialType.transparency,
          child: InkWell(
            borderRadius: radius,
            onTap: onTap,
            child: Ink(
              decoration: BoxDecoration(
                borderRadius: radius,
                color: selected ? tokens.selectedFill : null,
                border: Border.all(
                  color: selected
                      ? tokens.accent.withValues(alpha: 0.7)
                      : const Color(0x00000000),
                ),
              ),
              padding: const EdgeInsets.all(8),
              child: Row(
                children: [
                  for (final (index, theme) in preview.indexed) ...[
                    if (index > 0) const SizedBox(width: 4),
                    _Miniature(
                      theme,
                      width: (64 - 4 * (preview.length - 1)) / preview.length,
                    ),
                  ],
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          label,
                          style: text.titleSmall?.copyWith(
                            fontWeight: selected ? FontWeight.w700 : null,
                          ),
                        ),
                        Text(
                          character,
                          style: text.bodySmall?.copyWith(color: tokens.muted),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 8),
                  SizedBox.square(
                    dimension: 24,
                    child: selected
                        ? Icon(Icons.check, color: tokens.accent, size: 20)
                        : null,
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A theme in miniature, drawn with its own tokens: the page, a panel with two lines of
/// text, a selected row and a primary button.
class _Miniature extends StatelessWidget {
  const _Miniature(this.tokens, {required this.width});

  final OoTokens tokens;
  final double width;

  @override
  Widget build(BuildContext context) {
    Widget bar(Color color, double share, {double height = 3}) =>
        FractionallySizedBox(
          widthFactor: share,
          child: Container(
            height: height,
            decoration: BoxDecoration(
              color: color,
              borderRadius: BorderRadius.circular(2),
            ),
          ),
        );
    return Container(
      width: width,
      height: 44,
      padding: const EdgeInsets.all(4),
      decoration: BoxDecoration(
        color: tokens.bg,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: OoTokens.of(context).border),
      ),
      child: Container(
        padding: const EdgeInsets.all(3),
        decoration: BoxDecoration(
          color: tokens.surface,
          borderRadius: BorderRadius.circular(4),
          border: Border.all(color: tokens.border),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            bar(tokens.text, 0.6),
            Container(
              height: 8,
              decoration: BoxDecoration(
                color: tokens.selectedFill,
                border: Border(
                  left: BorderSide(color: tokens.accent, width: 2),
                ),
              ),
              alignment: Alignment.centerLeft,
              padding: const EdgeInsets.only(left: 2),
              child: bar(tokens.muted, 0.5),
            ),
            Align(
              alignment: Alignment.centerRight,
              child: FractionallySizedBox(
                widthFactor: 0.45,
                child: Container(
                  height: 7,
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(3),
                    gradient: LinearGradient(
                      begin: Alignment.topCenter,
                      end: Alignment.bottomCenter,
                      colors: tokens.accentGradient,
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
