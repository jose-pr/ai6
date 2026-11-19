/// An overlay's knowledge served through MCP (`spec/v0/knowledge.md`): the tools' schemas and
/// arguments, `kb_list`, `kb_read`, templates, the result envelope and the hint. The core reads
/// nothing itself: a tree is handed to it, or read through a [FileSystem] the caller supplies.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'defaults.dart';
import 'error.dart';
import 'fs.dart';
import 'limits.dart';
import 'names.dart';
import 'knowledge_serve.dart';
import 'template.dart';
import 'text.dart';
import 'tree.dart';
import 'value.dart';

/// The directories whose files are knowledge, beside the root `CONTEXT.md` and the files the
/// manifest's `rules` and `context` name.
const List<String> knowledgeDirs = [
  'kb',
  'flows',
  'references',
  'rules',
  'context',
];

/// The search modes a server may declare, in the order `kb_list` writes them.
const List<String> knowledgeModes = ['words', 'meaning', 'llm'];

/// The template engines this edition defines, in the order `kb_list` writes them.
const List<String> knowledgeEngines = ['envsubst', 'mustache'];

/// The largest integer an argument may hold (`2^53 - 1`).
const int knowledgeMaxInteger = 9007199254740991;

/// `top` when a search gives none.
const int knowledgeDefaultTop = 5;

/// A refused call: [errorClass] is the vectors' class (`invalid-path`, `not-found`, ...) and
/// [message] text for a person, never to be parsed.
class KnowledgeError implements Exception {
  /// Creates the refusal.
  KnowledgeError(this.errorClass, [String? message])
    : message = message ?? errorClass;

  /// The class of the refusal.
  final String errorClass;

  /// What a person may read.
  final String message;

  @override
  String toString() => '$errorClass: $message';
}

const int _mib = 1024 * 1024;

const List<(String, String, int)> _knowledgeDefinitions = [
  ('max_file_bytes', 'maxFileBytes', _mib),
  ('max_read_bytes', 'maxReadBytes', 64 * 1024),
  ('max_rendered_bytes', 'maxRenderedBytes', _mib),
  ('max_query_characters', 'maxQueryCharacters', 1000),
  ('max_top', 'maxTop', 100),
  ('max_context_bytes', 'maxContextBytes', 16 * 1024),
  ('max_context_depth', 'maxContextDepth', 8),
  ('max_include_depth', 'maxIncludeDepth', 5),
];

/// The limits knowledge is served within, each a setting with a default and no floor: a null
/// lifts the bound. Immutable. Changes are keyed by field name (`'maxTop'`).
class KnowledgeSettings {
  /// The defaults with [changes] applied; a negative number or an unknown name is an
  /// [ArgumentError].
  KnowledgeSettings([Map<String, Limit> changes = const {}])
    : _values = _build(changes);

  /// Settings as a configuration writes them: snake_case keys (`max_top`), a number, `BigInt`,
  /// null or `"none"`.
  factory KnowledgeSettings.fromData(Map<String, Object?> data) {
    final changes = <String, Limit>{};
    for (final (key, field, _) in _knowledgeDefinitions) {
      if (!data.containsKey(key)) continue;
      final value = data[key];
      if (value == null || value == 'none') {
        changes[field] = null;
      } else if (value is BigInt && value.isValidInt) {
        changes[field] = value.toInt();
      } else if (value is int) {
        changes[field] = value;
      } else {
        throw ArgumentError('$key is a whole number or "none"');
      }
    }
    for (final key in data.keys) {
      if (!_knowledgeDefinitions.any((entry) => entry.$1 == key)) {
        throw ArgumentError('an unknown setting');
      }
    }
    return KnowledgeSettings(changes);
  }

  final Map<String, Limit> _values;

  static Map<String, Limit> _build(Map<String, Limit> changes) {
    final values = {
      for (final (_, field, byDefault) in _knowledgeDefinitions)
        field: byDefault as Limit,
    };
    for (final MapEntry(key: field, value: limit) in changes.entries) {
      if (!values.containsKey(field)) {
        throw ArgumentError('no setting is named $field');
      }
      if (limit != null && limit < 0) {
        throw ArgumentError('$field cannot be negative');
      }
      values[field] = limit;
    }
    return Map.unmodifiable(values);
  }

  /// A copy with [changes] applied.
  KnowledgeSettings copyWith(Map<String, Limit> changes) =>
      KnowledgeSettings({..._values, ...changes});

  /// Every setting by field name.
  Map<String, Limit> values() => Map.of(_values);

  /// The settings that differ from their defaults, keyed as a configuration writes them.
  List<NonDefault> nonDefaults() => [
    for (final (key, field, byDefault) in _knowledgeDefinitions)
      if (_values[field] != byDefault)
        NonDefault(key, limitText(_values[field]), '$byDefault'),
  ];

  /// Bytes of a served file.
  Limit get maxFileBytes => _values['maxFileBytes'];

  /// Bytes of the text of one `kb_read`.
  Limit get maxReadBytes => _values['maxReadBytes'];

  /// Bytes of a rendered template.
  Limit get maxRenderedBytes => _values['maxRenderedBytes'];

  /// Characters of a query.
  Limit get maxQueryCharacters => _values['maxQueryCharacters'];

  /// The most `top` may be.
  Limit get maxTop => _values['maxTop'];

  /// Bytes of the strings and keys of a `context`.
  Limit get maxContextBytes => _values['maxContextBytes'];

  /// Levels a `context` may nest.
  Limit get maxContextDepth => _values['maxContextDepth'];

  /// `@path` includes, and Mustache partials, one in another.
  Limit get maxIncludeDepth => _values['maxIncludeDepth'];
}

/// One entry of an overlay's tree, for [KnowledgeOverlay]: a regular file with its bytes, a
/// directory, a link or a special file.
class KnowledgeEntry {
  /// A regular file. [size] is its size when [bytes] hold only the start of it.
  KnowledgeEntry.file(Uint8List bytes, {int? size})
    : kind = 'file',
      bytes = bytes,
      size = size ?? bytes.length;

  /// A regular file of text.
  factory KnowledgeEntry.text(String text) =>
      KnowledgeEntry.file(encodeUtf8(text));

  /// A directory.
  KnowledgeEntry.directory()
    : kind = 'directory',
      bytes = Uint8List(0),
      size = 0;

  /// A symbolic link, junction or other reparse point: never followed.
  KnowledgeEntry.symlink() : kind = 'symlink', bytes = Uint8List(0), size = 0;

  /// A device, pipe, socket or anything else.
  KnowledgeEntry.special() : kind = 'special', bytes = Uint8List(0), size = 0;

  /// `file`, `directory`, `symlink` or `special`.
  final String kind;

  /// A file's bytes.
  final Uint8List bytes;

  /// A file's size in bytes.
  final int size;
}

/// An active overlay as the knowledge tools see it.
class KnowledgeOverlay {
  /// Creates the overlay: its [identity] as the consumer holds it, its [manifest], its [tree]
  /// by path, whether the consumer has left its `rules` and `context` out of the context
  /// because they are [served], and the [facts] templates read.
  KnowledgeOverlay({
    required this.identity,
    this.manifest = const {},
    required this.tree,
    this.served = false,
    this.facts = const TemplateFacts(),
  });

  /// The identity the consumer holds the overlay under.
  final String identity;

  /// The manifest.
  final Document manifest;

  /// Every entry of the overlay by relative path, directories and links included.
  final Map<String, KnowledgeEntry> tree;

  /// Whether the overlay's knowledge is served in place of its context.
  final bool served;

  /// What `AOS_OVERLAY_ROOT` and the other names read; the name defaults to the identity.
  final TemplateFacts facts;
}

/// What a call's caller is: the role and profile the consumer has the agent in, the consumer's
/// environment and the names of it templates may read.
class CallerFacts {
  /// Creates the facts.
  const CallerFacts({
    this.role,
    this.profile,
    this.env = const {},
    this.envAllow = const [],
  });

  /// `AOS_CALLER_ROLE`, as the consumer's own label.
  final String? role;

  /// `AOS_CALLER_PROFILE`.
  final String? profile;

  /// The consumer's environment.
  final Map<String, String> env;

  /// The names the user allowed templates to read; a sensitive one is never given.
  final List<String> envAllow;
}

bool _hides(int code) =>
    (code >= 0x202a && code <= 0x202e) ||
    (code >= 0x2060 && code <= 0x2064) ||
    (code >= 0x2066 && code <= 0x206f) ||
    (code >= 0x200b && code <= 0x200f) ||
    code == 0x2028 ||
    code == 0x2029 ||
    code == 0x061c ||
    code == 0xfeff ||
    code == 0x00ad ||
    code == 0x180e ||
    (code >= 0xfe00 && code <= 0xfe0f) ||
    (code >= 0xe0000 && code <= 0xe007f) ||
    (code >= 0xe0100 && code <= 0xe01ef) ||
    code == 0x115f ||
    code == 0x1160 ||
    code == 0x3164 ||
    code == 0xffa0 ||
    code == 0x034f ||
    (code >= 0x17b4 && code <= 0x17b5) ||
    (code >= 0x1d173 && code <= 0x1d17a) ||
    (code >= 0xfff9 && code <= 0xfffb);

/// [text] as an agent may receive it: each control character but tab and line feed, and each
/// character that hides or reorders text, is written `<U+XXXX>`.
String inert(String text) {
  final out = StringBuffer();
  for (final code in text.runes) {
    if (code == 0x0a || code == 0x09) {
      out.writeCharCode(code);
    } else if (code < 0x20 || (code >= 0x7f && code <= 0x9f) || _hides(code)) {
      out.write('<U+${code.toRadixString(16).toUpperCase().padLeft(4, '0')}>');
    } else {
      out.writeCharCode(code);
    }
  }
  return out.toString();
}

/// A file's lines: split at line feed, a carriage return before the line feed dropped, and no
/// line after a final line feed.
List<String> knowledgeLines(String text) {
  final parts = text.split('\n');
  final last = parts.removeLast();
  final lines = [
    for (final part in parts)
      part.endsWith('\r') ? part.substring(0, part.length - 1) : part,
  ];
  if (last.isNotEmpty) lines.add(last);
  return lines;
}

Map<String, Object?> _lineSchema() => {
  'type': 'integer',
  'minimum': 1,
  'maximum': knowledgeMaxInteger,
};

/// The tools `tools/list` names, in order, with their input schemas and no descriptions.
/// `kb_search` is there only when [modes] declare at least one mode.
List<Map<String, Object?>> knowledgeTools(
  Iterable<String> modes, [
  KnowledgeSettings? settings,
]) {
  final limit = settings ?? KnowledgeSettings();
  final declared = [
    for (final mode in knowledgeModes)
      if (modes.contains(mode)) mode,
  ];
  final tools = <Map<String, Object?>>[
    {
      'name': 'kb_list',
      'inputSchema': {
        'type': 'object',
        'properties': {
          'overlay': {'type': 'string'},
        },
        'additionalProperties': false,
      },
    },
    {
      'name': 'kb_read',
      'inputSchema': {
        'type': 'object',
        'properties': {
          'path': {'type': 'string'},
          'from': _lineSchema(),
          'to': _lineSchema(),
          'overlay': {'type': 'string'},
          'context': {'type': 'object'},
        },
        'required': ['path'],
        'additionalProperties': false,
      },
    },
  ];
  if (declared.isNotEmpty) {
    tools.add({
      'name': 'kb_search',
      'inputSchema': {
        'type': 'object',
        'properties': {
          'query': {
            'type': 'string',
            'minLength': 1,
            if (limit.maxQueryCharacters != null)
              'maxLength': limit.maxQueryCharacters,
          },
          'top': {
            'type': 'integer',
            'minimum': 1,
            if (limit.maxTop != null) 'maximum': limit.maxTop,
            'default': knowledgeDefaultTop,
          },
          'overlay': {'type': 'string'},
          'mode': {
            'type': 'string',
            'enum': ['auto', ...declared],
            'default': 'auto',
          },
          'path': {'type': 'string'},
        },
        'required': ['query'],
        'additionalProperties': false,
      },
    });
  }
  return tools;
}

final RegExp _ident = RegExp(r'^[A-Za-z_][A-Za-z0-9_]*$');
final BigInt _maxInteger = BigInt.from(knowledgeMaxInteger);

BigInt? _integer(Object? value) => value is BigInt ? value : null;

/// Plain JSON as the data model: an `int` becomes an integer, since on the web a `double`
/// that is whole cannot be told from one.
Object? _model(Object? value) {
  if (value is int) return BigInt.from(value);
  if (value is Map) {
    return {for (final e in value.entries) e.key: _model(e.value)};
  }
  if (value is List) return [for (final item in value) _model(item)];
  return value;
}

bool _contextOk(
  Object? value,
  int depth,
  List<int> budget,
  KnowledgeSettings limit,
) {
  final deepest = limit.maxContextDepth;
  if (deepest != null && depth > deepest) return false;
  if (value is bool) return true;
  final number = _integer(value);
  if (number != null) return number.abs() <= _maxInteger;
  final most = limit.maxContextBytes;
  if (value is String) {
    budget[0] += utf8.encode(value).length;
    return most == null || budget[0] <= most;
  }
  if (value is List) {
    return value.every((item) => _contextOk(item, depth + 1, budget, limit));
  }
  if (value is Map) {
    for (final MapEntry(key: key, value: item) in value.entries) {
      if (key is! String || !_ident.hasMatch(key)) return false;
      budget[0] += utf8.encode(key).length;
      if (most != null && budget[0] > most) return false;
      if (!_contextOk(item, depth + 1, budget, limit)) return false;
    }
    return true;
  }
  return false;
}

KnowledgeError _invalid(String why) => KnowledgeError('invalid-arguments', why);

/// A call's [arguments] checked against the tool's schema and the rules of the section, with
/// the defaults applied. A null is the argument absent. An integer is a `BigInt` (the data model's, since on the web a whole `double` is an `int`); a
/// `double` is no integer. Throws [KnowledgeError] `invalid-arguments`.
Map<String, Object?> knowledgeArguments(
  String tool,
  Object? arguments,
  Iterable<String> modes, [
  KnowledgeSettings? settings,
]) {
  final limit = settings ?? KnowledgeSettings();
  Map<String, Object?>? schema;
  for (final entry in knowledgeTools(modes, limit)) {
    if (entry['name'] == tool) {
      schema = entry['inputSchema']! as Map<String, Object?>;
    }
  }
  if (schema == null) throw _invalid('no tool is named $tool');
  if (arguments is! Map) throw _invalid('the arguments are an object');
  final properties = schema['properties']! as Map<String, Object?>;
  final out = <String, Object?>{};
  for (final MapEntry(key: key, value: value) in arguments.entries) {
    if (key is! String || !properties.containsKey(key)) {
      throw _invalid('no argument is named $key');
    }
    if (value == null) continue;
    final spec = properties[key]! as Map<String, Object?>;
    if (spec['type'] == 'string') {
      if (value is! String) throw _invalid('$key is text');
      final allowed = spec['enum'];
      if (allowed is List && !allowed.contains(value)) {
        throw _invalid('$key is not one of its values');
      }
      if (key == 'query') {
        final most = spec['maxLength'] as int?;
        final blank = value.runes.every(
          (code) => const [0x20, 0x09, 0x0a, 0x0b, 0x0c, 0x0d].contains(code),
        );
        if ((most != null && value.runes.length > most) || blank) {
          throw _invalid('the query is empty or too long');
        }
      }
    } else if (spec['type'] == 'integer') {
      final number = _integer(value);
      if (number == null) throw _invalid('$key is a whole number');
      final high = spec['maximum'] as int?;
      if (number < BigInt.from(spec['minimum']! as int) ||
          (high != null && number > BigInt.from(high))) {
        throw _invalid('$key is out of range');
      }
    } else if (value is! Map || !_contextOk(value, 1, [0], limit)) {
      throw _invalid('$key is not a usable context');
    }
    out[key] = value;
  }
  for (final key in (schema['required'] as List?) ?? const []) {
    if (!out.containsKey(key)) throw _invalid('$key is required');
  }
  if (tool == 'kb_search') {
    out.putIfAbsent('top', () => knowledgeDefaultTop);
    out.putIfAbsent('mode', () => 'auto');
  }
  return out;
}

/// A file of an overlay's knowledge that is served.
class KnowledgeFile {
  KnowledgeFile._(
    this.path,
    this.text,
    this.engine,
    this.asWritten,
    this.options,
    this.bytes,
    this.data,
    this.rest,
  );

  /// The relative path.
  final String path;

  /// The file's text as stored.
  final String text;

  /// The engine its first line names when it is none of the formats every server knows
  /// (`markdown`, `md`, `raw`, `aos`, `agentic`), else null.
  final String? engine;

  /// Whether the file is served as written: not a markdown file, or one that names `markdown`,
  /// `md` or `raw`.
  final bool asWritten;

  /// The options of its first line (`name=value` words), by name.
  final Map<String, String> options;

  /// Its size as stored.
  final int bytes;

  /// Its bytes.
  final Uint8List data;

  /// The text after the first line when that names a format, else the whole text.
  final String rest;
}

/// An overlay's knowledge: the served files and the skipped ones, both in path-byte order.
class KnowledgeSet {
  KnowledgeSet._(this.files, this.skipped);

  /// The served files by path, in order.
  final Map<String, KnowledgeFile> files;

  /// Each skipped path and its reason (`name`, `too-large` or `not-text`).
  final List<(String, String)> skipped;
}

bool _selected(String path, Document manifest) {
  if (path == 'OVERLAY.md' ||
      path == 'overlay.cfg' ||
      !validRelativePath(path)) {
    return false;
  }
  final parts = path.split('/');
  if ((parts.length > 1 && knowledgeDirs.contains(parts.first)) ||
      path == 'CONTEXT.md') {
    return true;
  }
  for (final field in const ['rules', 'context']) {
    for (final entry in declared(manifest, field)) {
      if (named(entry) == path) return true;
    }
  }
  return false;
}

bool _hiddenName(String path) => path.runes.any(_hides);

final RegExp _option = RegExp(r'^([a-z][a-z_]*)=(\S+)$');

/// The name and the options a markdown file's first line gives its format: `#!` and words,
/// the options the words of the form `name=value`, the name the last word that is not one,
/// after any `/` or `\`, with leading dots dropped and ASCII capitals lower-cased. Null when
/// the file gives no name (no `#!` at its first byte, no word, bytes that are not UTF-8).
({String name, Map<String, String> options})? knowledgeFormat(Uint8List data) {
  if (data.length < 2 || data[0] != 0x23 || data[1] != 0x21) return null;
  final end = data.indexOf(0x0a);
  final line = end < 0
      ? Uint8List.sublistView(data, 2)
      : Uint8List.sublistView(data, 2, end);
  final String text;
  try {
    text = decodeUtf8(line);
  } on AosError {
    return null;
  }
  final options = <String, String>{};
  final names = <String>[];
  for (final word in text.split(RegExp(r'[ \t\f\r]+'))) {
    if (word.isEmpty) continue;
    final found = _option.firstMatch(word);
    if (found != null) {
      options[found.group(1)!] = found.group(2)!;
    } else {
      names.add(word);
    }
  }
  if (names.isEmpty) return null;
  var name = names.last.split(RegExp(r'[/\\]')).last;
  var dots = 0;
  while (dots < name.length && name[dots] == '.') {
    dots += 1;
  }
  name = asciiLower(name.substring(dots));
  return name.isEmpty ? null : (name: name, options: options);
}

String _afterFirstLine(String text) {
  final end = text.indexOf('\n');
  return end < 0 ? '' : text.substring(end + 1);
}

const List<String> _asWritten = ['raw', 'markdown', 'md'];
const List<String> _expanded = ['aos', 'agentic'];

/// What of [overlay] is knowledge: the files under `kb/`, `flows/`, `references/`, `rules/` and
/// `context/`, the root `CONTEXT.md` and the files `rules` and `context` name, that are not a
/// manifest. A file is skipped, with its reason, when its name holds a character served text
/// writes out, it is larger than `maxFileBytes`, or its bytes are not UTF-8.
KnowledgeSet knowledgeSet(
  KnowledgeOverlay overlay, [
  KnowledgeSettings? settings,
]) {
  final limit = settings ?? KnowledgeSettings();
  final files = <String, KnowledgeFile>{};
  final skipped = <(String, String)>[];
  final paths = overlay.tree.keys.toList()..sort(compareText);
  for (final path in paths) {
    final entry = overlay.tree[path]!;
    if (entry.kind != 'file' || !_selected(path, overlay.manifest)) continue;
    if (_hiddenName(path)) {
      skipped.add((path, 'name'));
    } else if (!allows(limit.maxFileBytes, entry.size)) {
      skipped.add((path, 'too-large'));
    } else {
      final String text;
      try {
        text = decodeUtf8(entry.bytes);
      } on AosError {
        skipped.add((path, 'not-text'));
        continue;
      }
      final lower = asciiLower(path);
      final markdown = lower.endsWith('.md') || lower.endsWith('.markdown');
      final line = markdown ? knowledgeFormat(entry.bytes) : null;
      final name = line?.name;
      files[path] = KnowledgeFile._(
        path,
        text,
        name == null || _asWritten.contains(name) || _expanded.contains(name)
            ? null
            : name,
        !markdown || (name != null && _asWritten.contains(name)),
        line?.options ?? const {},
        entry.size,
        entry.bytes,
        name != null ? _afterFirstLine(text) : text,
      );
    }
  }
  return KnowledgeSet._(files, skipped);
}

List<KnowledgeOverlay> _choose(
  List<KnowledgeOverlay> overlays,
  String? wanted,
) {
  if (wanted == null) return overlays;
  final chosen = [
    for (final overlay in overlays)
      if (normalizeName(overlay.identity) == normalizeName(wanted)) overlay,
  ];
  if (chosen.isEmpty) throw KnowledgeError('unknown-overlay', wanted);
  return chosen;
}

/// The overlays a search asks, as `kb_list` orders them.
List<KnowledgeOverlay> knowledgeChosen(
  List<KnowledgeOverlay> overlays,
  String? wanted,
) => _choose(overlays, wanted);

/// The engines a call renders with, in the edition's order, then the option values the
/// server lacks (`-engine:option=value`) as given.
List<String> knowledgeEngineList(Iterable<String> engines) => [
  for (final engine in knowledgeEngines)
    if (engines.contains(engine)) engine,
  for (final entry in engines)
    if (entry.startsWith('-')) entry,
];

/// What a server answers a call of `kb_list` or `kb_read`: `{'result': {...}}`, or
/// `{'error': {'class': ..., 'message': ...}}`. [overlays] are the session's active overlays in
/// composition order; [modes] the search modes the server declares; [caller] what templates
/// read; [engines] the template engines this server renders with (both of the edition's by
/// default).
Map<String, Object?> knowledgeCall(
  String tool,
  List<KnowledgeOverlay> overlays,
  Object? arguments, {
  Iterable<String> modes = const [],
  KnowledgeSettings? settings,
  CallerFacts caller = const CallerFacts(),
  Iterable<String> engines = knowledgeEngines,
}) {
  final limit = settings ?? KnowledgeSettings();
  final rendering = knowledgeEngineList(engines);
  try {
    final args = knowledgeArguments(tool, arguments, modes, limit);
    switch (tool) {
      case 'kb_list':
        return {
          'result': _list(
            overlays,
            args,
            modes.toList(),
            limit,
            rendering,
            caller,
          ),
        };
      case 'kb_read':
        return {'result': _read(overlays, args, limit, caller, rendering)};
      default:
        throw _invalid('no tool is named $tool');
    }
  } on KnowledgeError catch (error) {
    return {
      'error': {'class': error.errorClass, 'message': error.message},
    };
  }
}

Map<String, Object?> _list(
  List<KnowledgeOverlay> overlays,
  Map<String, Object?> args,
  List<String> modes,
  KnowledgeSettings limit,
  List<String> engines,
  CallerFacts caller,
) {
  final listed = <Object?>[];
  for (final overlay in _choose(overlays, args['overlay'] as String?)) {
    final found = knowledgeSet(overlay, limit);
    final files = <Object?>[];
    final skipped = [...found.skipped];
    for (final item in found.files.values) {
      final String text;
      try {
        text = knowledgeServe(
          overlay,
          found,
          item.path,
          const {},
          caller,
          limit,
          engines,
        );
      } on KnowledgeError {
        skipped.add((item.path, 'render'));
        continue;
      }
      files.add({
        'path': item.path,
        'lines': knowledgeLines(text).length,
        'bytes': utf8Length(text),
        if (item.engine != null) 'template': item.engine,
        if (item.engine != null) 'rendered': knowledgeRenders(item, engines),
      });
    }
    skipped.sort((a, b) => compareText(a.$1, b.$1));
    listed.add({
      'overlay': overlay.identity,
      'served': overlay.served,
      'files': files,
      'skipped': [
        for (final (path, reason) in skipped)
          {'path': inert(path), 'reason': reason},
      ],
    });
  }
  final result = <String, Object?>{
    'overlays': listed,
    'engines': [
      for (final engine in engines)
        if (!engine.startsWith('-')) engine,
    ],
  };
  if (modes.isNotEmpty) {
    result['search'] = {
      'modes': [
        for (final mode in knowledgeModes)
          if (modes.contains(mode)) mode,
      ],
    };
  }
  return result;
}

const Map<String, String> _skipClass = {
  'name': 'invalid-path',
  'too-large': 'too-large',
  'not-text': 'not-text',
};
const List<String> _priority = ['link', 'not-text', 'too-large', 'not-found'];

Map<String, Object?> _read(
  List<KnowledgeOverlay> overlays,
  Map<String, Object?> args,
  KnowledgeSettings limit,
  CallerFacts caller,
  List<String> engines,
) {
  final path = args['path']! as String;
  if (!validRelativePath(path) || _hiddenName(path)) {
    throw KnowledgeError('invalid-path', inert(path));
  }
  final foundIn = <(KnowledgeOverlay, KnowledgeSet)>[];
  final problems = <String>[];
  final parts = path.split('/');
  for (final overlay in _choose(overlays, args['overlay'] as String?)) {
    final held = knowledgeSet(overlay, limit);
    var linked = false;
    for (var n = 0; n < parts.length; n += 1) {
      if (overlay.tree[parts.sublist(0, n + 1).join('/')]?.kind == 'symlink') {
        linked = true;
      }
    }
    if (linked) {
      problems.add('link');
    } else if (held.files.containsKey(path)) {
      foundIn.add((overlay, held));
    } else {
      String? reason;
      for (final (skippedPath, why) in held.skipped) {
        if (skippedPath == path) reason = why;
      }
      problems.add(reason == null ? 'not-found' : _skipClass[reason]!);
    }
  }
  if (foundIn.length > 1) {
    throw KnowledgeError('ambiguous', 'more than one overlay has that path');
  }
  if (foundIn.isEmpty) {
    problems.sort((a, b) => _priority.indexOf(a) - _priority.indexOf(b));
    throw KnowledgeError(
      problems.isEmpty ? 'not-found' : problems.first,
      inert(path),
    );
  }
  final (overlay, held) = foundIn.single;
  final engine = held.files[path]!.engine;
  final context = args['context'];
  final text = knowledgeServe(
    overlay,
    held,
    path,
    context is Map ? Map<String, Object?>.from(context) : const {},
    caller,
    limit,
    engines,
  );
  final lines = knowledgeLines(text);
  final total = lines.length;
  final start = _integer(args['from'])?.toInt() ?? 1;
  final stop = _integer(args['to'])?.toInt();
  if ((stop != null && stop < start) || start > (total < 1 ? 1 : total)) {
    throw KnowledgeError('line-range', 'the file has $total lines');
  }
  final last = stop == null || stop > total ? total : stop;
  final chosen = <String>[];
  var size = 0;
  var truncated = false;
  final most = limit.maxReadBytes;
  for (final line in lines.sublist(
    start - 1,
    last < start - 1 ? start - 1 : last,
  )) {
    final add = utf8Length(line) + (chosen.isNotEmpty ? 1 : 0);
    if (chosen.isNotEmpty && most != null && size + add > most) {
      truncated = true;
      break;
    }
    chosen.add(line);
    size += add;
  }
  return {
    'overlay': overlay.identity,
    'path': path,
    'from_line': start,
    'to_line': start - 1 + chosen.length,
    'total_lines': total,
    'truncated': truncated,
    'text': inert(chosen.join('\n')),
    'template': ?engine,
    if (engine != null)
      'rendered': knowledgeRenders(held.files[path]!, engines),
  };
}

/// A handler's [outcome] as MCP's `tools/call` result: the same JSON as `structuredContent`
/// and, as text, in one block of `content`, and `isError`.
Map<String, Object?> knowledgeResult(Map<String, Object?> outcome) {
  final failed = outcome.containsKey('error');
  final body = failed ? {'error': outcome['error']} : outcome['result'];
  return {
    'content': [
      {'type': 'text', 'text': jsonEncode(body)},
    ],
    'structuredContent': body,
    'isError': failed,
  };
}

/// `kb_read`: lines of one file of [overlays], rendered for this call when the file is a
/// template (from [caller] and [context]), answered as MCP's `tools/call` result. [from] and
/// [to] are lines from 1; [overlay] limits the overlays asked. A consumer serves this and never
/// renders a template itself.
Map<String, Object?> kbRead(
  List<KnowledgeOverlay> overlays, {
  required String path,
  Object? from,
  Object? to,
  String? overlay,
  Map<String, Object?>? context,
  CallerFacts caller = const CallerFacts(),
  Iterable<String> engines = knowledgeEngines,
  KnowledgeSettings? settings,
}) => knowledgeResult(
  knowledgeCall(
    'kb_read',
    overlays,
    {
      'path': path,
      'from': _model(from),
      'to': _model(to),
      'overlay': overlay,
      'context': _model(context),
    },
    settings: settings,
    caller: caller,
    engines: engines,
  ),
);

/// `kb_list`: each overlay's served files and what is skipped, as MCP's `tools/call` result.
/// [modes] are the modes the server declares, which `search` lists.
Map<String, Object?> kbList(
  List<KnowledgeOverlay> overlays, {
  String? overlay,
  Iterable<String> modes = const [],
  CallerFacts caller = const CallerFacts(),
  Iterable<String> engines = knowledgeEngines,
  KnowledgeSettings? settings,
}) => knowledgeResult(
  knowledgeCall(
    'kb_list',
    overlays,
    {'overlay': overlay},
    modes: modes,
    settings: settings,
    caller: caller,
    engines: engines,
  ),
);

/// Whether an overlay's `rules` and `context` files are left out of the context because they
/// are served, and which.
class KnowledgePlan {
  /// Creates the plan.
  const KnowledgePlan(this.served, this.leftOut);

  /// Whether the overlay is served.
  final bool served;

  /// The files left out of the context; the body of `OVERLAY.md` never is.
  final List<String> leftOut;
}

/// The plan for an overlay with [manifest] and [tree]: the manifest's `[knowledge] serve` hint,
/// or [user]'s choice, which wins over it either way.
KnowledgePlan knowledgePlan(Document manifest, OverlayTree tree, [bool? user]) {
  final hint = asMap(manifest['knowledge'])?['serve'] == true;
  final served = user ?? hint;
  final left = <String>[];
  if (served) {
    for (final path in [
      ...rulesOf(manifest, tree),
      ...contextOf(manifest, tree),
    ]) {
      if (path != 'OVERLAY.md' && !left.contains(path)) left.add(path);
    }
  }
  return KnowledgePlan(served, left);
}

/// Reads the tree under [root] through [files] without following a link: a link is recorded as
/// a link and never entered, a file larger than `maxFileBytes` keeps its size and only the start
/// of its bytes, and a name that is not Unicode, or that no overlay may name, is left out. The
/// limits `treeFiles` and `treeDepth` of [limits] bound the walk.
Future<Map<String, KnowledgeEntry>> readKnowledgeTree(
  FileSystem files,
  String root, {
  Settings? limits,
  KnowledgeSettings? settings,
}) async {
  final bounds = limits ?? Settings();
  final served = settings ?? KnowledgeSettings();
  final found = <String, KnowledgeEntry>{};
  var entries = 0;

  Future<void> walk(String directory, String prefix, int depth) async {
    if (!allows(bounds.treeDepth, depth)) {
      throw AosError(
        'too-large',
        'directories nested too deep',
      ).over('tree_depth');
    }
    final List<Object> names;
    try {
      names = await files.list(directory);
    } on Object {
      throw AosError('io', 'the filesystem');
    }
    for (final name in names) {
      if (name is! String ||
          name.isEmpty ||
          name == '.' ||
          name == '..' ||
          name.contains('/') ||
          name.contains(r'\') ||
          name.contains('\u0000')) {
        continue;
      }
      final path = prefix.isEmpty ? name : '$prefix/$name';
      if (!validRelativePath(path)) continue;
      entries += 1;
      if (!allows(bounds.treeFiles, entries)) {
        throw AosError('too-large', 'too many files').over('tree_files');
      }
      final Metadata metadata;
      try {
        metadata = await files.metadata(joinPath(directory, name));
      } on Object {
        throw AosError('io', 'the filesystem');
      }
      switch (metadata.kind) {
        case FileKind.directory:
          found[path] = KnowledgeEntry.directory();
          await walk(joinPath(directory, name), path, depth + 1);
        case FileKind.symlink:
          found[path] = KnowledgeEntry.symlink();
        case FileKind.other:
          found[path] = KnowledgeEntry.special();
        case FileKind.file:
          final most = served.maxFileBytes;
          final Uint8List bytes;
          try {
            bytes = await files.read(
              joinPath(directory, name),
              most == null ? metadata.len : most + 1,
            );
          } on Object {
            throw AosError('io', 'the filesystem');
          }
          found[path] = KnowledgeEntry.file(bytes, size: metadata.len);
      }
    }
  }

  await walk(root, '', 1);
  return found;
}
