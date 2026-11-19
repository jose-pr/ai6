import 'package:coqui_acp_chat/coqui_acp_chat.dart';

/// The `[usage] warn_permille` default: the fraction from which the control shows a status.
const int defaultWarnPermille = 800;

/// A count in a few characters: `950`, `1.5k`, `120k`, `2.4M`.
String compactCount(int count) {
  String scaled(double value, String suffix) {
    final text = value >= 100 || value == value.roundToDouble() ? value.round().toString() : value.toStringAsFixed(1);
    return '$text$suffix';
  }

  if (count < 1000) return '$count';
  if (count < 1000000) return scaled(count / 1000, 'k');
  if (count < 1000000000) return scaled(count / 1000000, 'M');
  return scaled(count / 1000000000, 'G');
}

/// Millionths of a US dollar as money: `$1.20`.
String dollars(int microUsd) {
  final cents = (microUsd + 5000) ~/ 10000;
  return '\$${cents ~/ 100}.${(cents % 100).toString().padLeft(2, '0')}';
}

/// A length of time in its two largest units: `1 h 20 m`, `3 d 4 h`, `45 s`.
String spanText(int seconds) {
  if (seconds < 60) return seconds <= 0 ? '0 s' : '$seconds s';
  final days = seconds ~/ 86400, hours = seconds % 86400 ~/ 3600, minutes = seconds % 3600 ~/ 60;
  if (days > 0) return hours == 0 ? '$days d' : '$days d $hours h';
  if (hours > 0) return minutes == 0 ? '$hours h' : '$hours h $minutes m';
  return '$minutes m';
}

/// How long ago, as the menu says it: `just now`, `4 min ago`, `2 h ago`.
String agoText(int seconds) {
  if (seconds < 45) return 'just now';
  if (seconds < 3600) return '${(seconds / 60).round()} min ago';
  if (seconds < 86400) return '${(seconds / 3600).round()} h ago';
  return '${(seconds / 86400).round()} d ago';
}

/// The unit's words after a count: `tokens`, `requests`; none for money and percentages.
String? _unitWords(UsageUnit unit) => switch (unit) {
  UsageUnit.tokens => 'tokens',
  UsageUnit.requests => 'requests',
  UsageUnit.microUsd || UsageUnit.permille => null,
  _ => unit.name,
};

/// How much of [bucket] is used, as a person reads it: `62%`, `120k / 400k tokens`, `$1.20 / $5.00`. Null when the bucket gives
/// no figure at all.
String? figureOf(UsageBucket bucket) {
  final fraction = bucket.fractionPermille;
  if (bucket.unit == UsageUnit.permille) return fraction == null ? null : '${fraction ~/ 10}%';
  final limit = bucket.limit;
  final used = bucket.used ?? (limit != null && bucket.remaining != null ? (bucket.remaining! >= limit ? 0 : limit - bucket.remaining!) : null);
  String count(int value) => bucket.unit == UsageUnit.microUsd ? dollars(value) : compactCount(value);
  final words = _unitWords(bucket.unit);
  final tail = words == null ? '' : ' $words';
  if (used != null && limit != null) return '${count(used)} / ${count(limit)}$tail';
  if (used != null) return '${count(used)}$tail used';
  if (bucket.remaining != null) return '${count(bucket.remaining!)}$tail left';
  return fraction == null ? null : '${fraction ~/ 10}%';
}

/// When [bucket] comes back, as the control says it: `resets in 1 h 20 m`, `40k tokens/min`, or that its window has ended
/// and the agent has not yet said so. Null for a balance, a refill that is not known and a window with no known end.
String? refillOf(UsageBucket bucket, int now) {
  final refill = bucket.refill;
  if (refill.isFixedWindow) {
    final left = bucket.resetsIn(now);
    if (left == null) return null;
    return left == 0 ? 'reset, not yet reported' : 'resets in ${spanText(left)}';
  }
  if (refill.isTokenBucket) {
    final per = refill.per!;
    if (per <= Duration.zero) return null;
    final words = _unitWords(bucket.unit);
    final perSecond = refill.amount! * 1000000 / per.inMicroseconds;
    final (rate, span) = per >= const Duration(hours: 1) ? (perSecond * 3600, 'h') : (perSecond * 60, 'min');
    final amount = bucket.unit == UsageUnit.microUsd ? dollars(rate.round()) : compactCount(rate.round());
    return '$amount${words == null ? '' : ' $words'}/$span';
  }
  return null;
}

/// The coverage in words: `every model`, `opus`, `models not known`.
String coverageOf(UsageCoverage models) => switch (models.kind) {
  'all' => 'every model',
  'only' => models.models.isEmpty ? 'some models' : models.models.join(', '),
  _ => 'models not known',
};

/// A bucket's place in the order the control uses: an exhausted one first, then the highest fraction used, and last a bucket whose
/// window has ended (its figures are of the window before) or that gives no fraction. Larger comes first.
int _rank(UsageBucket bucket, int now) {
  if (bucket.pastReset(now)) return -2;
  if (bucket.isExhausted) return 2000;
  return bucket.fractionPermille ?? -1;
}

/// The buckets in the order the control lists them.
List<UsageBucket> ordered(UsageReport report, int now) {
  final indexed = report.buckets.indexed.toList()
    ..sort((a, b) {
      final byRank = _rank(b.$2, now).compareTo(_rank(a.$2, now));
      return byRank != 0 ? byRank : a.$1.compareTo(b.$1);
    });
  return [for (final (_, bucket) in indexed) bucket];
}

/// What the control shows: the bucket that matters most, or none when nothing was reported.
UsageBucket? headlineOf(UsageReport report, int now) => ordered(report, now).firstOrNull;

/// How the control looks: nothing wrong, close to the limit (a status), or out of it (danger).
enum UsageLevel { calm, warn, danger }

/// The level of [bucket]: exhausted is danger; from [warnPermille] used, or a status the agent itself named `warning`, is warn.
/// A window that has ended is calm: what it shows is of the window before.
UsageLevel levelOf(UsageBucket bucket, int now, {int warnPermille = defaultWarnPermille}) {
  if (bucket.pastReset(now)) return UsageLevel.calm;
  if (bucket.isExhausted) return UsageLevel.danger;
  if (bucket.status == 'warning' || (bucket.fractionPermille ?? 0) >= warnPermille) return UsageLevel.warn;
  return UsageLevel.calm;
}

/// The control's text for [report]: `No usage yet` when it holds no bucket, else the headline bucket's name, figure and refill.
String pillText(UsageReport report, int now) {
  final bucket = headlineOf(report, now);
  if (bucket == null) return 'No usage yet';
  final figure = bucket.pastReset(now) ? null : figureOf(bucket) ?? (bucket.isExhausted ? 'limit reached' : null);
  final refill = refillOf(bucket, now);
  return [bucket.name, ?figure, ?refill].join(' · ');
}

/// One line of the control's menu for [bucket]: name and figure, then how it refills.
String bucketLine(UsageBucket bucket, int now) {
  final figure = bucket.pastReset(now) ? null : figureOf(bucket) ?? (bucket.isExhausted ? 'limit reached' : null);
  return [bucket.name, ?figure, ?refillOf(bucket, now)].join(' · ');
}

/// What else the menu says of [bucket], a line each: that it is inferred, which models it covers, and as of when.
List<String> bucketDetails(UsageBucket bucket, int now) => [
  if (bucket.inferred) 'inferred from ${bucket.evidence == null ? 'an error' : '"${bucket.evidence}"'}, not reported by the agent',
  'covers ${coverageOf(bucket.models)}',
  if (bucket.observedAt > 0) 'as of ${agoText(now - bucket.observedAt)}',
];

/// The session's spend as the transcript's header writes it: tokens, and cost when the agent reported one. A cost it did not
/// report stays absent.
String? spendText(UsageSpend? spend) {
  if (spend == null) return null;
  final tokens = spend.input + spend.output;
  return [
    if (tokens > 0) '${compactCount(tokens)} tokens',
    if (spend.costMicroUsd != null) dollars(spend.costMicroUsd!),
  ].join(' · ');
}

/// What a session has used, in a line: the context's fraction, tokens and cost as the agent said them. Null when it said none.
String? describeUsage(SessionUsage usage, {UsageSpend? spend}) {
  final spent = spendText(spend);
  final used = usage.used, size = usage.size, cost = usage.cost;
  final parts = <String>[
    if (used != null && size != null && size > 0) '${(100 * used / size).clamp(0, 100).round()}% of context',
    if (spent != null && spent.isNotEmpty) spent,
    if (cost != null && spend?.costMicroUsd == null) '${cost.toStringAsFixed(2)} ${usage.currency ?? ''}'.trim(),
  ];
  return parts.isEmpty ? null : parts.join(' · ');
}
