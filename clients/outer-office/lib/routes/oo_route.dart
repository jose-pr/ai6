import 'dart:convert';

import 'package:flutter/foundation.dart' show immutable, listEquals;

import '../held/held_briefs_page.dart' show isInvisible;
import '../shell/oo_shell.dart' show OoPage;

/// The states of a Mission or Session on the board, in the board's order.
const boardStates = [
  'queued',
  'working',
  'blocked',
  'stalled',
  'parked',
  'quarantined',
  'done',
  'failed',
  'abandoned',
];

/// The bounds every route is checked against.
@immutable
class OoRouteLimits {
  const OoRouteLimits({this.textBytes = defaultTextBytes});

  /// The longest `text` filter by default, in bytes.
  static const defaultTextBytes = 128;

  /// The longest `text` filter, in bytes: `OO.cfg`'s `[routes] text_bytes`.
  final int textBytes;

  /// The limits in force, set once at start from `OO.cfg`.
  static OoRouteLimits current = const OoRouteLimits();
}

const _missionSorts = ['updated', 'created', 'state', 'operation', 'mission'];
const _sessionSorts = [..._missionSorts, 'session'];
const _orders = ['asc', 'desc'];

enum _Kind { identifier, text, states, oneOf }

class _Field {
  const _Field(this.name, this.kind, [this.words = const []]);

  final String name;
  final _Kind kind;

  /// The words a [_Kind.oneOf] field takes.
  final List<String> words;
}

/// Each page's fields, in link order. Universal Exports' `routes.rs` has the same table.
const _fields = <OoPage, List<_Field>>{
  OoPage.overview: [],
  OoPage.operations: [_Field('operation_id', _Kind.identifier)],
  OoPage.missions: [
    _Field('operation_id', _Kind.identifier),
    _Field('state', _Kind.states),
    _Field('station_id', _Kind.identifier),
    _Field('agent', _Kind.identifier),
    _Field('text', _Kind.text),
    _Field('sort', _Kind.oneOf, _missionSorts),
    _Field('order', _Kind.oneOf, _orders),
    _Field('mission_id', _Kind.identifier),
    _Field('subtree', _Kind.oneOf, ['true']),
  ],
  OoPage.sessions: [
    _Field('operation_id', _Kind.identifier),
    _Field('state', _Kind.oneOf, boardStates),
    _Field('station_id', _Kind.identifier),
    _Field('agent', _Kind.identifier),
    _Field('sort', _Kind.oneOf, _sessionSorts),
    _Field('order', _Kind.oneOf, _orders),
    _Field('session_id', _Kind.identifier),
  ],
  OoPage.stations: [
    _Field('station_id', _Kind.identifier),
    _Field('tab', _Kind.oneOf, [
      'overview',
      'theaters',
      'settings',
      'placement',
    ]),
    _Field('theater_id', _Kind.identifier),
  ],
  OoPage.agents: [
    _Field('tab', _Kind.oneOf, ['agents', 'roles']),
    _Field('operation_id', _Kind.identifier),
    _Field('agent', _Kind.identifier),
    _Field('role', _Kind.identifier),
  ],
  OoPage.greenInk: [
    _Field('tab', _Kind.oneOf, ['inbox', 'classes']),
    _Field('operation_id', _Kind.identifier),
    _Field('request_id', _Kind.identifier),
  ],
  OoPage.briefings: [
    _Field('tab', _Kind.oneOf, ['proposals', 'held']),
    _Field('operation_id', _Kind.identifier),
    _Field('item_id', _Kind.identifier),
  ],
  OoPage.roster: [
    _Field('kind', _Kind.oneOf, [
      'operation',
      'agent',
      'role',
      'cover',
      'assistant',
    ]),
    _Field('key', _Kind.identifier),
  ],
  OoPage.pools: [
    _Field('operation_id', _Kind.identifier),
    _Field('pool', _Kind.identifier),
  ],
  OoPage.timeline: [
    _Field('operation_id', _Kind.identifier),
    _Field('mission_id', _Kind.identifier),
  ],
  OoPage.audit: [
    _Field('operation_id', _Kind.identifier),
    _Field('station_id', _Kind.identifier),
  ],
  OoPage.memory: [],
};

/// Whether [rune] is a control character (Unicode category Cc), as Rust's `char::is_control`.
bool _isControl(int rune) => rune <= 0x1F || (rune >= 0x7F && rune <= 0x9F);

/// An identifier as AI6 records it: 1 to 256 bytes without control characters.
bool isIdentifier(String text) =>
    text.isNotEmpty &&
    utf8.encode(text).length <= 256 &&
    !text.runes.any(_isControl);

/// A page of Outer Office and its filters: a closed set of pages, each with typed optional
/// fields, and nothing else (no action, dialog, form value, token or outside URL).
///
/// Its JSON form is tagged by `page`; its link form is `app:/<page>?k=v&...`, the query in
/// the page's field order, `state` comma-joined and every value percent-encoded. A web URL
/// carries the link form after the `#`. Universal Exports parses both forms the same way and
/// refuses with the same reasons, both tested on `test/fixtures/assistant_routes.json`.
@immutable
class OoRoute {
  /// [page] without fields.
  const OoRoute(this.page) : _values = const {};

  const OoRoute._(this.page, this._values);

  final OoPage page;

  /// Each field's value in the page's field order: a `String`, or a `List<String>` for a
  /// Missions `state`.
  final Map<String, Object> _values;

  /// The fields that are set, in link order.
  Iterable<String> get keys => _values.keys;

  /// A field's value; `null` when it is absent. A Missions `state` reads comma-joined.
  String? operator [](String key) => switch (_values[key]) {
    final String text => text,
    final List<String> states => states.join(','),
    _ => null,
  };

  /// The board states this route filters by: a Missions list, a Sessions state, or none.
  List<String> get states => switch (_values['state']) {
    final String state => [state],
    final List<String> states => states,
    _ => const [],
  };

  /// Whether the route sets any field but `tab`.
  bool get filtered => _values.keys.any((key) => key != 'tab');

  /// Parses the JSON form; a [FormatException] gives the reason it is not a route.
  factory OoRoute.fromJson(Object? json) {
    const notARoute = 'a route is an object naming its page';
    if (json is! Map) throw const FormatException(notARoute);
    final name = json['page'];
    if (name is! String) throw const FormatException(notARoute);
    final page = _named(name);
    final keys = [
      for (final key in json.keys)
        if (key != 'page') key is String ? key : '$key',
    ]..sort();
    final values = <(String, Object)>[];
    for (final key in keys) {
      final field = _field(page, key);
      final value = json[key];
      if (field.kind == _Kind.states) {
        if (value is! List || value.any((item) => item is! String)) {
          throw const FormatException('state is a list of board states');
        }
        values.add((field.name, List<String>.of(value.cast<String>())));
      } else if (value is String) {
        values.add((field.name, value));
      } else {
        throw FormatException('${field.name} is a string');
      }
    }
    return _checked(page, values);
  }

  /// Parses the link form, `app:/<page>?k=v&...`; fields may come in any order.
  factory OoRoute.parseUri(String uri) {
    if (!uri.startsWith('app:/')) {
      throw const FormatException('a route link starts with app:/');
    }
    final rest = uri.substring('app:/'.length);
    final at = rest.indexOf('?');
    final page = _named(at < 0 ? rest : rest.substring(0, at));
    final values = <(String, Object)>[];
    if (at >= 0) {
      for (final pair in rest.substring(at + 1).split('&')) {
        final equals = pair.indexOf('=');
        if (equals <= 0) {
          throw const FormatException('a query parameter is key=value');
        }
        final field = _field(page, pair.substring(0, equals));
        final value = pair.substring(equals + 1);
        values.add((
          field.name,
          field.kind == _Kind.states
              ? [
                  for (final state in value.split(','))
                    _decode(field.name, state),
                ]
              : _decode(field.name, value),
        ));
      }
    }
    return _checked(page, values);
  }

  /// Parses a web location, the part of the URL after `#`: `/missions?state=failed`. The
  /// bare `/` is Overview.
  factory OoRoute.fromLocation(Uri location) {
    final text = location.toString();
    if (text.isEmpty || text == '/') return const OoRoute(OoPage.overview);
    return OoRoute.parseUri('app:$text');
  }

  /// The JSON form.
  Map<String, Object> toJson() => {
    'page': page.id,
    for (final MapEntry(:key, :value) in _values.entries)
      key: value is List<String> ? List<String>.of(value) : value,
  };

  /// The link form, its query in the page's field order.
  String toUri() {
    final query = [
      for (final MapEntry(:key, :value) in _values.entries)
        '$key=${value is List<String> ? value.map(_encode).join(',') : _encode(value as String)}',
    ].join('&');
    return query.isEmpty ? 'app:/${page.id}' : 'app:/${page.id}?$query';
  }

  /// The web location: the link form without `app:`.
  String get location => toUri().substring('app:'.length);

  /// This route with [key] set to [value], a `String` or, for a Missions `state`, a
  /// `List<String>`; `null` or an empty list removes it. Throws a [FormatException] when the
  /// result is not a route.
  OoRoute withField(String key, Object? value) {
    final values = [
      for (final MapEntry(:key, :value) in _values.entries) (key, value),
    ]..removeWhere((entry) => entry.$1 == key);
    if (value != null && !(value is List && value.isEmpty)) {
      _field(page, key);
      values.add((key, value is List ? List<String>.of(value.cast()) : value));
    }
    return _checked(page, values);
  }

  /// This route without [key], and without what needs it (`mission_id` needs
  /// `operation_id`, `theater_id` needs `station_id`).
  OoRoute without(String key) {
    final values = {..._values}..remove(key);
    if (key == 'operation_id' && page == OoPage.missions) {
      values
        ..remove('mission_id')
        ..remove('subtree');
    }
    if (key == 'station_id' && page == OoPage.stations) {
      values.remove('theater_id');
    }
    return OoRoute._(page, values);
  }

  /// This route without its filters; the tab stays.
  OoRoute get cleared =>
      OoRoute._(page, {if (_values['tab'] case final Object tab) 'tab': tab});

  @override
  bool operator ==(Object other) =>
      other is OoRoute &&
      other.page == page &&
      other._values.length == _values.length &&
      _values.entries.every((entry) {
        final theirs = other._values[entry.key];
        final ours = entry.value;
        return ours is List<String> && theirs is List<String>
            ? listEquals(ours, theirs)
            : ours == theirs;
      });

  @override
  int get hashCode => Object.hash(
    page,
    Object.hashAll([
      for (final MapEntry(:key, :value) in _values.entries) ...[
        key,
        if (value is List<String>) Object.hashAll(value) else value,
      ],
    ]),
  );

  @override
  String toString() => toUri();
}

/// The words [page]'s field [key] takes, in their order; empty when it is not one of a set.
List<String> routeWords(OoPage page, String key) => [
  for (final field in _fields[page]!)
    if (field.name == key) ...field.words,
];

OoPage _named(String name) {
  for (final page in OoPage.values) {
    if (page.id == name) return page;
  }
  throw FormatException('unknown page $name');
}

_Field _field(OoPage page, String key) {
  for (final field in _fields[page]!) {
    if (field.name == key) return field;
  }
  throw FormatException('${page.id} has no $key');
}

/// Checks each field against its kind and the page's rules, and puts them in field order.
OoRoute _checked(OoPage page, List<(String, Object)> values) {
  for (var index = 0; index < values.length; index++) {
    final name = values[index].$1;
    if (values.take(index).any((earlier) => earlier.$1 == name)) {
      throw FormatException('$name appears twice');
    }
  }
  final order = [for (final field in _fields[page]!) field.name];
  values.sort((a, b) => order.indexOf(a.$1).compareTo(order.indexOf(b.$1)));
  for (final (name, value) in values) {
    final field = _field(page, name);
    switch ((field.kind, value)) {
      case (_Kind.identifier, final String text):
        if (!isIdentifier(text)) {
          throw FormatException('$name is not an identifier');
        }
      case (_Kind.text, final String text):
        if (text.isEmpty ||
            utf8.encode(text).length > OoRouteLimits.current.textBytes ||
            text.runes.any((rune) => _isControl(rune) || isInvisible(rune))) {
          throw FormatException(
            'text is 1 to ${OoRouteLimits.current.textBytes} bytes without control or '
            'invisible characters',
          );
        }
      case (_Kind.oneOf, final String text):
        if (!field.words.contains(text)) {
          throw FormatException('$name is one of ${field.words.join(', ')}');
        }
      case (_Kind.states, final List<String> states):
        for (final state in states) {
          if (!boardStates.contains(state)) {
            throw FormatException('$state is not a board state');
          }
        }
        if (states.isEmpty ||
            states.length > boardStates.length ||
            states.toSet().length != states.length) {
          throw FormatException(
            'state lists 1 to ${boardStates.length} distinct board states',
          );
        }
      case (_Kind.states, _):
        throw const FormatException('state is a list of board states');
      case (_, _):
        throw FormatException('$name is a string');
    }
  }
  bool has(String key) => values.any((entry) => entry.$1 == key);
  if (page == OoPage.missions && has('mission_id') && !has('operation_id')) {
    throw const FormatException('mission_id needs operation_id');
  }
  if (page == OoPage.missions && has('subtree') && !has('operation_id')) {
    throw const FormatException('subtree needs operation_id');
  }
  if (page == OoPage.stations && has('theater_id') && !has('station_id')) {
    throw const FormatException('theater_id needs station_id');
  }
  if (page == OoPage.audit && has('operation_id') && has('station_id')) {
    throw const FormatException(
      'audit takes operation_id or station_id, not both',
    );
  }
  return OoRoute._(page, {for (final (name, value) in values) name: value});
}

/// Percent-encodes every byte but the unreserved characters of RFC 3986, section 2.3.
String _encode(String text) {
  final encoded = StringBuffer();
  for (final byte in utf8.encode(text)) {
    final unreserved =
        (byte >= 0x30 && byte <= 0x39) ||
        (byte >= 0x41 && byte <= 0x5A) ||
        (byte >= 0x61 && byte <= 0x7A) ||
        byte == 0x2D ||
        byte == 0x2E ||
        byte == 0x5F ||
        byte == 0x7E;
    if (unreserved) {
      encoded.writeCharCode(byte);
    } else {
      encoded.write('%${byte.toRadixString(16).toUpperCase().padLeft(2, '0')}');
    }
  }
  return encoded.toString();
}

/// Decodes `%XX` sequences; everything else is taken as written. `+` is not a space.
String _decode(String name, String text) {
  final refused = FormatException('$name is not percent-encoded UTF-8');
  final bytes = utf8.encode(text);
  final decoded = <int>[];
  for (var at = 0; at < bytes.length;) {
    if (bytes[at] == 0x25) {
      if (at + 3 > bytes.length) throw refused;
      final hex = String.fromCharCodes(bytes.sublist(at + 1, at + 3));
      if (!RegExp(r'^[0-9A-Fa-f]{2}$').hasMatch(hex)) throw refused;
      decoded.add(int.parse(hex, radix: 16));
      at += 3;
    } else {
      decoded.add(bytes[at]);
      at += 1;
    }
  }
  try {
    return utf8.decode(decoded);
  } on FormatException {
    throw refused;
  }
}
