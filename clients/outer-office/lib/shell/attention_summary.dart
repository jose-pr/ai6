import 'package:flutter/foundation.dart';

import '../routes/oo_route.dart';

/// Whether a condition holds, as the gateway judged it from the Station's report and the audit
/// log.
enum AttentionCondition {
  /// It holds now, as of the check.
  current,

  /// It happened and has since passed.
  historical,

  /// It was seen, and whether it still holds could not be checked.
  unverified;

  static AttentionCondition named(Object? name) => switch (name) {
    'historical' => historical,
    'unverified' => unverified,
    _ => current,
  };
}

/// One thing that needs the person, or did, as `GET /v1/user/attention` says it.
@immutable
class AttentionEntry {
  const AttentionEntry({
    required this.id,
    required this.kind,
    required this.condition,
    required this.title,
    required this.scope,
    required this.checkedAt,
    this.detail,
    this.entityKind,
    this.entityId,
    this.route,
    this.observedAt,
    this.since,
    this.editor,
    this.revision,
  });

  factory AttentionEntry.fromJson(Map<String, dynamic> json) {
    final entity = json['entity'] as Map<String, dynamic>?;
    OoRoute? route;
    OoRoute? editor;
    try {
      if (json['route'] case final Object value) {
        route = OoRoute.fromJson(value);
      }
    } on FormatException {
      route = null;
    }
    try {
      if (json['editor'] case final Object value) {
        editor = OoRoute.fromJson(value);
      }
    } on FormatException {
      editor = null;
    }
    return AttentionEntry(
      id: json['id'] as String,
      kind: json['kind'] as String,
      condition: AttentionCondition.named(json['condition']),
      title: json['title'] as String,
      detail: json['detail'] as String?,
      scope: json['scope'] as String? ?? '',
      checkedAt: (json['checked_at'] as num).toDouble(),
      observedAt: (json['observed_at'] as num?)?.toDouble(),
      since: (json['since'] as num?)?.toDouble(),
      entityKind: entity?['kind'] as String?,
      entityId: entity?['id'] as String?,
      route: route,
      editor: editor,
      revision: json['revision'] as String?,
    );
  }

  final String id;

  /// `blocked_run`, `green_ink`, `held_brief`, `mission_proposal`, `station_dark`,
  /// `config_refused`, `report`, `charge_failed`, `charge_lapsed` or `login_needs_person`.
  final String kind;
  final AttentionCondition condition;
  final String title;

  /// Why the condition is what it is, when there is more to say.
  final String? detail;

  /// What was checked: `operation:G` or `station:NAME`.
  final String scope;

  /// When the gateway read its source, as Unix seconds.
  final double checkedAt;

  /// When the Station reported what the item rests on, as Unix seconds.
  final double? observedAt;

  /// When the condition began or happened, as Unix seconds.
  final double? since;
  final String? entityKind;
  final String? entityId;

  /// The page that deals with it.
  final OoRoute? route;

  /// For a refused configuration, the record editor on that Station's own records, where a
  /// change is reviewed and saved.
  final OoRoute? editor;

  /// The configuration revision a refusal names.
  final String? revision;
}

/// How one source of the summary was read.
@immutable
class AttentionCoverage {
  const AttentionCoverage({
    required this.source,
    required this.label,
    required this.status,
    this.unavailable = const [],
    this.notPermitted = const [],
  });

  factory AttentionCoverage.fromJson(Map<String, dynamic> json) =>
      AttentionCoverage(
        source: json['source'] as String,
        label: json['label'] as String,
        status: json['status'] as String,
        unavailable: [
          for (final scope in json['unavailable'] as List<dynamic>? ?? const [])
            scope as String,
        ],
        notPermitted: [
          for (final scope
              in json['not_permitted'] as List<dynamic>? ?? const [])
            scope as String,
        ],
      );

  final String source;
  final String label;

  /// `checked`, `partial`, `not_permitted` or `unavailable`.
  final String status;

  /// Scopes that could not be read now.
  final List<String> unavailable;

  /// Scopes that are not the person's to read.
  final List<String> notPermitted;
}

/// What needs the signed-in person, as the gateway derived it once for the Overview page and
/// for Moneypenny alike.
@immutable
class AttentionSummary {
  const AttentionSummary({
    required this.checkedAt,
    required this.scopeLabel,
    required this.items,
    required this.coverage,
    required this.complete,
    required this.headline,
    this.omitted = 0,
  });

  factory AttentionSummary.fromJson(Map<String, dynamic> json) =>
      AttentionSummary(
        checkedAt: (json['checked_at'] as num).toDouble(),
        scopeLabel:
            (json['scope'] as Map<String, dynamic>)['label'] as String? ?? '',
        items: [
          for (final item in json['items'] as List<dynamic>)
            AttentionEntry.fromJson(item as Map<String, dynamic>),
        ],
        coverage: [
          for (final read in json['coverage'] as List<dynamic>)
            AttentionCoverage.fromJson(read as Map<String, dynamic>),
        ],
        complete: json['complete'] as bool? ?? false,
        headline: json['headline'] as String? ?? '',
        omitted: json['omitted'] as int? ?? 0,
      );

  /// When the gateway made the checks, as Unix seconds.
  final double checkedAt;

  /// "All accessible Operations" or "Operation G".
  final String scopeLabel;

  /// Current and unverified items first, historical last.
  final List<AttentionEntry> items;
  final List<AttentionCoverage> coverage;

  /// Whether every source was read.
  final bool complete;

  /// The gateway's one-sentence answer, which Moneypenny gets as well.
  final String headline;

  /// Items found past the configured count, left out.
  final int omitted;

  /// What needs the person now or may: current and unverified items.
  List<AttentionEntry> get open => [
    for (final item in items)
      if (item.condition != AttentionCondition.historical) item,
  ];

  /// Conditions that have passed.
  List<AttentionEntry> get past => [
    for (final item in items)
      if (item.condition == AttentionCondition.historical) item,
  ];

  /// The sources that could not be read, named, or null when all were.
  String? get gaps {
    final missing = [
      for (final read in coverage)
        if (read.unavailable.isNotEmpty) read.label,
    ];
    return missing.isEmpty ? null : 'Could not be read: ${missing.join(', ')}.';
  }

  /// The sources that are not the person's to see, named, or null when there are none.
  String? get hidden {
    final names = [
      for (final read in coverage)
        if (read.notPermitted.isNotEmpty) read.label,
    ];
    return names.isEmpty
        ? null
        : 'Not visible to you, so not included: ${names.join(', ')}.';
  }
}
