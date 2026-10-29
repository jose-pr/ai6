import 'package:flutter/widgets.dart';
import 'package:flutter_svg/flutter_svg.dart';

import 'oo_tokens.dart';

/// The design kit's icons, under `assets/icons/`: 24×24 strokes in `currentColor`.
enum OoIconName {
  agents('agents'),
  audit('audit'),
  briefings('briefings'),
  filter('filter'),
  greenInk('green-ink'),
  interrupt('interrupt'),
  missions('missions'),
  moneypenny('moneypenny'),
  newMission('new-mission'),
  operations('operations'),
  pause('pause'),
  pools('pools'),
  resume('resume'),
  roster('roster'),
  search('search'),
  sessions('sessions'),
  stations('stations'),
  theaters('theaters'),
  timeline('timeline');

  const OoIconName(this.file);

  final String file;

  String get asset => 'assets/icons/$file.svg';
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
