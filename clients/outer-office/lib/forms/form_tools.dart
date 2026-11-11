/// Working with a large record: narrowing a form to the settings a person looks for, to those
/// that differ from their defaults, and reading what a save would change.
library;

import 'package:guatiao/guatiao.dart' as guatiao;
import 'package:guatiao_flutter/guatiao_flutter.dart';

/// A [FormModel] that can be narrowed: to the settings whose name or help holds a text, to those
/// that differ from their defaults, and without the sections folded away.
///
/// Narrowing only decides which fields are drawn; every value stays in the model, so a saved
/// record is the same whatever was showing.
class OoFormModel extends FormModel {
  // dart2js 3.15.0-72.0.dev crashes inlining FormModel's initializers into a constructor that
  // forwards with super parameters; an explicit, never-inlined forward compiles.
  @pragma('dart2js:noInline')
  // ignore: use_super_parameters
  OoFormModel({
    required Map<String, Object?> schema,
    Map<String, Object?>? form,
    Object? value,
  }) : super(schema: schema, form: form, value: value);

  String _query = '';
  bool _modifiedOnly = false;
  final _folded = <String>{};
  final _matched = <String, bool>{};
  List<ModifiedField>? _modified;
  Map<String, String>? _sections;

  /// The words a field must hold, in its title, its help or its key; empty shows every field.
  String get query => _query;
  set query(String text) {
    final next = text.trim().toLowerCase();
    if (next == _query) return;
    _query = next;
    notifyListeners();
  }

  /// Whether only the settings that differ from their defaults are drawn.
  bool get modifiedOnly => _modifiedOnly;
  set modifiedOnly(bool on) {
    if (on == _modifiedOnly) return;
    _modifiedOnly = on;
    notifyListeners();
  }

  /// Whether a narrowing is in force.
  bool get narrowed => _query.isNotEmpty || _modifiedOnly;

  /// Whether the section `id` of the record is folded away.
  bool isFolded(String id) => _folded.contains(id);

  /// Folds the section `id` away, or draws it again.
  void toggleFolded(String id) {
    if (!_folded.remove(id)) _folded.add(id);
    notifyListeners();
  }

  @override
  void notifyListeners() {
    _matched.clear();
    _modified = null;
    super.notifyListeners();
  }

  @override
  bool isVisible(String path, String key, [FieldSchema? at]) {
    if (!super.isVisible(path, key, at)) return false;
    final full = memberPath(path, key);
    if (path.isEmpty && _folded.contains(sectionOf(key))) return false;
    if (!narrowed) return true;
    final member = at?.member(key) ?? field(full);
    if (member == null) return true;
    if (_modifiedOnly && !_isModified(full, member)) return false;
    if (_query.isEmpty) return true;
    return _matches(full, key, member) || _ancestorMatches(path);
  }

  /// The id of the top-level section that draws `key`; `''` for the default section.
  String sectionOf(String key) {
    final known = _sections ??= {
      for (final group in layout())
        for (final field in group.fields) field: group.id,
    };
    return known[key] ?? '';
  }

  bool _textMatches(String key, FieldSchema field) =>
      key.toLowerCase().contains(_query) ||
      field.title.toLowerCase().contains(_query) ||
      field.description.toLowerCase().contains(_query);

  /// Whether the field, or a field under it, holds the query.
  bool _matches(String full, String key, FieldSchema field) =>
      _matched.putIfAbsent(full, () => _anyTextMatches(key, field, 0));

  bool _anyTextMatches(String key, FieldSchema field, int depth) {
    if (_textMatches(key, field)) return true;
    if (depth > 8) return false;
    for (final member in field.memberKeys) {
      final inner = field.member(member);
      if (inner != null && _anyTextMatches(member, inner, depth + 1)) {
        return true;
      }
    }
    final below = field.items ?? field.values;
    return below != null && _anyTextMatches('', below, depth + 1);
  }

  /// Whether the group or entry at `path`, or one holding it, names the query: its fields are
  /// shown whole.
  bool _ancestorMatches(String path) {
    if (path.isEmpty) return false;
    final here = field(path);
    if (here != null && _textMatches('', here)) return true;
    return _ancestorMatches(parentOf(path));
  }

  /// Whether the field at `full` holds anything that differs from its default.
  bool _isModified(String full, FieldSchema field) =>
      modified.any((entry) => isAtOrUnder(entry.path, full));

  /// The leaf settings that differ from their defaults, in order.
  List<ModifiedField> get modified {
    final known = _modified;
    if (known != null) return known;
    final found = <ModifiedField>[];
    _collect(value(), schema, '', '', found);
    return _modified = found;
  }

  void _collect(
    Object? held,
    FieldSchema field,
    String full,
    String group,
    List<ModifiedField> into,
  ) {
    if (field.hasMembers) {
      if (held is! Map) return;
      for (final key in field.memberKeys) {
        final member = field.member(key);
        if (member == null) continue;
        _collect(
          held[key],
          member,
          memberPath(full, key),
          full.isEmpty ? member.title : group,
          into,
        );
      }
      return;
    }
    if (!differsFromDefault(held, field)) return;
    into.add(
      ModifiedField(
        path: full,
        title: field.title.isEmpty ? full : field.title,
        group: group,
        value: held,
        sensitive: field.sensitive,
      ),
    );
  }

  /// Returns a field to its default: nothing is held there.
  void reset(String path) => remove(path);

  /// Returns every setting that differs from its default to it.
  void resetAll() {
    for (final field in modified.toList()) {
      remove(field.path);
    }
  }

  /// How many settings of the top-level section `id` differ from their defaults.
  int modifiedIn(String id) => modified
      .where((entry) => sectionOf(parsePathHead(entry.path)) == id)
      .length;
}

/// The first member of `path`: the key of the top-level setting it is in.
String parsePathHead(String path) {
  final end = path.indexOf(RegExp(r'[.\[]'), path.startsWith('[') ? 1 : 0);
  final head = end < 0 ? path : path.substring(0, end);
  return head.startsWith('[')
      ? head.replaceAll(RegExp(r'^\["?|"?\]$'), '')
      : head;
}

/// The path of the group or entry that holds the one at `path`; empty at the top.
String parentOf(String path) {
  if (path.endsWith(']')) {
    var depth = 0;
    var quoted = false;
    for (var i = path.length - 1; i >= 0; i--) {
      final c = path[i];
      if (c == '"' && (i == 0 || path[i - 1] != '\\')) quoted = !quoted;
      if (quoted) continue;
      if (c == ']') depth++;
      if (c == '[' && --depth == 0) return path.substring(0, i);
    }
    return '';
  }
  final dot = path.lastIndexOf('.');
  return dot < 0 ? '' : path.substring(0, dot);
}

/// Whether `path` is `owner` or lies under it.
bool isAtOrUnder(String path, String owner) =>
    path == owner ||
    owner.isEmpty ||
    path.startsWith('$owner.') ||
    path.startsWith('$owner[');

/// One setting that differs from its default.
class ModifiedField {
  const ModifiedField({
    required this.path,
    required this.title,
    required this.group,
    required this.value,
    this.sensitive = false,
  });

  final String path;
  final String title;

  /// The title of the group it is in, when it is in one.
  final String group;
  final Object? value;

  /// Whether the schema marks it a secret, which is never shown.
  final bool sensitive;
}

/// Whether `held` is a value that differs from what `field` defaults to: nothing, for a field
/// with no default.
bool differsFromDefault(Object? held, FieldSchema field) {
  if (identical(held, guatiao.absent) || held == null) return false;
  final fallback = field.hasDefault ? field.defaultValue : null;
  if (fallback != null) return !sameValue(held, fallback);
  return !isBlank(held);
}

/// Whether `value` holds nothing a person wrote: no value, no text, no entries.
bool isBlank(Object? value) =>
    identical(value, guatiao.absent) ||
    value == null ||
    (value is String && value.isEmpty) ||
    (value is List && value.isEmpty) ||
    (value is Map && value.isEmpty) ||
    (value is Map && value.values.every(isBlank));

/// Whether two plain values are the same: maps by their entries, lists by their items, numbers by
/// their value.
bool sameValue(Object? a, Object? b) {
  if (a is Map && b is Map) {
    if (a.length != b.length) return false;
    for (final key in a.keys) {
      if (!b.containsKey(key) || !sameValue(a[key], b[key])) return false;
    }
    return true;
  }
  if (a is List && b is List) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (!sameValue(a[i], b[i])) return false;
    }
    return true;
  }
  if (a is num && b is num) return a == b;
  if (a is BigInt || b is BigInt) return '$a' == '$b';
  return a == b;
}

/// What one line of a diff does to a setting.
enum ChangeKind { added, removed, changed }

/// One setting a save would change.
class RecordChange {
  const RecordChange(this.path, this.kind, this.before, this.after);

  /// The setting's key, dotted down through groups and maps.
  final String path;
  final ChangeKind kind;
  final Object? before;
  final Object? after;

  /// `+ key = value`, `- key = value` or `~ key: old -> new`.
  String get line => switch (kind) {
    ChangeKind.added => '+ $path = ${showValue(after)}',
    ChangeKind.removed => '- $path = ${showValue(before)}',
    ChangeKind.changed =>
      '~ $path: ${showValue(before)} -> ${showValue(after)}',
  };
}

/// Stands for a secret's value in a diff.
const hiddenValue = _Hidden();

class _Hidden {
  const _Hidden();
}

/// A value on one line; a secret is `(hidden)`.
String showValue(Object? value) => switch (value) {
  null => 'none',
  _Hidden() => '(hidden)',
  String text => '"$text"',
  List items => '[${items.map(showValue).join(', ')}]',
  Map entries =>
    '{${entries.entries.map((entry) => '${entry.key}: ${showValue(entry.value)}').join(', ')}}',
  _ => '$value',
};

/// Every setting that differs between the record as stored and as it would be saved, key by key.
///
/// A setting held as nothing (no value, empty text, an empty list or table) is the same as a
/// setting not held, so a form that writes a group out empty is not a change.
List<RecordChange> diffRecords(
  Map<String, Object?> before,
  Map<String, Object?> after, {
  String path = '',
  bool Function(String path)? secret,
}) {
  final changes = <RecordChange>[];
  final keys = <String>{...before.keys, ...after.keys}.toList()..sort();
  for (final key in keys) {
    final at = path.isEmpty ? key : '$path.$key';
    final old = before[key];
    final next = after[key];
    if (old is Map && next is Map) {
      changes.addAll(
        diffRecords(
          old.cast<String, Object?>(),
          next.cast<String, Object?>(),
          path: at,
          secret: secret,
        ),
      );
    } else if (isBlank(old) && isBlank(next)) {
      continue;
    } else {
      final hidden = secret?.call(at) ?? false;
      Object? shown(Object? value) => hidden ? hiddenValue : value;
      if (isBlank(old)) {
        changes.add(RecordChange(at, ChangeKind.added, null, shown(next)));
      } else if (isBlank(next)) {
        changes.add(RecordChange(at, ChangeKind.removed, shown(old), null));
      } else if (!sameValue(old, next)) {
        changes.add(
          RecordChange(at, ChangeKind.changed, shown(old), shown(next)),
        );
      }
    }
  }
  return changes;
}
