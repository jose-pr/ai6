import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_svg/flutter_svg.dart';

import 'oo_tokens.dart';

/// Outer Office's icons: the design kit's line icons and five more in its style under
/// `assets/icons/` (24×24 strokes in `currentColor`), and a duotone drawing of each under
/// `assets/icons/duotone/` for tiles and other large uses.
enum OoIconName {
  agents('agents'),
  audit('audit'),
  briefings('briefings'),
  cable('cable'),
  filter('filter'),
  greenInk('green-ink'),
  interrupt('interrupt'),
  missions('missions'),
  moneypenny('moneypenny'),
  newMission('new-mission'),
  notifications('notifications'),
  operations('operations'),
  overview('overview'),
  pause('pause'),
  pools('pools'),
  resume('resume'),
  roster('roster'),
  search('search'),
  sessions('sessions'),
  settings('settings'),
  sleeper('sleeper'),
  stations('stations'),
  theaters('theaters'),
  timeline('timeline');

  const OoIconName(this.file);

  final String file;

  String get asset => 'assets/icons/$file.svg';

  /// The duotone drawing, in the sentinel colours [OoDuotone] replaces.
  String get duotoneAsset => 'assets/icons/duotone/$file.svg';
}

/// One of the kit's icons, sized and coloured by the nearest [IconTheme] like an [Icon].
class OoIcon extends StatelessWidget {
  const OoIcon(this.name, {this.semanticLabel, super.key});

  final OoIconName name;

  /// Read by screen readers; without one the icon is decorative.
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final theme = IconTheme.of(context);
    final size = theme.size ?? 24;
    final base = theme.color ?? const Color(0xFF000000);
    final color = base.withValues(alpha: base.a * (theme.opacity ?? 1));
    return SvgPicture.asset(
      name.asset,
      width: size,
      height: size,
      colorFilter: ColorFilter.mode(color, BlendMode.srcIn),
      semanticsLabel: semanticLabel,
      excludeFromSemantics: semanticLabel == null,
    );
  }
}

/// The kit icon for a Mission or Session state, where one fits; the state's name is always
/// shown beside it.
OoIconName? ooStateIcon(String state, {String? reason}) => switch (state) {
  'working' => OoIconName.resume,
  'parked' => OoIconName.pause,
  'blocked' when reason == 'green-ink' => OoIconName.greenInk,
  _ => null,
};

/// A state's mark in its token colour: the kit icon where one fits, else a dot. Always put
/// the state's name beside it.
class OoStateMark extends StatelessWidget {
  const OoStateMark(this.state, {this.reason, this.size = 14, super.key});

  final String state;
  final String? reason;
  final double size;

  @override
  Widget build(BuildContext context) {
    final color = OoTokens.of(context).stateColor(state);
    final icon = ooStateIcon(state, reason: reason);
    if (icon == null) {
      return SizedBox.square(
        dimension: size,
        child: Center(
          child: DecoratedBox(
            decoration: BoxDecoration(color: color, shape: BoxShape.circle),
            child: SizedBox.square(dimension: size * 0.6),
          ),
        ),
      );
    }
    return IconTheme.merge(
      data: IconThemeData(color: color, size: size),
      child: OoIcon(icon),
    );
  }
}

/// The colours the duotone SVGs are drawn in, replaced by the theme's at runtime.
const _duotoneHi = '#F6D58E';
const _duotoneLo = '#B9832F';
const _duotoneInk = '#2A1C08';

final _duotoneSources = <OoIconName, Future<String>>{};

String _hex(Color color) =>
    '#${(color.toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0')}';

/// The duotone drawing of [name] at [size], in the theme's accent: a gradient body with
/// darker details ([OoTokens.duotone]), so one set of files serves every theme.
class OoDuotone extends StatelessWidget {
  const OoDuotone(this.name, {this.size = 24, this.semanticLabel, super.key});

  final OoIconName name;
  final double size;

  /// Read by screen readers; without one the icon is decorative.
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final colors = OoTokens.of(context).duotone;
    final source = _duotoneSources.putIfAbsent(
      name,
      () => rootBundle.loadString(name.duotoneAsset),
    );
    final drawing = SizedBox.square(
      dimension: size,
      child: FutureBuilder<String>(
        future: source,
        builder: (context, snapshot) {
          final svg = snapshot.data;
          if (svg == null) return const SizedBox.shrink();
          return SvgPicture.string(
            svg
                .replaceAll(_duotoneHi, _hex(colors.hi))
                .replaceAll(_duotoneLo, _hex(colors.lo))
                .replaceAll(_duotoneInk, _hex(colors.ink)),
            width: size,
            height: size,
            excludeFromSemantics: true,
          );
        },
      ),
    );
    final label = semanticLabel;
    if (label == null) return ExcludeSemantics(child: drawing);
    return Semantics(
      label: label,
      image: true,
      excludeSemantics: true,
      child: drawing,
    );
  }
}
