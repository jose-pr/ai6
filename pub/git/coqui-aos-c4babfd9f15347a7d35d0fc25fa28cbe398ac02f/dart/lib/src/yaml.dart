/// One YAML 1.2 document under the core schema, with every tag, anchor, alias, merge key and
/// directive refused. Nothing is expanded, so an alias cannot grow a document.
library;

// The package's parser emits the events (tag, anchor, alias, scalar style) the public loader
// resolves away; reading them is the only way to refuse those before anything is composed.
// ignore: implementation_imports
import 'package:yaml/src/event.dart';
// ignore: implementation_imports
import 'package:yaml/src/parser.dart';
// ignore: implementation_imports
import 'package:yaml/src/style.dart';
import 'package:yaml/yaml.dart' show YamlException;

import 'error.dart';
import 'json.dart';
import 'limits.dart';

final RegExp _unicodeBreaks = RegExp('[\u0085  ]');

/// The deepest run of open `[` and `{` outside quotes and comments, which a parser that stops
/// early reports only as its own failure.
int nesting(String text) {
  var open = 0;
  var deepest = 0;
  int? quote;
  var comment = false;
  var escaped = false;
  for (var at = 0; at < text.length; at += 1) {
    final c = text.codeUnitAt(at);
    if (comment) {
      comment = c != 0x0a;
      continue;
    }
    if (quote != null) {
      if (escaped) {
        escaped = false;
      } else if (c == 0x5c && quote == 0x22) {
        escaped = true;
      } else if (c == quote || c == 0x0a) {
        quote = null;
      }
      continue;
    }
    if (c == 0x22 || c == 0x27) {
      quote = c;
    } else if (c == 0x23) {
      comment = true;
    } else if (c == 0x5b || c == 0x7b) {
      open += 1;
      if (open > deepest) deepest = open;
    } else if (c == 0x5d || c == 0x7d) {
      open = open > 0 ? open - 1 : 0;
    }
  }
  return deepest;
}

final RegExp _directiveStart = RegExp(r'^%', multiLine: true);
final RegExp _documentStart = RegExp(r'^---(?=[ \t\r\n]|$)', multiLine: true);

/// A line starting `%` before the first `---` is a directive, which is refused.
void _refuseDirective(String text) {
  final directive = _directiveStart.firstMatch(text);
  if (directive == null) return;
  final start = _documentStart.firstMatch(text);
  if (start == null || directive.start < start.start) {
    throw _failure(
      'yaml-directive',
      'a %YAML or %TAG directive',
      text,
      directive.start,
    );
  }
}

/// Nesting of flow collections: an opening bracket counts where a flow collection can start
/// (after a blank, a bracket or a comma), so a plain scalar holding brackets does not.
int flowNesting(String text) {
  var open = 0;
  var deepest = 0;
  int? quote;
  var comment = false;
  var escaped = false;
  var previous = 0x0a;
  for (var at = 0; at < text.length; at += 1) {
    final c = text.codeUnitAt(at);
    final before = previous;
    previous = c;
    if (comment) {
      comment = c != 0x0a;
      continue;
    }
    if (quote != null) {
      if (escaped) {
        escaped = false;
      } else if (c == 0x5c && quote == 0x22) {
        escaped = true;
      } else if (c == quote || c == 0x0a) {
        quote = null;
      }
      continue;
    }
    final startsToken =
        before == 0x0a ||
        before == 0x20 ||
        before == 0x09 ||
        before == 0x5b ||
        before == 0x7b ||
        before == 0x2c;
    if ((c == 0x22 || c == 0x27) && startsToken) {
      quote = c;
    } else if (c == 0x23 && startsToken) {
      comment = true;
    } else if ((c == 0x5b || c == 0x7b) && (startsToken || open > 0)) {
      open += 1;
      if (open > deepest) deepest = open;
    } else if ((c == 0x5d || c == 0x7d) && open > 0) {
      open -= 1;
    }
  }
  return deepest;
}

final RegExp _pairEscape = RegExp(
  r'\\u[dD][89abAB][0-9a-fA-F]{2}\\u[dD][c-fC-F][0-9a-fA-F]{2}',
);

/// Writes a surrogate pair escaped as two short escapes in a double-quoted scalar as one long escape,
/// which the parser reads where it refuses each half.
String _joinSurrogatePairs(String text) {
  if (!_pairEscape.hasMatch(text)) return text;
  final out = StringBuffer();
  var quoted = false;
  var at = 0;
  while (at < text.length) {
    final c = text.codeUnitAt(at);
    if (!quoted) {
      if (c == 0x22) quoted = true;
      out.writeCharCode(c);
      at += 1;
    } else if (c == 0x5c) {
      final match = _pairEscape.matchAsPrefix(text, at);
      if (match != null) {
        final high = int.parse(text.substring(at + 2, at + 6), radix: 16);
        final low = int.parse(text.substring(at + 8, at + 12), radix: 16);
        final code = 0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00);
        out.write('\\U${code.toRadixString(16).padLeft(8, '0')}');
        at += 12;
      } else {
        out.write(text.substring(at, at + (at + 1 < text.length ? 2 : 1)));
        at += at + 1 < text.length ? 2 : 1;
      }
    } else {
      if (c == 0x22 || c == 0x0a) quoted = false;
      out.writeCharCode(c);
      at += 1;
    }
  }
  return out.toString();
}

AosError _failure(String cls, String detail, String text, int offset) =>
    atOffset(AosError(cls, detail), text, offset);

// What the walk needs of one event.
enum _Kind { scalar, sequence, mapping, end, alias }

class _Node {
  _Node(
    this.kind,
    this.offset, {
    this.value,
    this.plain = false,
    this.tag = false,
    this.anchor = false,
  });

  final _Kind kind;
  final int offset;
  final String? value;
  final bool plain;
  final bool tag;
  final bool anchor;
}

/// Reads [text] as one YAML document into the data model.
Object readYaml(String text, Limit depth) {
  if (_unicodeBreaks.hasMatch(text)) {
    throw AosError(
      'parse-error',
      'a Unicode line break YAML 1.1 and 1.2 read differently',
    );
  }
  if (text.indexOf('﻿', 1) > 0) {
    throw AosError('parse-error', 'a byte-order mark on a later line');
  }
  _refuseDirective(text);
  if (!allows(depth, flowNesting(text))) {
    // The parser's cost grows with the square of the nesting, so a document this deep is refused
    // before it is scanned.
    throw tooDeep(depth);
  }
  final nodes = <_Node>[];
  try {
    final parser = Parser(_joinSurrogatePairs(text));
    var documents = 0;
    while (!parser.isDone) {
      final event = parser.parse();
      final offset = event.span.start.offset;
      switch (event.type) {
        case EventType.documentStart:
          event as DocumentStartEvent;
          if (event.versionDirective != null ||
              event.tagDirectives.isNotEmpty) {
            throw _failure(
              'yaml-directive',
              'a %YAML or %TAG directive',
              text,
              offset,
            );
          }
          documents += 1;
          if (documents > 1) {
            throw _failure('parse-error', 'not valid YAML', text, offset);
          }
        case EventType.alias:
          nodes.add(_Node(_Kind.alias, offset));
        case EventType.scalar:
          event as ScalarEvent;
          nodes.add(
            _Node(
              _Kind.scalar,
              offset,
              value: event.value,
              plain: event.style == ScalarStyle.PLAIN,
              // The parser gives every quoted or block scalar the tag `!` by default.
              tag:
                  event.tag != null &&
                  !(event.tag == '!' && event.style != ScalarStyle.PLAIN),
              anchor: event.anchor != null,
            ),
          );
        case EventType.sequenceStart:
          event as SequenceStartEvent;
          nodes.add(
            _Node(
              _Kind.sequence,
              offset,
              tag: event.tag != null,
              anchor: event.anchor != null,
            ),
          );
        case EventType.mappingStart:
          event as MappingStartEvent;
          nodes.add(
            _Node(
              _Kind.mapping,
              offset,
              tag: event.tag != null,
              anchor: event.anchor != null,
            ),
          );
        case EventType.sequenceEnd:
        case EventType.mappingEnd:
          nodes.add(_Node(_Kind.end, offset));
        case EventType.streamStart:
        case EventType.streamEnd:
        case EventType.documentEnd:
          break;
      }
    }
  } on YamlException catch (error) {
    final spanText = error.span?.text ?? '';
    if (error.message == 'Invalid Unicode character escape code.') {
      final hex = RegExp(r'[0-9a-fA-F]+$').firstMatch(spanText)?[0];
      final code = hex == null ? null : int.tryParse(hex, radix: 16);
      if (code != null && code >= 0xd800 && code <= 0xdfff) {
        throw _failure(
          'invalid-unicode',
          'an escape that is no character',
          text,
          error.span?.start.offset ?? 0,
        );
      }
    }
    if (spanText.startsWith('!<')) {
      // A verbatim tag, which the parser does not read.
      throw _failure('yaml-tag', 'a tag', text, error.span?.start.offset ?? 0);
    }
    final deep = !allows(depth, nesting(text));
    final offset = error.span?.start.offset ?? 0;
    final refusal = _failure(
      deep ? 'too-deep' : 'parse-error',
      'not valid YAML',
      text,
      offset,
    );
    throw deep ? refusal.over('manifest_depth') : refusal;
  }
  if (nodes.isEmpty) return <String, Object>{};
  final first = nodes.first;
  if (first.kind == _Kind.scalar &&
      first.plain &&
      first.value == '' &&
      !first.tag &&
      !first.anchor &&
      nodes.length == 1) {
    return <String, Object>{};
  }
  return _walk(nodes, depth, text);
}

enum _Typed { text, boolean, integer, float }

final RegExp _digitsOnly = RegExp(r'^[0-9]+$');
final RegExp _octalOrHex = RegExp(r'^(0o[0-7]+|0x[0-9a-fA-F]+)$');
final RegExp _floatShape = RegExp(
  r'^(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$',
);
final RegExp _infinity = RegExp(r'^\.(inf|Inf|INF)$');

/// A plain scalar's type under the YAML 1.2 core schema, and its value.
(_Typed, Object?) _coreScalar(String value) {
  if (value == '' ||
      value == '~' ||
      value == 'null' ||
      value == 'Null' ||
      value == 'NULL') {
    throw AosError('null-value', 'a null');
  }
  if (value == 'true' || value == 'True' || value == 'TRUE') {
    return (_Typed.boolean, true);
  }
  if (value == 'false' || value == 'False' || value == 'FALSE') {
    return (_Typed.boolean, false);
  }
  final signed = value.startsWith('-') || value.startsWith('+');
  final unsigned = signed ? value.substring(1) : value;
  if (_digitsOnly.hasMatch(unsigned)) {
    final magnitude = BigInt.parse(unsigned);
    return (
      _Typed.integer,
      integer64(value.startsWith('-') ? -magnitude : magnitude),
    );
  }
  if (_octalOrHex.hasMatch(value)) {
    final octal = value.startsWith('0o');
    final parsed = BigInt.parse(value.substring(2), radix: octal ? 8 : 16);
    return (_Typed.integer, integer64(parsed));
  }
  if (_floatShape.hasMatch(unsigned)) {
    return (_Typed.float, finiteFloat(double.parse(value)));
  }
  if (_infinity.hasMatch(unsigned) ||
      value == '.nan' ||
      value == '.NaN' ||
      value == '.NAN') {
    throw AosError('non-finite-float', 'a float that is not finite');
  }
  return (_Typed.text, null);
}

/// The events as the data model.
Object _walk(List<_Node> nodes, Limit depth, String text) {
  final builder = Builder(depth, false);
  for (final node in nodes) {
    try {
      if (node.kind == _Kind.end) {
        builder.close();
        continue;
      }
      if (node.kind == _Kind.alias) throw AosError('yaml-anchor', 'an alias');
      if (node.tag) throw AosError('yaml-tag', 'a tag');
      if (node.anchor) throw AosError('yaml-anchor', 'an anchor');
      if (node.kind == _Kind.scalar) {
        final written = wellFormed(node.value!);
        final (kind, typed) = node.plain
            ? _coreScalar(written)
            : (_Typed.text, null);
        if (builder.wantsKey()) {
          if (kind != _Typed.text) {
            throw AosError(
              'non-string-key',
              'a mapping key that is not a string',
            );
          }
          if (written == '<<') throw AosError('yaml-merge-key', 'a merge key');
          builder.key(written);
        } else {
          builder.value(kind == _Typed.text ? written : typed);
        }
      } else if (builder.wantsKey()) {
        throw AosError('non-string-key', 'a mapping key that is not a string');
      } else if (node.kind == _Kind.sequence) {
        builder.openList();
      } else {
        builder.openMap();
      }
    } on AosError catch (error) {
      if (error.line == null) atOffset(error, text, node.offset);
      rethrow;
    }
  }
  return builder.root ?? <String, Object>{};
}
