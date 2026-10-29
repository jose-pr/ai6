import 'package:material_ui/material_ui.dart';

import 'oo_tokens.dart';

/// The theme's wallpaper behind [child], which sits on a `surface` panel at 0.92 opacity so
/// text over it keeps its contrast.
///
/// Only for the sign-in page, the Overview header and empty states: never behind tables,
/// terminals, Session pages, dialogs, green ink, briefings or the chat. Paints only the panel
/// when the theme has no wallpaper, it is turned off, or high contrast is on.
class OoBackdrop extends StatelessWidget {
  const OoBackdrop({
    required this.child,
    this.padding = const EdgeInsets.all(24),
    this.alignment,
    super.key,
  });

  final Widget child;

  /// Around the panel, where the wallpaper shows when there is one.
  final EdgeInsets padding;

  /// Where the panel sits at its own size; without one it fills the space it is given.
  final AlignmentGeometry? alignment;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final panel = DecoratedBox(
      decoration: BoxDecoration(
        color: tokens.surface.withValues(alpha: 0.92),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: tokens.border),
      ),
      child: child,
    );
    final framed = Padding(
      padding: padding,
      child: alignment == null
          ? panel
          : Align(alignment: alignment!, child: panel),
    );
    final image = tokens.backdrop;
    if (image == null) return framed;
    final width =
        (MediaQuery.sizeOf(context).width *
                MediaQuery.devicePixelRatioOf(context))
            .round();
    return DecoratedBox(
      decoration: BoxDecoration(
        image: DecorationImage(
          // Decoded at the view's width, not the 1920 px source.
          image: ResizeImage(image, width: width < 1 ? 1 : width),
          fit: BoxFit.cover,
        ),
      ),
      child: framed,
    );
  }
}
