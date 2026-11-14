import 'package:flutter/foundation.dart' show immutable;

/// One line of a charge's logbook.
@immutable
class ChargeLogLine {
  const ChargeLogLine({
    required this.at,
    required this.kind,
    required this.text,
  });

  factory ChargeLogLine.fromJson(Map<String, dynamic> json) => ChargeLogLine(
    at: (json['at'] as num).toDouble(),
    kind: json['kind'] as String,
    text: json['text'] as String,
  );

  final double at;

  /// `turn`, `started`, `resumed`, `fresh`, `quota`, `station` or `agent`.
  final String kind;
  final String text;
}

/// A charge as Universal Exports lists it: who keeps it, where it runs, when it last woke, and
/// what the person may do with it.
@immutable
class ChargeSummary {
  const ChargeSummary({
    required this.scope,
    required this.held,
    required this.turnsLastHour,
    required this.canConverse,
    required this.canInterrupt,
    this.agent,
    this.handler,
    this.model,
    this.routes = 0,
    this.configured = false,
    this.station,
    this.lastWake,
    this.lastTurn,
    this.leaseExpires,
    this.logbook = const [],
  });

  factory ChargeSummary.fromJson(Map<String, dynamic> json) {
    final route = json['route'] as Map<String, dynamic>?;
    return ChargeSummary(
      scope: json['scope'] as String,
      agent: json['agent'] as String?,
      handler: route?['handler'] as String?,
      model: route?['model'] as String?,
      routes: (route?['routes'] as num?)?.toInt() ?? 0,
      configured: json['configured'] as bool? ?? false,
      station: json['station'] as String?,
      held: json['held'] as bool? ?? false,
      leaseExpires: (json['lease_expires'] as num?)?.toDouble(),
      lastWake: (json['last_wake'] as num?)?.toDouble(),
      lastTurn: json['last_turn'] as String?,
      turnsLastHour: (json['turns_last_hour'] as num?)?.toInt() ?? 0,
      canConverse: json['can_converse'] as bool? ?? false,
      canInterrupt: json['can_interrupt'] as bool? ?? false,
      logbook: [
        for (final line in json['logbook'] as List<dynamic>? ?? const [])
          ChargeLogLine.fromJson(line as Map<String, dynamic>),
      ],
    );
  }

  /// `m`, `operation:<codename>` or `station:<id>`.
  final String scope;
  final String? agent;
  final String? handler;
  final String? model;

  /// How many routes the Agent has.
  final int routes;
  final bool configured;

  /// The Station holding its lease; `null` when none does.
  final String? station;

  /// Whether a Station holds the charge's lease: with none, nothing runs it.
  final bool held;
  final double? leaseExpires;
  final double? lastWake;

  /// What its last turn recorded: what woke it, and the start of what it said.
  final String? lastTurn;
  final int turnsLastHour;

  /// Whether the person may attach to it, type to it and answer it.
  final bool canConverse;

  /// Whether the person may interrupt its turn.
  final bool canInterrupt;

  /// Its logbook, oldest first; only in the detail read.
  final List<ChargeLogLine> logbook;

  /// Whose charge it is: `M`, `Operation G` or `Station worker`.
  String get title => switch (scope.split(':')) {
    ['m'] => 'M',
    ['operation', ...final rest] => 'Operation ${rest.join(':')}',
    ['station', ...final rest] => 'Station ${rest.join(':')}',
    _ => scope,
  };

  /// `m`, `operation` or `station`.
  String get kind => scope.split(':').first;

  /// The Operation the charge keeps; `null` for M's and a Station's.
  String? get operationId =>
      kind == 'operation' ? scope.substring('operation:'.length) : null;

  /// The Station the charge keeps; `null` for M's and an Operation's.
  String? get keptStationId =>
      kind == 'station' ? scope.substring('station:'.length) : null;

  /// The Run's state as the list can know it. Whether a turn is under way shows in the live
  /// view, which hears it from the Run.
  String get state => held ? 'running' : 'stopped';

  /// The Agent and the model it reaches, as one line.
  String get agentLine {
    final who = agent ?? 'No Agent';
    final how = [?handler, ?model].join(' · ');
    final more = routes > 1 ? ' (+${routes - 1} more routes)' : '';
    return how.isEmpty ? who : '$who · $how$more';
  }
}

/// The route's fields for the charge [scope], the inverse of [ChargeSummary.scope].
Map<String, String> chargeRouteFields(String scope) =>
    switch (scope.split(':')) {
      ['m'] => const {'charge': 'm'},
      ['operation', ...final rest] => {'operation_id': rest.join(':')},
      ['station', ...final rest] => {'station_id': rest.join(':')},
      _ => const {},
    };
