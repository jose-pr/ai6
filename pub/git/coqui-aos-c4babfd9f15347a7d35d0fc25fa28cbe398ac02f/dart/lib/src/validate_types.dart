/// The shapes validation reports in, and the small readers it shares.
library;

import 'dart:convert';

import 'kinds.dart';

/// What is wrong with a manifest, by the class the specification's vectors name it.
const List<String> issueCodes = [
  'env-name-case-duplicate',
  'env-path-conflict',
  'integer-required',
  'invalid-source',
  'license-expression',
  'local-source',
  'manifest-file',
  'mcp-command',
  'mcp-duplicate-server',
  'mcp-duplicate-skill',
  'mcp-unknown-server',
  'mcp-url',
  'missing-file',
  'missing-skill',
  'option-default-type',
  'option-sensitive-default',
  'ref-not-allowed',
  'reserved-name',
  'reserved-variable',
  'schema',
  'skill-ambiguous',
  'skill-case',
  'skill-duplicate',
  'skill-name',
  'source-credentials',
  'two-manifests',
];

/// The words of an issue code: `issueCode.named('schema')`.
const Words issueCode = Words(issueCodes);

/// One problem with a manifest: where, as a JSON pointer such as `/env/PATH`, and what.
class Issue {
  /// Creates the issue.
  const Issue(this.at, this.code);

  /// The JSON pointer to the offending value.
  final String at;

  /// The issue code, a word of [issueCodes].
  final String code;

  @override
  String toString() => '$code at $at';
}

/// One reference token of a JSON Pointer: `~` is `~0` and `/` is `~1`.
String token(String text) => text.replaceAll('~', '~0').replaceAll('/', '~1');

/// A pointer extended by [key].
String join(String at, Object key) => '$at/${token('$key')}';

/// A value as text that tells an integer from a float and keeps a mapping's order.
String canon(Object? value) {
  if (value == null) return 'none';
  if (value is String) return 's${jsonEncode(value)}';
  if (value is BigInt) return 'i$value';
  if (value is double) return 'f$value';
  if (value is bool) return 'b$value';
  if (value is List<Object?>) return '[${value.map(canon).join(',')}]';
  if (value is Map<String, Object?>) {
    return '{${value.entries.map((e) => '${jsonEncode(e.key)}:${canon(e.value)}').join(',')}}';
  }
  return 'none';
}

/// An entry of a mapping, or null when [value] is no mapping.
Object? member(Object? value, String key) =>
    value is Map<String, Object> ? value[key] : null;

/// A mapping's entry that is text.
String? memberText(Object? value, String key) {
  final found = member(value, key);
  return found is String ? found : null;
}

/// The list a manifest field holds, or none.
List<Object> declared(Object? manifest, String field) {
  final found = member(manifest, field);
  return found is List<Object> ? found : const [];
}
