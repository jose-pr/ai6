import '../forms/form_host.dart';

/// A line about what a record holds, in its fields' own titles: never its format. Fields left at
/// their default are not named.
List<String> summaryOf(
  Map<String, Object?> schema,
  Map<String, Object?> value, {
  int fields = 4,
}) {
  final out = <String>[];
  final at = FieldSchema(schema);
  String? shown(FieldSchema field, Object? held) => switch (held) {
    null => null,
    final bool on => on ? 'yes' : 'no',
    final String text when field.choices != null => field.labelOf(text),
    final String text when field.sensitive => text.isEmpty ? null : 'set',
    final String text => text.isEmpty ? null : text,
    final num number => '$number',
    final List<Object?> list when list.isEmpty => null,
    final List<Object?> list =>
      list.every((item) => item is String || item is num)
          ? list.join(', ')
          : '${list.length}',
    _ => null,
  };
  final properties = at.node['properties'];
  if (properties is! Map) return out;
  for (final advanced in [false, true]) {
    for (final key in properties.keys.cast<String>()) {
      if (out.length >= fields) return out;
      final field = at.member(key)!;
      if (field.advanced != advanced || field.sensitive) continue;
      final text = shown(field, value[key]);
      if (text != null) {
        final line = text.length > 60 ? '${text.substring(0, 60)}…' : text;
        out.add('${field.title}: $line');
      }
    }
  }
  return out;
}
