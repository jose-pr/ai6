import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart' show rootBundle;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:material_ui/material_ui.dart';

import 'oo_tokens.dart';

/// The drawing every theme's wallpaper is, and its label's text as outlines.
const ooWallpaperAsset = 'assets/backdrops/wallpaper.svg';
const ooWallpaperLabelAsset = 'assets/backdrops/wallpaper-label.svg';

/// The size the drawing and the label's place in it are measured in.
const _drawing = Size(1920, 1080);
const _labelLeft = 124.0;
const _labelBottom = 133.0;
const _labelText = Size(229, 21);
const _labelRule = 648.0;
const _labelTick = 53.0;
const _labelInset = 16.0;

/// The colours the drawings are made in, replaced by the theme's when they are painted.
const _sentinels = {
  '#a00001': _Slot.centre,
  '#a00002': _Slot.middle,
  '#a00003': _Slot.edge,
  '#a00004': _Slot.grid,
  '#a00005': _Slot.circle,
  '#a00006': _Slot.line,
  '#a00007': _Slot.ring,
  '#a00008': _Slot.dot,
  '#a00009': _Slot.label,
};

enum _Slot { centre, middle, edge, grid, circle, line, ring, dot, label }

String _hex(Color color) =>
    '#${(color.toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0')}';

/// [svg] in [wallpaper]'s colours.
String _inColours(String svg, OoWallpaper wallpaper) {
  var out = svg;
  _sentinels.forEach((sentinel, slot) {
    out = out.replaceAll(
      sentinel,
      _hex(switch (slot) {
        _Slot.centre => wallpaper.centre,
        _Slot.middle => wallpaper.middle,
        _Slot.edge => wallpaper.edge,
        _Slot.grid => wallpaper.grid,
        _Slot.circle => wallpaper.circle,
        _Slot.line => wallpaper.line,
        _Slot.ring => wallpaper.ring,
        _Slot.dot => wallpaper.dot,
        _Slot.label => wallpaper.label,
      }),
    );
  });
  return out;
}

/// The drawings already loaded: a page opened later paints at once.
final _loaded = <String, String>{};

/// The drawing [asset] in [wallpaper]'s colours; nothing until it has loaded, or if it cannot,
/// and the page's own background shows.
class _Drawing extends StatefulWidget {
  const _Drawing(this.asset, this.wallpaper, {required this.fit});

  final String asset;
  final OoWallpaper wallpaper;
  final BoxFit fit;

  @override
  State<_Drawing> createState() => _DrawingState();
}

class _DrawingState extends State<_Drawing> {
  @override
  void initState() {
    super.initState();
    if (_loaded.containsKey(widget.asset)) return;
    unawaited(
      rootBundle
          .loadString(widget.asset)
          .then((svg) {
            _loaded[widget.asset] = svg;
            if (mounted) setState(() {});
          })
          .catchError((Object _) {}),
    );
  }

  @override
  Widget build(BuildContext context) {
    final svg = _loaded[widget.asset];
    if (svg == null) return const SizedBox.shrink();
    return SvgPicture.string(
      _inColours(svg, widget.wallpaper),
      fit: widget.fit,
      excludeFromSemantics: true,
    );
  }
}

/// The theme's wallpaper behind [child], which sits on a `surface` panel at
/// [ooArtPanelOpacity] so text over it keeps its contrast.
///
/// Only for the sign-in page, the Overview header and empty states: never behind tables,
/// terminals, Session pages, dialogs, green ink, briefings or the chat. Paints only the panel
/// when the theme has no wallpaper, it is turned off, or high contrast is on.
///
/// With [labelClear], the width kept for a centred panel, it also draws the wallpaper's label at
/// the lower left, whole, where it fits beside that panel, and not at all where it does not.
class OoBackdrop extends StatelessWidget {
  const OoBackdrop({
    required this.child,
    this.padding = const EdgeInsets.all(24),
    this.alignment,
    this.labelClear,
    super.key,
  });

  final Widget child;

  /// Around the panel, where the wallpaper shows when there is one.
  final EdgeInsets padding;

  /// Where the panel sits at its own size; without one it fills the space it is given.
  final AlignmentGeometry? alignment;

  final double? labelClear;

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
    final framed = Padding(
      padding: padding,
      child: alignment == null
          ? panel
          : Align(alignment: alignment!, child: panel),
    );
    final wallpaper = tokens.decorated ? tokens.backdrop : null;
    if (wallpaper == null) return framed;
    return Stack(
      fit: StackFit.passthrough,
      children: [
        Positioned.fill(
          // Paints once per size: the page above it may change every second.
          child: RepaintBoundary(
            child: ClipRect(
              child: _Drawing(ooWallpaperAsset, wallpaper, fit: BoxFit.cover),
            ),
          ),
        ),
        if (labelClear case final clear?)
          Positioned.fill(
            child: LayoutBuilder(
              builder: (context, box) =>
                  _label(box.biggest, clear, wallpaper) ??
                  const SizedBox.shrink(),
            ),
          ),
        framed,
      ],
    );
  }

  /// The label for a window of [window]: at the place the drawing gives it, scaled as the
  /// drawing is, or null when it would not fit whole beside a centred panel [clear] wide.
  Widget? _label(Size window, double clear, OoWallpaper wallpaper) {
    final scale = math.max(
      window.width / _drawing.width,
      window.height / _drawing.height,
    );
    final side = (window.width - clear) / 2;
    final left = _labelLeft * window.width / _drawing.width;
    final textLeft = _labelInset * scale;
    final text = Size(_labelText.width * scale, _labelText.height * scale);
    final room = side - left - _labelInset;
    if (textLeft + text.width > room) return null;
    final tick = _labelTick * scale;
    return Stack(
      children: [
        Positioned(
          key: const ValueKey('wallpaper-label'),
          left: left,
          bottom: _labelBottom * window.height / _drawing.height,
          width: math.min(_labelRule * scale, room),
          height: tick,
          child: DecoratedBox(
            decoration: BoxDecoration(
              border: Border(
                left: BorderSide(color: wallpaper.rule, width: 2),
                bottom: BorderSide(color: wallpaper.rule, width: 2),
              ),
            ),
            child: Align(
              alignment: Alignment.centerLeft,
              child: Padding(
                padding: EdgeInsets.only(left: textLeft),
                child: SizedBox.fromSize(
                  size: text,
                  child: _Drawing(
                    ooWallpaperLabelAsset,
                    wallpaper,
                    fit: BoxFit.contain,
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}
