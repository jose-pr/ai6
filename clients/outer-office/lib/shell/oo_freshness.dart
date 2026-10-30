import 'package:material_ui/material_ui.dart';

import '../theme/oo_tokens.dart';

/// The time now. Tests replace it so relative times are exact.
DateTime Function() ooClock = DateTime.now;

DateTime _at(double seconds) =>
    DateTime.fromMillisecondsSinceEpoch((seconds * 1000).round()).toLocal();

String _two(int value) => value.toString().padLeft(2, '0');

/// [seconds] (Unix time) as a local date and time with its offset from UTC, to the second:
/// `2026-10-30 14:02:31 (UTC-04:00)`.
String ooExactTime(double seconds) {
  final at = _at(seconds);
  final offset = at.timeZoneOffset;
  final minutes = offset.inMinutes.abs();
  return '${at.year}-${_two(at.month)}-${_two(at.day)} '
      '${_two(at.hour)}:${_two(at.minute)}:${_two(at.second)} '
      '(UTC${offset.isNegative ? '-' : '+'}${_two(minutes ~/ 60)}:${_two(minutes % 60)})';
}

/// How long before [now] [seconds] was: `just now`, `12 s ago`, `5 min ago`, `3 h ago` or
/// `2 d ago`. A time ahead of [now] (clocks differ) reads `just now`.
String ooRelativeTime(double seconds, {DateTime? now}) {
  final elapsed = (now ?? ooClock()).difference(_at(seconds)).inSeconds;
  if (elapsed < 5) return 'just now';
  if (elapsed < 60) return '$elapsed s ago';
  if (elapsed < 3600) return '${elapsed ~/ 60} min ago';
  if (elapsed < 86400) return '${elapsed ~/ 3600} h ago';
  return '${elapsed ~/ 86400} d ago';
}

/// A time as relative text, with the exact local time and zone in a tooltip and in the
/// semantics label; `Not reported` when the data holds none. Order by the value, never by
/// this text.
class OoTime extends StatelessWidget {
  const OoTime(this.seconds, {this.prefix = '', this.style, super.key});

  /// Unix time in seconds; `null` when the data does not say.
  final double? seconds;

  /// Words before the time, such as `Updated `.
  final String prefix;
  final TextStyle? style;

  @override
  Widget build(BuildContext context) {
    final style =
        this.style ??
        Theme.of(context).textTheme.bodySmall
            ?.copyWith(color: OoTokens.of(context).muted);
    final at = seconds;
    if (at == null) {
      return Text(
        prefix.isEmpty ? 'Not reported' : '${prefix.trim()}: Not reported',
        style: style,
      );
    }
    final relative = '$prefix${ooRelativeTime(at)}';
    final exact = ooExactTime(at);
    return Tooltip(
      message: exact,
      child: Semantics(
        label: '$relative, $exact',
        excludeSemantics: true,
        child: Text(relative, style: style),
      ),
    );
  }
}
