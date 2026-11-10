/// What the hosts say they have loaded: each provider with the schema and form of its own
/// configuration, as the gateway merges them (`GET /v1/user/providers`).
library;

import 'package:flutter/foundation.dart';

/// One provider, the hosts that have it, and what draws its settings.
@immutable
class ReportedProvider {
  const ReportedProvider({
    required this.id,
    required this.name,
    required this.kinds,
    required this.hosts,
    this.schema,
    this.form,
    this.unavailable,
    this.schemaOmitted = false,
  });

  final String id;
  final String name;

  /// The Guatiao kinds it serves, such as `ai6_safehouse`.
  final List<String> kinds;

  /// The Stations (and `gateway`) that have it loaded.
  final List<String> hosts;

  /// The JSON Schema of its configuration and Guatiao's form over it; absent when it takes none.
  final Map<String, Object?>? schema;
  final Map<String, Object?>? form;

  /// Why it cannot run on the host that reported it, when it said.
  final String? unavailable;

  /// The host left the schema out to keep its report small.
  final bool schemaOmitted;

  /// Whether the provider takes settings at all.
  bool get hasSettings => schema != null;

  factory ReportedProvider.fromJson(Map<String, dynamic> json) =>
      ReportedProvider(
        id: json['id'] as String,
        name: json['name'] as String? ?? json['id'] as String,
        kinds: ((json['kinds'] as List<dynamic>?) ?? const []).cast<String>(),
        hosts: ((json['hosts'] as List<dynamic>?) ?? const []).cast<String>(),
        schema: (json['schema'] as Map<String, dynamic>?)
            ?.cast<String, Object?>(),
        form: (json['form'] as Map<String, dynamic>?)?.cast<String, Object?>(),
        unavailable: json['unavailable'] as String?,
        schemaOmitted: json['schema_omitted'] as bool? ?? false,
      );
}

/// Every provider the hosts report.
@immutable
class ProvidersReport {
  const ProvidersReport({
    this.providers = const [],
    this.hosts = const [],
    this.differs = const [],
  });

  final List<ReportedProvider> providers;

  /// The hosts that reported.
  final List<String> hosts;

  /// Providers whose schema is not the same on every host that has them.
  final List<String> differs;

  /// The providers serving the Guatiao kind [kind], by ID.
  List<ReportedProvider> serving(String kind) => [
    for (final provider in providers)
      if (provider.kinds.contains(kind)) provider,
  ]..sort((a, b) => a.id.compareTo(b.id));

  /// The provider [id] of [kind], when a host reports it.
  ReportedProvider? find(String kind, String id) {
    for (final provider in serving(kind)) {
      if (provider.id == id) return provider;
    }
    return null;
  }

  factory ProvidersReport.fromJson(
    Map<String, dynamic> json,
  ) => ProvidersReport(
    providers: [
      for (final provider in (json['providers'] as List<dynamic>? ?? const []))
        ReportedProvider.fromJson(provider as Map<String, dynamic>),
    ],
    hosts: ((json['hosts'] as List<dynamic>?) ?? const []).cast<String>(),
    differs: ((json['differs'] as List<dynamic>?) ?? const []).cast<String>(),
  );
}
