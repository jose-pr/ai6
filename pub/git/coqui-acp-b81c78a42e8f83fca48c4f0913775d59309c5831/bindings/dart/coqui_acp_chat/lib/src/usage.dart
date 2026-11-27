import 'limits.dart';
import 'text.dart';

// How much of each usage allowance an account has used and when it refills: coqui-acp's usage model (`coqui_acp_api::usage`,
// held to the same vectors in `vectors/usage.json`). Words for a person to read it in are the UI's.

/// The `_meta` key of the usage extension in the capabilities of an agent and of a client.
const String usageCapability = 'dev.boriken.coqui.acp/usage';

/// The extension's request, and the notification an agent sends when a report changes.
const String usageMethod = '_coqui_acp/usage';
const String usageNotification = '_coqui_acp/usage_update';

/// A kind of figure a bucket is counted in: a name this app knows, or the name a newer writer sent, kept as it came.
final class UsageUnit {
  const UsageUnit(this.name);

  static const tokens = UsageUnit('tokens');
  static const requests = UsageUnit('requests');
  static const microUsd = UsageUnit('micro_usd');

  /// A percentage only: the limit is 1000 and the used figure thousandths.
  static const permille = UsageUnit('permille');

  final String name;

  /// Whether this reader does not know the unit, and so treats the bucket's figures as unitless.
  bool get isOther =>
      this != tokens &&
      this != requests &&
      this != microUsd &&
      this != permille;

  @override
  bool operator ==(Object other) => other is UsageUnit && other.name == name;

  @override
  int get hashCode => name.hashCode;
}

/// Which models a bucket covers. `unknown` counts as covering every model.
final class UsageCoverage {
  const UsageCoverage(this.kind, [this.models = const []]);

  static const all = UsageCoverage('all');
  static const unknown = UsageCoverage('unknown');
  static const other = UsageCoverage('other');

  /// `all`, `unknown`, `only` or `other` (a kind a newer writer named).
  final String kind;
  final List<String> models;

  Map<String, Object?> toJson() => {
    'kind': kind,
    if (kind == 'only') 'models': models,
  };
}

/// How a bucket refills.
final class UsageRefill {
  const UsageRefill(
    this.kind, {
    this.length,
    this.resetsAt,
    this.amount,
    this.per,
    this.max,
  });

  /// `fixed_window` (all at once when the window ends), `token_bucket` (continuously), `none`, `unknown` or `other`.
  final String kind;
  final Duration? length;

  /// Unix seconds.
  final int? resetsAt;
  final int? amount;
  final Duration? per;
  final int? max;

  bool get isFixedWindow => kind == 'fixed_window';
  bool get isTokenBucket =>
      kind == 'token_bucket' && amount != null && per != null;

  Map<String, Object?> toJson() => switch (kind) {
    'fixed_window' => {
      'kind': kind,
      if (length != null) 'length': _duration(length!),
      if (resetsAt != null) 'resets_at': resetsAt,
    },
    'token_bucket' => {
      'kind': kind,
      'amount': amount,
      'per': _duration(per ?? Duration.zero),
      'max': max,
    },
    _ => {'kind': kind},
  };
}

/// One allowance: what it counts, how much of it is used and how it refills.
final class UsageBucket {
  const UsageBucket({
    required this.id,
    this.label,
    this.models = UsageCoverage.unknown,
    this.unit = UsageUnit.permille,
    this.used,
    this.limit,
    this.remaining,
    this.refill = const UsageRefill('unknown'),
    this.status = 'unknown',
    this.observedAt = 0,
    this.source = 'reported',
    this.evidence,
  });

  final String id;
  final String? label;
  final UsageCoverage models;
  final UsageUnit unit;
  final int? used, limit, remaining;
  final UsageRefill refill;

  /// `ok`, `warning`, `exhausted`, `unknown`, or a name a newer writer sent.
  final String status;

  /// Unix seconds.
  final int observedAt;

  /// `reported` (the harness's own figure) or `inferred` (read from an error's words, a 429 or a refusal).
  final String source;
  final String? evidence;

  bool get inferred => source == 'inferred';
  bool get isExhausted => status == 'exhausted';

  /// What the bucket is called: its own label, else its id in words.
  String get name => label ?? id.replaceAll('_', ' ');

  /// How much is used, in thousandths and at most 1000: used over limit, else limit minus remaining over limit; null without
  /// a limit. Integer arithmetic, so a count as large as a double holds exactly gives the same answer on every platform.
  int? get fractionPermille {
    final limit = this.limit;
    if (limit == null || limit <= 0) return null;
    final int used;
    if (this.used case final taken?) {
      used = taken;
    } else if (remaining case final left?) {
      used = left >= limit ? 0 : limit - left;
    } else {
      return null;
    }
    if (used >= limit) return 1000;
    return (used * 1000) ~/ limit;
  }

  /// Seconds until a fixed window ends, zero once it has; null for any other refill or a window with no known end.
  int? resetsIn(int now) {
    final end = refill.resetsAt;
    if (!refill.isFixedWindow || end == null) return null;
    return end > now ? end - now : 0;
  }

  /// A fixed window whose end has passed: the figures are of the window before, and a new report is due.
  bool pastReset(int now) => resetsIn(now) == 0;

  Map<String, Object?> toJson() => {
    'id': id,
    if (label != null) 'label': label,
    'models': models.toJson(),
    'unit': unit.name,
    if (used != null) 'used': used,
    if (limit != null) 'limit': limit,
    if (remaining != null) 'remaining': remaining,
    'refill': refill.toJson(),
    'status': status,
    'observed_at': observedAt,
    'source': source,
    if (evidence != null) 'evidence': evidence,
  };
}

/// A refused request: the bucket it names, when the allowance comes back and what the refusal said.
final class UsageLimitHit {
  const UsageLimitHit({
    this.bucket,
    this.resetsAt,
    this.retryAfter,
    this.source = 'reported',
    this.text,
    this.observedAt = 0,
  });

  final String? bucket;
  final int? resetsAt;
  final Duration? retryAfter;
  final String source;
  final String? text;
  final int observedAt;

  Map<String, Object?> toJson() => {
    if (bucket != null) 'bucket': bucket,
    if (resetsAt != null) 'resets_at': resetsAt,
    if (retryAfter != null) 'retry_after': _duration(retryAfter!),
    'source': source,
    if (text != null) 'text': text,
    'observed_at': observedAt,
  };
}

/// What one model of a session has used.
final class UsageModelSpend {
  const UsageModelSpend(
    this.model, {
    this.input = 0,
    this.output = 0,
    this.cacheRead = 0,
    this.cacheWrite = 0,
    this.costMicroUsd,
  });

  final String model;
  final int input, output, cacheRead, cacheWrite;

  /// Millionths of a US dollar; null when the harness reported none, never 0 for unknown.
  final int? costMicroUsd;

  Map<String, Object?> toJson() => {
    'model': model,
    'input': input,
    'output': output,
    'cache_read': cacheRead,
    'cache_write': cacheWrite,
    if (costMicroUsd != null) 'cost_micro_usd': costMicroUsd,
  };
}

/// A session's running totals.
final class UsageSpend {
  const UsageSpend(
    this.sessionId, {
    this.input = 0,
    this.output = 0,
    this.cacheRead = 0,
    this.cacheWrite = 0,
    this.reasoning,
    this.costMicroUsd,
    this.byModel = const [],
  });

  final String sessionId;
  final int input, output, cacheRead, cacheWrite;

  /// Part of [output], when the harness separates it.
  final int? reasoning;
  final int? costMicroUsd;
  final List<UsageModelSpend> byModel;

  Map<String, Object?> toJson() => {
    'session_id': sessionId,
    'input': input,
    'output': output,
    'cache_read': cacheRead,
    'cache_write': cacheWrite,
    if (reasoning != null) 'reasoning': reasoning,
    if (costMicroUsd != null) 'cost_micro_usd': costMicroUsd,
    'by_model': [for (final model in byModel) model.toJson()],
  };
}

/// What an account has used: [supported] false is a real answer (this agent reports none); supported with no buckets is
/// another, nothing reported yet.
final class UsageReport {
  const UsageReport({
    required this.supported,
    this.unsupportedBecause,
    this.sources = const [],
    this.account,
    this.revision = 0,
    this.observedAt = 0,
    this.buckets = const [],
    this.limit,
    this.spend,
  });

  final bool supported;
  final String? unsupportedBecause;

  /// Which kinds of figure the agent can give at all: `reported` and `inferred`.
  final List<String> sources;

  /// The host's id for the account, which the agent never interprets.
  final String? account;

  /// Grows with each change on a connection, so a reader keeps the newest.
  final int revision;
  final int observedAt;
  final List<UsageBucket> buckets;
  final UsageLimitHit? limit;
  final UsageSpend? spend;

  /// The report as the extension writes it: an absent optional left out, a list always written.
  Map<String, Object?> toJson() => {
    'supported': supported,
    if (unsupportedBecause != null) 'unsupported_because': unsupportedBecause,
    'sources': sources,
    if (account != null) 'account': account,
    'revision': revision,
    'observed_at': observedAt,
    'buckets': [for (final bucket in buckets) bucket.toJson()],
    if (limit != null) 'limit': limit!.toJson(),
    if (spend != null) 'spend': spend!.toJson(),
  };

  /// This report with [spend] in place of its own.
  UsageReport withSpend(UsageSpend? spend) => UsageReport(
    supported: supported,
    unsupportedBecause: unsupportedBecause,
    sources: sources,
    account: account,
    revision: revision,
    observedAt: observedAt,
    buckets: buckets,
    limit: limit,
    spend: spend,
  );

  /// Reads a report leniently, as the model's JSON spells it: what a newer writer added is ignored, and a unit, status, source,
  /// refill or coverage it named reads as its `other` and the report is still read. Null when [json] is not a report at all.
  static UsageReport? parse(
    Object? json, [
    ChatLimits limits = const ChatLimits(),
  ]) {
    if (json is! Map || json['supported'] is! bool) return null;
    return UsageReport(
      supported: json['supported'] as bool,
      unsupportedBecause: _text(json['unsupported_because'], limits),
      sources: _texts(json['sources'], limits),
      account: _text(json['account'], limits),
      revision: _count(json['revision']) ?? 0,
      observedAt: _count(json['observed_at']) ?? 0,
      buckets: [
        for (final bucket in _list(json['buckets'], limits))
          ?_bucket(bucket, limits),
      ],
      limit: _limit(json['limit'], limits),
      spend: _spend(json['spend'], limits),
    );
  }
}

/// The agent's capability for the extension, from its `initialize` answer's `_meta`; null when it advertises none.
final class UsageSupport {
  const UsageSupport({
    this.version = 1,
    this.sources = const [],
    this.spend = false,
    this.needsTurn = false,
  });

  final int version;
  final List<String> sources;

  /// Whether it reports a session's spend.
  final bool spend;

  /// Whether it learns usage only during a turn, so a read gives what the connection last saw.
  final bool needsTurn;

  static UsageSupport? parse(
    Object? meta, [
    ChatLimits limits = const ChatLimits(),
  ]) {
    final said = meta is Map ? meta[usageCapability] : null;
    if (said is! Map) return null;
    return UsageSupport(
      version: _count(said['version']) ?? 1,
      sources: _texts(said['sources'], limits),
      spend: said['spend'] == true,
      needsTurn: said['needs_turn'] == true,
    );
  }
}

List<Object?> _duration(Duration length) => [
  length.inSeconds,
  (length.inMicroseconds % 1000000) * 1000,
];

Duration? _length(Object? json) {
  if (json is! List || json.length != 2) return null;
  final seconds = _count(json[0]), nanos = _count(json[1]);
  if (seconds == null || nanos == null || seconds > 1099511627776) return null;
  return Duration(seconds: seconds, microseconds: nanos ~/ 1000);
}

// A figure an agent sends is a whole number every platform holds exactly: not NaN, not infinite (which a browser counts as an int),
// and below 2^53.
int? _count(Object? value) {
  if (value is! num || !value.isFinite || value.abs() >= 9007199254740992) {
    return null;
  }
  if (value is int) return value;
  return value == value.truncateToDouble() ? value.toInt() : null;
}

// Every string an agent sends is shown to a person, so it is one clean line cut at ChatLimits.shortText, and every list is cut at
// ChatLimits.usageBuckets.
String? _text(Object? value, ChatLimits limits) => shortText(value, limits);

List<String> _texts(Object? value, ChatLimits limits) => [
  for (final item in (value is List ? value : const []).take(
    limits.usageBuckets,
  ))
    ?_text(item, limits),
];

List<Object?> _list(Object? value, ChatLimits limits) =>
    (value is List ? value : const []).take(limits.usageBuckets).toList();

UsageBucket? _bucket(Object? json, ChatLimits limits) {
  if (json is! Map) return null;
  final id = _text(json['id'], limits);
  if (id == null) return null;
  final models = json['models'];
  final refill = json['refill'];
  return UsageBucket(
    id: id,
    label: _text(json['label'], limits),
    models: models is Map
        ? switch (models['kind']) {
            'all' => UsageCoverage.all,
            'unknown' => UsageCoverage.unknown,
            'only' => UsageCoverage('only', _texts(models['models'], limits)),
            _ => UsageCoverage.other,
          }
        : UsageCoverage.unknown,
    unit: UsageUnit(_text(json['unit'], limits) ?? 'other'),
    used: _count(json['used']),
    limit: _count(json['limit']),
    remaining: _count(json['remaining']),
    refill: refill is Map
        ? switch (refill['kind']) {
            'fixed_window' => UsageRefill(
              'fixed_window',
              length: _length(refill['length']),
              resetsAt: _count(refill['resets_at']),
            ),
            'token_bucket' => UsageRefill(
              'token_bucket',
              amount: _count(refill['amount']),
              per: _length(refill['per']),
              max: _count(refill['max']),
            ),
            'none' => const UsageRefill('none'),
            'unknown' => const UsageRefill('unknown'),
            _ => const UsageRefill('other'),
          }
        : const UsageRefill('unknown'),
    status: _text(json['status'], limits) ?? 'unknown',
    observedAt: _count(json['observed_at']) ?? 0,
    source: _text(json['source'], limits) ?? 'other',
    evidence: _text(json['evidence'], limits),
  );
}

UsageLimitHit? _limit(Object? json, ChatLimits limits) {
  if (json is! Map) return null;
  return UsageLimitHit(
    bucket: _text(json['bucket'], limits),
    resetsAt: _count(json['resets_at']),
    retryAfter: _length(json['retry_after']),
    source: _text(json['source'], limits) ?? 'other',
    text: _text(json['text'], limits),
    observedAt: _count(json['observed_at']) ?? 0,
  );
}

UsageSpend? _spend(Object? json, ChatLimits limits) {
  if (json is! Map) return null;
  final session = _text(json['session_id'], limits);
  if (session == null) return null;
  return UsageSpend(
    session,
    input: _count(json['input']) ?? 0,
    output: _count(json['output']) ?? 0,
    cacheRead: _count(json['cache_read']) ?? 0,
    cacheWrite: _count(json['cache_write']) ?? 0,
    reasoning: _count(json['reasoning']),
    costMicroUsd: _count(json['cost_micro_usd']),
    byModel: [
      for (final model in _list(json['by_model'], limits))
        if (model is Map && _text(model['model'], limits) != null)
          UsageModelSpend(
            _text(model['model'], limits)!,
            input: _count(model['input']) ?? 0,
            output: _count(model['output']) ?? 0,
            cacheRead: _count(model['cache_read']) ?? 0,
            cacheWrite: _count(model['cache_write']) ?? 0,
            costMicroUsd: _count(model['cost_micro_usd']),
          ),
    ],
  );
}
