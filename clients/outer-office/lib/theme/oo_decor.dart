import 'package:material_ui/material_ui.dart';

import 'oo_icon.dart';
import 'oo_theme.dart';

/// A rounded tile holding a duotone icon, like the stat cards of the design reference:
/// `raised` to `surface`, a thin accent edge and a soft accent glow.
class OoTile extends StatelessWidget {
  const OoTile(this.icon, {this.size = 56, this.semanticLabel, super.key});

  final OoIconName icon;

  /// Side in logical pixels; the icon takes 60 % of it.
  final double size;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(size * 0.26),
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: tokens.gradient(tokens.raised, tokens.surface),
        ),
        border: Border.all(color: tokens.accent.withValues(alpha: 0.4)),
        boxShadow: [
          for (final shadow in tokens.glow(tokens.accent, radius: size / 5))
            BoxShadow(
              color: shadow.color.withValues(alpha: shadow.color.a * 0.45),
              blurRadius: shadow.blurRadius,
            ),
        ],
      ),
      alignment: Alignment.center,
      child: OoDuotone(icon, size: size * 0.6, semanticLabel: semanticLabel),
    );
  }
}

/// A status dot in [color] with a glow of the same colour. Always put its meaning in text
/// beside it: colour alone says nothing to some people.
class OoStatusDot extends StatelessWidget {
  const OoStatusDot(this.color, {this.size = 9, super.key});

  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) => Container(
    width: size,
    height: size,
    decoration: BoxDecoration(
      color: color,
      shape: BoxShape.circle,
      boxShadow: OoTokens.of(context).glow(color, radius: size),
    ),
  );
}

/// The Outer Office wordmark: "OO" in Cinzel in the accent gradient and, unless [compact],
/// "OUTER OFFICE" spaced beside it.
class OoWordmark extends StatelessWidget {
  const OoWordmark({this.compact = false, this.size = 30, super.key});

  final bool compact;

  /// The height of "OO", in logical pixels.
  final double size;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final mark = ShaderMask(
      blendMode: BlendMode.srcIn,
      shaderCallback: (bounds) => LinearGradient(
        begin: Alignment.topCenter,
        end: Alignment.bottomCenter,
        colors: tokens.accentGradient,
      ).createShader(bounds),
      child: Text(
        'OO',
        style: TextStyle(
          fontFamily: ooDisplayFamily,
          fontWeight: FontWeight.w700,
          fontSize: size,
          height: 1,
          color: tokens.accent,
        ),
      ),
    );
    return Semantics(
      label: 'Outer Office',
      excludeSemantics: true,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          mark,
          if (!compact) ...[
            SizedBox(width: size * 0.45),
            Text(
              'OUTER OFFICE',
              style: TextStyle(
                fontFamily: ooDisplayFamily,
                fontSize: size * 0.5,
                letterSpacing: size * 0.14,
                color: tokens.text,
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// A card with a section label ([title], in [ooSectionLabel]), an optional [trailing]
/// widget beside it, and its [child].
class OoPanel extends StatelessWidget {
  const OoPanel({
    required this.title,
    required this.child,
    this.trailing,
    super.key,
  });

  final String title;
  final Widget? trailing;
  final Widget child;

  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Semantics(
                  header: true,
                  child: Text(title, style: ooSectionLabel(context)),
                ),
              ),
              ?trailing,
            ],
          ),
          const SizedBox(height: 12),
          child,
        ],
      ),
    ),
  );
}

/// The theme's hero art ([OoTokens.hero]) at [height], decorative only: [child] sits on a
/// `surface` panel at [ooArtPanelOpacity] over its lower left corner. Without art
/// (wallpaper off, high contrast) only the panel is shown.
///
/// Nothing is plotted on the art: AI6 knows no Station geography.
class OoHero extends StatelessWidget {
  const OoHero({required this.child, this.height = 260, super.key});

  final Widget child;

  /// Null to take the height it is given.
  final double? height;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final panel = DecoratedBox(
      decoration: BoxDecoration(
        color: tokens.surface.withValues(alpha: ooArtPanelOpacity),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: tokens.border),
      ),
      child: child,
    );
    final art = tokens.decorated ? tokens.hero : null;
    if (art == null) return Card(child: panel);
    Widget image = Image(
      image: art,
      fit: BoxFit.cover,
      excludeFromSemantics: true,
      gaplessPlayback: true,
    );
    if (tokens.heroDuotone) {
      image = ColorFiltered(
        colorFilter: ColorFilter.matrix(tokens.heroMatrix),
        child: image,
      );
    }
    return Container(
      height: height,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: Color.lerp(tokens.border, tokens.accent, 0.22)!,
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: Stack(
        fit: StackFit.expand,
        children: [
          // Positioned, so the art adds nothing to the hero's intrinsic height: beside
          // another panel the hero takes that panel's height whatever the art's shape.
          Positioned.fill(child: image),
          Align(
            alignment: Alignment.bottomLeft,
            child: Padding(padding: const EdgeInsets.all(14), child: panel),
          ),
        ],
      ),
    );
  }
}
