import 'dart:convert';

import 'package:guatiao/guatiao.dart' as guatiao;
import 'package:guatiao_intake/guatiao_intake.dart' as intake;
import 'package:material_ui/material_ui.dart' show ChangeNotifier;

/// One place in a JSON Schema, read for what a form needs to draw it.
class FieldSchema {
  FieldSchema(Map<String, Object?> node) : node = _peeled(node);

  /// The schema, with an optional kind's wrapper taken off and the words kept.
  final Map<String, Object?> node;

  static Map<String, Object?> _peeled(Map<String, Object?> node) {
    final arms = node['anyOf'];
    if (arms is List && arms.length == 2 && !node.containsKey('type')) {
      final others = arms
          .whereType<Map>()
          .where((arm) => arm['type'] != 'null')
          .toList();
      if (others.length == 1) {
        final words = {...node}..remove('anyOf');
        return _peeled({...others.single.cast<String, Object?>(), ...words});
      }
    }
    return node;
  }

  static List<String> _types(Object? type) => switch (type) {
    final String one => [one],
    final List<Object?> many => many.whereType<String>().toList(),
    _ => const [],
  };

  List<String> get types => _types(node['type']);

  bool get isBool => types.contains('boolean');
  bool get isInteger => types.contains('integer');
  bool get isNumber => types.contains('number');
  bool get isText => types.contains('string');
  bool get isList => types.contains('array');
  bool get isObject => types.contains('object');

  /// Whether it says nothing about what it holds: any value.
  bool get isAny => types.isEmpty && !node.containsKey('anyOf');

  /// An object with declared fields.
  bool get hasMembers => node['properties'] is Map;

  /// An object whose keys are data.
  bool get isOpenMap => isObject && !hasMembers;

  String get title => node['title'] as String? ?? '';
  String get description => node['description'] as String? ?? '';
  String? get format => node['format'] as String?;
  bool get advanced => node['x-advanced'] == true;
  bool get sensitive =>
      node['x-sensitive'] == true || node['writeOnly'] == true;
  Object? get defaultValue => node['default'];
  bool get hasDefault => node.containsKey('default');
  List<String> get requiredKeys =>
      (node['required'] as List?)?.cast<String>() ?? const [];

  /// The values it may hold, for a closed choice.
  List<String>? get choices {
    final values = node['enum'];
    return values is List ? values.whereType<String>().toList() : null;
  }

  /// The values it offers while accepting others.
  List<String>? get suggestions {
    final values = node['examples'];
    return values is List && isText
        ? values.whereType<String>().toList()
        : null;
  }

  /// A choice's words, by its value.
  String labelOf(String value) {
    final labels = node['x-enum-labels'];
    return labels is Map ? labels[value] as String? ?? value : value;
  }

  /// Its alternatives, when it is one of several kinds.
  List<FieldSchema> get arms => [
    for (final arm in (node['anyOf'] as List?) ?? const [])
      if (arm is Map && arm['type'] != 'null')
        FieldSchema(arm.cast<String, Object?>()),
  ];

  /// The arm that holds a number, when one does.
  FieldSchema? get numberArm {
    for (final arm in arms) {
      if (arm.isInteger || arm.isNumber) return arm;
    }
    return null;
  }

  /// The arm that holds a fixed word or several, when one does.
  FieldSchema? get wordArm {
    for (final arm in arms) {
      if (arm.choices != null) return arm;
    }
    return null;
  }

  /// The arm with fields of its own, when one has them.
  FieldSchema? get memberArm {
    for (final arm in arms) {
      if (arm.hasMembers) return arm;
    }
    return null;
  }

  FieldSchema? member(String key) {
    final properties = node['properties'];
    final member = properties is Map ? properties[key] : null;
    return member is Map ? FieldSchema(member.cast<String, Object?>()) : null;
  }

  FieldSchema? get items {
    final items = node['items'];
    return items is Map ? FieldSchema(items.cast<String, Object?>()) : null;
  }

  FieldSchema? get values {
    final values = node['additionalProperties'];
    return values is Map ? FieldSchema(values.cast<String, Object?>()) : null;
  }
}

/// The hints a form gives one field.
class FieldHints {
  const FieldHints(this.node);

  final Map<String, Object?> node;

  String? get widget => node['widget'] as String?;
  String? get placeholder => node['placeholder'] as String?;
  String? get unit => node['unit'] as String?;

  /// The form of the field's own members, when it has one.
  Map<String, Object?> get form =>
      (node['form'] as Map?)?.cast<String, Object?>() ?? const {};
}

/// A section as the form lays it out.
class FormGroup {
  const FormGroup({
    required this.id,
    required this.title,
    required this.help,
    required this.fields,
  });

  final String id;
  final String title;
  final String help;
  final List<String> fields;
}

/// The path of the entry `key` of the map at `path`, quoted so any key reads back as itself.
String keyedPath(String path, String key) =>
    '$path["${key.replaceAll(r'\', r'\\').replaceAll('"', r'\"')}"]';

/// The key a store entry directly under `path` names, however the store spelled it, and how many
/// characters of it the key took; null for an entry that is not one.
(String, int)? _keyAfter(String stored, String path) {
  if (!stored.startsWith(path)) return null;
  var at = path.length;
  if (at >= stored.length) return null;
  if (stored[at] == '.') {
    final end = stored.indexOf(RegExp(r'[.\[]'), at + 1);
    final stop = end < 0 ? stored.length : end;
    return (stored.substring(at + 1, stop), stop);
  }
  if (stored[at] != '[') return null;
  at += 1;
  if (at < stored.length && stored[at] == '"') {
    final key = StringBuffer();
    at += 1;
    while (at < stored.length && stored[at] != '"') {
      if (stored[at] == r'\' && at + 1 < stored.length) at += 1;
      key.write(stored[at]);
      at += 1;
    }
    return (key.toString(), at + 2);
  }
  final end = stored.indexOf(']', at);
  if (end < 0) return null;
  return (stored.substring(at, end), end + 1);
}

/// The list or map `path` is an entry of, or `path` itself when it ends in a member.
String _ownerOfEntry(String path) {
  if (!path.endsWith(']')) return path;
  var quoted = false;
  var open = -1;
  for (var at = 0; at < path.length; at++) {
    final character = path[at];
    if (quoted) {
      if (character == r'') {
        at += 1;
      } else if (character == '"') {
        quoted = false;
      }
    } else if (character == '[') {
      open = at;
      quoted = at + 1 < path.length && path[at + 1] == '"';
      if (quoted) at += 1;
    }
  }
  return open > 0 ? path.substring(0, open) : path;
}

/// What a person is filling in, over a schema and a form from anywhere: the flat store of what
/// was typed, the layout and which fields show, both judged by `guatiao_intake`, and the value
/// that comes of it.
///
/// It holds native or module memory: [close] it.
class FormModel extends ChangeNotifier {
  FormModel({
    required Map<String, Object?> schema,
    required Map<String, Object?> form,
    Map<String, Object?> value = const {},
  }) : schema = FieldSchema(schema),
       _scope = _Scope(schema, form) {
    final problem = intake.check(_scope.schema, _scope.form);
    if (problem != null) {
      close();
      throw StateError(
        'The form does not fit its schema: ${problem['message']}',
      );
    }
    _load(value);
  }

  final FieldSchema schema;
  final _Scope _scope;
  final _scopes = <String, _Scope>{};

  /// What was typed, by flat key.
  final store = <String, String>{};

  /// The containers a person left empty, or that came empty: a flat store cannot hold them.
  final _empty = <String>{};

  /// The fields with members of their own holding one of several arms, by path: the arm holds
  /// entries in the store under the path.
  final _memberArm = <String>{};

  /// What an open value held before it was typed over, so an untouched one keeps its kind.
  final _original = <String, Object?>{};

  bool _closed = false;

  /// Lays out the fields at `path` (`''` for the record), in sections, in order.
  List<FormGroup> layout([String path = '']) {
    final scope = _scopeAt(path);
    if (scope == null) return const [];
    return [
      for (final group in intake.layout(scope.schema, scope.form))
        () {
          final section = (group['section'] as Map?)?.cast<String, Object?>();
          return FormGroup(
            id: section?['id'] as String? ?? '',
            title: section?['title'] as String? ?? '',
            help: section?['description'] as String? ?? '',
            fields: (group['fields'] as List).cast<String>(),
          );
        }(),
    ];
  }

  /// The schema of the field at `path`.
  FieldSchema? field(String path) {
    final resolved = intake.resolve(_scope.schema, path);
    if (resolved == null) return null;
    return FieldSchema((resolved.toDart() as Map).cast<String, Object?>());
  }

  /// The form's hints for the member `key` of what is at `path`.
  FieldHints hints(String path, String key) {
    final fields = _scopeAt(path)?.formJson['fields'];
    final found = fields is Map ? fields[key] : null;
    return FieldHints(found is Map ? found.cast<String, Object?>() : const {});
  }

  /// Whether the member `key` of what is at `path` shows now, by what was typed.
  bool isVisible(String path, String key) {
    final scope = _scopeAt(path);
    if (scope == null) return true;
    final values = guatiao.Value.fromDart(_valueAt(path));
    try {
      return intake.isVisible(scope.schema, scope.form, key, values);
    } finally {
      values.close();
    }
  }

  _Scope? _scopeAt(String path) {
    if (path.isEmpty) return _scope;
    final known = _scopes[path];
    if (known != null) return known;
    final at = field(path);
    final schema = at?.hasMembers == true ? at : at?.memberArm;
    if (schema == null || !schema.hasMembers) return null;
    // A list's or a map's own form is its entries'.
    final form = intake.formFor(
      _scope.form,
      _scope.schema,
      _ownerOfEntry(path),
    );
    final scope = _Scope(
      schema.node,
      form == null ? const {} : (form.toDart() as Map).cast<String, Object?>(),
    );
    form?.close();
    return _scopes[path] = scope;
  }

  void _load(Map<String, Object?> value) {
    for (final entry in value.entries) {
      final at = schema.member(entry.key);
      if (at == null) continue;
      _seed(entry.key, at, entry.value);
    }
  }

  /// Writes `value` into the store under `path`, by `at`.
  void _seed(String path, FieldSchema at, Object? value) {
    if (at.hasMembers && value is Map) {
      for (final entry in value.entries) {
        final member = at.member(entry.key as String);
        if (member != null) {
          _seed('$path.${entry.key}', member, entry.value);
        }
      }
      return;
    }
    if (at.memberArm != null && value is Map) {
      _memberArm.add(path);
      _seed(path, at.memberArm!, value);
      return;
    }
    if (at.isList && value is List) {
      if (value.isEmpty) _empty.add(path);
      final items = at.items ?? FieldSchema(const {});
      for (var index = 0; index < value.length; index++) {
        _seed('$path[$index]', items, value[index]);
      }
      return;
    }
    if (at.isObject && value is Map) {
      if (value.isEmpty) _empty.add(path);
      final values = at.values ?? FieldSchema(const {});
      for (final entry in value.entries) {
        _seed(keyedPath(path, entry.key as String), values, entry.value);
      }
      return;
    }
    if (value == null) {
      // A lifted bound may be stored as null; its word is what a person sees.
      final word = at.wordArm?.choices;
      if (word != null && word.length == 1) store[path] = word.single;
      return;
    }
    _original[path] = value;
    store[path] = value is String
        ? value
        : (value is Map || value is List)
        ? jsonEncode(value)
        : '$value';
  }

  /// Whether a value comes of what is at or under `path`.
  bool _carries(String path) {
    bool inside(String key) =>
        key == path || key.startsWith('$path.') || key.startsWith('$path[');
    return holds(path) || _empty.any(inside) || _memberArm.any(inside);
  }

  /// Whether anything is typed at or under `path`.
  bool holds(String path) =>
      store.containsKey(path) ||
      store.keys.any(
        (key) => key.startsWith('$path.') || key.startsWith('$path['),
      );

  String? text(String path) => store[path];

  /// Sets or, with an empty text, clears one entry.
  void setText(String path, String? text) {
    if (text == null || text.isEmpty) {
      store.remove(path);
    } else {
      store[path] = text;
    }
    notifyListeners();
  }

  /// Sets one entry of a map; with nothing typed yet it is still an entry.
  void setEntry(String path, String text) {
    store[path] = text;
    notifyListeners();
  }

  /// Whether the arm with its own fields is the one chosen at `path`.
  bool usesMemberArm(String path) => _memberArm.contains(path);

  /// Chooses a word at `path`, or with `null` the arm with its own fields.
  void chooseArm(String path, String? word) {
    _clearUnder(path);
    if (word == null) {
      _memberArm.add(path);
    } else {
      _memberArm.remove(path);
      if (word.isNotEmpty) store[path] = word;
    }
    notifyListeners();
  }

  void _clearUnder(String path) {
    store.removeWhere(
      (key, _) =>
          key == path || key.startsWith('$path.') || key.startsWith('$path['),
    );
  }

  /// The texts of a list at `path`, in order.
  List<int> positions(String path) {
    final found = <int>{};
    for (final key in store.keys) {
      if (!key.startsWith('$path[')) continue;
      final end = key.indexOf(']', path.length);
      final at = int.tryParse(key.substring(path.length + 1, end));
      if (at != null) found.add(at);
    }
    return found.toList()..sort();
  }

  /// A new blank place at the end of the list at `path`, for fields with members to fill.
  int nextPosition(String path) {
    final known = positions(path);
    return known.isEmpty ? 0 : known.last + 1;
  }

  /// Adds one text to the end of the list at `path`.
  void addToList(String path, String text) {
    _empty.remove(path);
    store['$path[${nextPosition(path)}]'] = text;
    notifyListeners();
  }

  /// Takes out what is at position `at` of the list at `path`.
  void removeFromList(String path, int at) {
    _clearUnder('$path[$at]');
    if (positions(path).isEmpty) _empty.add(path);
    notifyListeners();
  }

  /// The keys of an open map at `path`, in the order they sort.
  List<String> mapKeys(String path) {
    final found = <String>{};
    for (final key in store.keys) {
      final step = _keyAfter(key, path);
      if (step != null) found.add(step.$1);
    }
    return found.toList()..sort();
  }

  /// Adds an entry under `key`, blank for a value with members to fill.
  void addToMap(String path, String key, [String value = '']) {
    _empty.remove(path);
    store[keyedPath(path, key)] = value;
    notifyListeners();
  }

  /// Takes out the entry under `key`.
  void removeFromMap(String path, String key) {
    for (final stored in store.keys.toList()) {
      final step = _keyAfter(stored, path);
      if (step != null && step.$1 == key) store.remove(stored);
    }
    if (mapKeys(path).isEmpty) _empty.add(path);
    notifyListeners();
  }

  /// The value entered so far, with `issues` naming what will not read as its kind.
  Map<String, Object?> value({Map<String, String>? issues}) =>
      _objectValue('', schema, issues);

  Map<String, Object?> _valueAt(String path) {
    if (path.isEmpty) return value();
    final at = field(path);
    if (at == null) return const {};
    final schema = at.hasMembers ? at : at.memberArm;
    if (schema == null) return const {};
    return _objectValue(path, schema, null);
  }

  Map<String, Object?> _objectValue(
    String path,
    FieldSchema at,
    Map<String, String>? issues,
  ) {
    final out = <String, Object?>{};
    final properties = at.node['properties'];
    if (properties is! Map) return out;
    for (final key in properties.keys.cast<String>()) {
      final member = at.member(key)!;
      final here = path.isEmpty ? key : '$path.$key';
      if (!_carries(here)) continue;
      final value = _read(here, member, issues);
      if (value != _nothing) out[key] = value;
    }
    return out;
  }

  static const _nothing = Object();

  Object? _read(String path, FieldSchema at, Map<String, String>? issues) {
    if (at.hasMembers) {
      final value = _objectValue(path, at, issues);
      return value.isEmpty ? _nothing : value;
    }
    if (_memberArm.contains(path) && at.memberArm != null) {
      return _objectValue(path, at.memberArm!, issues);
    }
    if (at.isList) {
      final items = at.items ?? FieldSchema(const {});
      final out = [
        for (final index in positions(path))
          _read('$path[$index]', items, issues),
      ].where((value) => value != _nothing).toList();
      return out.isEmpty && !_empty.contains(path) ? _nothing : out;
    }
    if (at.isObject) {
      final values = at.values ?? FieldSchema(const {});
      final out = <String, Object?>{};
      for (final key in mapKeys(path)) {
        final value = _read(keyedPath(path, key), values, issues);
        if (value != _nothing) out[key] = value;
      }
      return out.isEmpty && !_empty.contains(path) ? _nothing : out;
    }
    final text = store[path];
    if (text == null) return _nothing;
    return _typed(path, at, text, issues);
  }

  Object? _typed(
    String path,
    FieldSchema at,
    String text,
    Map<String, String>? issues,
  ) {
    if (at.isBool) return text == 'true';
    if (at.isInteger) {
      final number = int.tryParse(text.trim());
      if (number == null) issues?[path] = 'A whole number, please.';
      return number ?? text;
    }
    if (at.isNumber) {
      final number = num.tryParse(text.trim());
      if (number == null) issues?[path] = 'A number, please.';
      return number ?? text;
    }
    if (at.isText) return text;
    final arms = at.arms;
    if (arms.isNotEmpty) {
      final word = at.wordArm?.choices;
      if (word != null && word.contains(text)) return text;
      final number = at.numberArm;
      if (number != null) return _typed(path, number, text, issues);
      return text;
    }
    return _open(path, text);
  }

  /// A value whose kind nothing declares: as it was while its text is unchanged; else the
  /// number, true, false or JSON the text spells, and otherwise the text.
  Object? _open(String path, String text) {
    final original = _original[path];
    if (_original.containsKey(path)) {
      final was = original is String
          ? original
          : (original is Map || original is List)
          ? jsonEncode(original)
          : '$original';
      if (was == text) return original;
    }
    try {
      final decoded = jsonDecode(text);
      return decoded;
    } on FormatException {
      return text;
    }
  }

  /// The longest field path `reason` names, for showing a refusal on its field.
  String? fieldNamedIn(String reason) {
    final paths = <String>[];
    void walk(String path, FieldSchema at) {
      final properties = at.node['properties'];
      if (properties is! Map) {
        final arm = at.memberArm;
        if (arm != null) walk(path, arm);
        return;
      }
      for (final key in properties.keys.cast<String>()) {
        final here = path.isEmpty ? key : '$path.$key';
        paths.add(here);
        walk(here, at.member(key)!);
      }
    }

    walk('', schema);
    paths.sort((a, b) => b.length.compareTo(a.length));
    for (final path in paths) {
      if (RegExp('(^|[^A-Za-z0-9_.])${RegExp.escape(path)}(\$|[^A-Za-z0-9_])')
          .hasMatch(reason)) {
        return path;
      }
    }
    return null;
  }

  void close() {
    if (_closed) return;
    _closed = true;
    _scope.close();
    for (final scope in _scopes.values) {
      scope.close();
    }
  }

  @override
  void dispose() {
    close();
    super.dispose();
  }
}

/// A schema and a form held as guatiao values.
class _Scope {
  _Scope(Map<String, Object?> schemaJson, this.formJson)
    : schema = guatiao.Value.fromDart(schemaJson),
      form = guatiao.Value.fromDart(formJson);

  final Map<String, Object?> formJson;
  final guatiao.Value schema;
  final guatiao.Value form;

  void close() {
    schema.close();
    form.close();
  }
}

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
