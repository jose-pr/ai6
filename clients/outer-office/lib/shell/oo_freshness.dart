import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_tokens.dart';

/// The time now. Tests replace it so relative times are exact.
DateTime Function() ooClock = DateTime.now;

/// How current what the board holds from one source is.
enum OoFreshness {
  /// Not read yet.
  loading,

  /// Read on the last attempt.
  live,

  /// The last attempt failed; the data is from an earlier read.
  stale,

  /// The last attempt failed and there is no earlier data.
  unavailable,

  /// The gateway refused it (403): the person may not see it, so it is hidden, not empty.
  noAccess,
}

/// The kinds of data the board reads, each with its own [OoFreshness].
enum BoardSource {
  stations('Stations'),
  board('Sessions'),
  missions('Missions'),
  heldBriefs('Held briefs'),
  greenInk('Green ink requests'),
  proposals('Mission proposals'),
  timeline('Timeline');

  const BoardSource(this.label);

  /// How it is named to the person.
  final String label;
}

/// [time] as local `HH:MM:SS`.
String ooClockTime(DateTime time) {
  final local = time.toLocal();
  return '${_two(local.hour)}:${_two(local.minute)}:${_two(local.second)}';
}

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
/// `2 d ago`; a time ahead of [now] (a deadline) reads `in 12 s`, `in 5 min` and so on.
/// Within five seconds either way (clocks differ) it is `just now`.
String ooRelativeTime(double seconds, {DateTime? now}) {
  final elapsed = (now ?? ooClock()).difference(_at(seconds)).inSeconds;
  final span = elapsed.abs();
  if (span < 5) return 'just now';
  final text = span < 60
      ? '$span s'
      : span < 3600
      ? '${span ~/ 60} min'
      : span < 86400
      ? '${span ~/ 3600} h'
      : '${span ~/ 86400} d';
  return elapsed < 0 ? 'in $text' : '$text ago';
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

/// The shell-wide strip shown only while the gateway is unreachable: what is on screen is
/// the last the gateway said, and the app keeps trying.
class GatewayStrip extends StatelessWidget {
  const GatewayStrip({required this.board, super.key});

  final MissionBoard board;

  @override
  Widget build(BuildContext context) {
    if (!board.unreachable) return const SizedBox.shrink();
    final at = board.lastUpdated(BoardSource.board);
    final tokens = OoTokens.of(context);
    return Semantics(
      liveRegion: true,
      child: Material(
        color: tokens.raised,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
          child: Row(
            children: [
              Icon(Icons.cloud_off, size: 18, color: tokens.warning),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  at == null
                      ? 'Universal Exports unreachable. Retrying.'
                      : 'Universal Exports unreachable, showing data from '
                            '${ooClockTime(at)}. Retrying.',
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A line for each of [sources] whose data is not current: when it was last read, or that it
/// could not be read at all. A source the gateway refused is hidden, so it says nothing here.
class FreshnessNote extends StatelessWidget {
  const FreshnessNote({required this.board, required this.sources, super.key});

  final MissionBoard board;
  final Iterable<BoardSource> sources;

  @override
  Widget build(BuildContext context) {
    final lines = [
      for (final source in sources)
        switch (board.freshness(source)) {
          OoFreshness.stale =>
            '${source.label}: Last updated ${ooClockTime(board.lastUpdated(source)!)}',
          OoFreshness.unavailable => '${source.label}: could not be read',
          _ => null,
        },
    ].nonNulls.toList();
    if (lines.isEmpty) return const SizedBox.shrink();
    final tokens = OoTokens.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.history, size: 16, color: tokens.warning),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              lines.join(' · '),
              style: Theme.of(context).textTheme.bodySmall
                  ?.copyWith(color: tokens.muted),
            ),
          ),
        ],
      ),
    );
  }
}
