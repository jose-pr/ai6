import 'dart:convert';

import 'oo_settings.g.dart';

/// `[oo]` as its declaration states it ([ooSettingsJson]): which tables and keys there are, what
/// each holds, and its default. Plain JSON, so a file can be read before `guatiao_intake` is
/// loaded, which on the web comes after the file; the settings page's form goes through it.
abstract final class OoSchema {
  static final Map<String, Object?> _declared =
      (jsonDecode(ooSettingsJson) as Map).cast<String, Object?>();

  /// The JSON Schema of `[oo]`.
  static Map<String, Object?> get schema =>
      (_declared['schema'] as Map).cast<String, Object?>();

  /// The form over [schema].
  static Map<String, Object?> get form =>
      (_declared['form'] as Map).cast<String, Object?>();

  static Map<String, Object?> _properties(Map<String, Object?> node) =>
      (node['properties'] as Map).cast<String, Object?>();

  /// The tables of `[oo]`.
  static Set<String> get tables => _properties(schema).keys.toSet();

  static Map<String, Object?> _table(String table) =>
      (_properties(schema)[table] as Map).cast<String, Object?>();

  /// The keys of `[oo.table]`.
  static Set<String> keysOf(String table) =>
      _properties(_table(table)).keys.toSet();

  /// The schema of `[oo.table] key`, an optional value's arm with the words kept.
  static Map<String, Object?> field(String table, String key) {
    final node = (_properties(_table(table))[key] as Map)
        .cast<String, Object?>();
    final arms = node['anyOf'];
    if (arms is! List) return node;
    final held = arms.whereType<Map>().firstWhere(
      (arm) => arm['type'] != 'null',
    );
    return {...held.cast<String, Object?>(), ...node}..remove('anyOf');
  }

  /// The default of `[oo.table] key`, `null` for one that has none.
  static Object? defaultOf(String table, String key) =>
      field(table, key)['default'];

  /// What is wrong with [value] at `[oo.table] key`, worded to follow its name; `null` when it
  /// fits. It never quotes the value.
  static String? refusal(String table, String key, Object? value) {
    final node = field(table, key);
    final minimum = node['minimum'] as num?;
    final type = node['type'];
    switch (type is List ? type.firstWhere((one) => one != 'null') : type) {
      case 'integer':
        final least = minimum ?? 0;
        return value is int && value >= least
            ? null
            : 'is not a whole number of $least or more';
      case 'number':
        final seconds = node['format'] == 'seconds';
        final positive = minimum != null && minimum > 0;
        if (value is num && value.isFinite && value >= (minimum ?? 0)) {
          return null;
        }
        if (!seconds) return 'is not a positive number';
        return 'is not a number of seconds, ${positive ? 'more than 0' : '0 or more'}';
      case 'string':
        return value is String ? null : 'is not text';
      default:
        return null;
    }
  }

  /// [oo] without what its default says already, or an empty table: what a file need hold.
  static Map<String, Object?> withoutDefaults(Map<String, Object?> oo) => {
    for (final MapEntry(key: table, value: keys) in oo.entries)
      if (keys is Map && tables.contains(table))
        if (_changed(table, keys.cast<String, Object?>()) case final kept
            when kept.isNotEmpty)
          table: kept,
  };

  static Map<String, Object?> _changed(
    String table,
    Map<String, Object?> keys,
  ) => {
    for (final MapEntry(:key, :value) in keys.entries)
      if (value != null && value != defaultOf(table, key)) key: value,
  };
}
