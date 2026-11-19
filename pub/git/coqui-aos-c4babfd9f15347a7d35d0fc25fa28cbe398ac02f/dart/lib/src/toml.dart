/// TOML 1.0 read into the data model.
///
/// A date or time is refused (`datetime-value`), an integer is a `BigInt` held to the signed
/// 64-bit range, and a key defined twice in any way is `duplicate-key`. Values nest without
/// recursion, so a document nested far past the bound is refused as `too-deep`, not a stack
/// overflow.
library;

import 'error.dart';
import 'json.dart';
import 'limits.dart';
import 'value.dart';
import 'yaml.dart' show nesting;

/// A date, a time or both: kept only so the walk can refuse it.
class _DateTime {
  const _DateTime();
}

const _DateTime _dateTime = _DateTime();

/// Reads [text] as one TOML document into the data model.
Object readToml(String text, Limit depth) {
  final _Parser parser = _Parser(text);
  Document root;
  try {
    root = parser.document();
  } on AosError catch (error) {
    if (!allows(depth, nesting(text))) {
      throw tooDeep(depth).at(error.line ?? 1, error.column ?? 1);
    }
    rethrow;
  } on StackOverflowError {
    throw tooDeep(depth);
  } on RangeError catch (error) {
    if (error.toString().contains('call stack')) throw tooDeep(depth);
    rethrow;
  }
  try {
    _validate(root, depth);
  } on StackOverflowError {
    throw tooDeep(depth);
  }
  return root;
}

/// Walks the finished document in order: nesting past [depth], dates, integers past 64 bits and
/// floats that are not finite are refused.
void _validate(Document root, Limit depth) {
  final stack = <(Object, int)>[(root, 1)];
  while (stack.isNotEmpty) {
    final (item, level) = stack.removeLast();
    if (item is _DateTime) {
      throw AosError('datetime-value', 'a date or time has no AOS type');
    } else if (item is BigInt) {
      integer64(item);
    } else if (item is double) {
      finiteFloat(item);
    } else if (item is List<Object>) {
      if (!allows(depth, level)) throw tooDeep(depth);
      for (var at = item.length - 1; at >= 0; at -= 1) {
        stack.add((item[at], level + 1));
      }
    } else if (item is Map<String, Object>) {
      if (!allows(depth, level)) throw tooDeep(depth);
      for (final value in item.values.toList().reversed) {
        stack.add((value, level + 1));
      }
    }
  }
}

bool _isDigit(int c) => c >= 0x30 && c <= 0x39;

bool _isBare(int c) =>
    (c >= 0x41 && c <= 0x5a) ||
    (c >= 0x61 && c <= 0x7a) ||
    _isDigit(c) ||
    c == 0x2d ||
    c == 0x5f;

final RegExp _decimalInt = RegExp(r'^[+-]?(0|[1-9](_?[0-9])*)$');
final RegExp _hexInt = RegExp(r'^0x[0-9a-fA-F](_?[0-9a-fA-F])*$');
final RegExp _octInt = RegExp(r'^0o[0-7](_?[0-7])*$');
final RegExp _binInt = RegExp(r'^0b[01](_?[01])*$');
final RegExp _float = RegExp(
  r'^[+-]?(0|[1-9](_?[0-9])*)(\.[0-9](_?[0-9])*)?([eE][+-]?[0-9](_?[0-9])*)?$',
);
final RegExp _dateOnly = RegExp(r'^[0-9]{4}-[0-9]{2}-[0-9]{2}$');
final RegExp _timeLike = RegExp(r'^[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?$');
final RegExp _dateTime2 = RegExp(
  r'^([0-9]{4})-([0-9]{2})-([0-9]{2})([Tt ]([0-9]{2}):([0-9]{2}):([0-9]{2})(\.[0-9]+)?([Zz]|[+-]([0-9]{2}):([0-9]{2}))?)?$',
);
final RegExp _localTime = RegExp(
  r'^([0-9]{2}):([0-9]{2}):([0-9]{2})(\.[0-9]+)?$',
);

bool _validDate(int year, int month, int day) {
  if (month < 1 || month > 12 || day < 1) return false;
  const days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  final leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
  final limit = month == 2 && leap ? 29 : days[month - 1];
  return day <= limit;
}

sealed class _Frame {}

final class _ArrayFrame extends _Frame {
  final List<Object> items = [];
}

final class _InlineFrame extends _Frame {
  final Document table = {};
  List<String>? key;
  bool afterComma = false;
}

class _Parser {
  _Parser(this.text);

  final String text;
  int at = 0;

  // Tables and arrays the document may no longer extend, by identity.
  final Set<Object> _inline = Set.identity();
  final Set<Object> _explicit = Set.identity();
  final Set<Object> _dotted = Set.identity();
  final Set<Object> _tableArrays = Set.identity();

  Never _fail([String cls = 'parse-error', String detail = 'not valid TOML']) =>
      throw atOffset(AosError(cls, detail), text, at);

  Never _duplicate() => _fail('duplicate-key', 'a key defined twice');

  int _code([int offset = 0]) {
    final index = at + offset;
    return index < text.length ? text.codeUnitAt(index) : -1;
  }

  bool get _done => at >= text.length;

  void _blank() {
    while (_code() == 0x20 || _code() == 0x09) {
      at += 1;
    }
  }

  // A comment to the end of the line, if there is one.
  void _comment() {
    if (_code() != 0x23) return;
    while (!_done && _code() != 0x0a) {
      final c = _code();
      if (c == 0x0d && _code(1) == 0x0a) break;
      if ((c < 0x20 && c != 0x09) || c == 0x7f) _fail();
      at += 1;
    }
  }

  bool _newline() {
    if (_code() == 0x0a) {
      at += 1;
      return true;
    }
    if (_code() == 0x0d && _code(1) == 0x0a) {
      at += 2;
      return true;
    }
    return false;
  }

  // The rest of a line after a statement: blanks, a comment, then a newline or the end.
  void _endOfLine() {
    _blank();
    _comment();
    if (!_done && !_newline()) _fail();
  }

  Document document() {
    final root = <String, Object>{};
    _explicit.add(root);
    var table = root;
    for (;;) {
      _blank();
      _comment();
      if (_done) break;
      if (_newline()) continue;
      final c = _code();
      if (c == 0x5b) {
        table = _header(root);
        _endOfLine();
      } else {
        _keyValue(table);
        _endOfLine();
      }
    }
    return root;
  }

  // `[a.b]` or `[[a.b]]`: the table the following key/values belong to.
  Document _header(Document root) {
    at += 1;
    final isArray = _code() == 0x5b;
    if (isArray) at += 1;
    _blank();
    final path = _keyPath();
    _blank();
    if (_code() != 0x5d) _fail();
    at += 1;
    if (isArray) {
      if (_code() != 0x5d) _fail();
      at += 1;
    }
    var table = root;
    for (var index = 0; index < path.length - 1; index += 1) {
      table = _descend(table, path[index]);
    }
    final last = path.last;
    final existing = table[last];
    if (isArray) {
      if (existing == null) {
        final created = <String, Object>{};
        _explicit.add(created);
        final list = <Object>[created];
        _tableArrays.add(list);
        table[last] = list;
        return created;
      }
      if (existing is List<Object> && _tableArrays.contains(existing)) {
        final created = <String, Object>{};
        _explicit.add(created);
        existing.add(created);
        return created;
      }
      _duplicate();
    }
    if (existing == null) {
      final created = <String, Object>{};
      _explicit.add(created);
      table[last] = created;
      return created;
    }
    if (existing is Document &&
        !_explicit.contains(existing) &&
        !_dotted.contains(existing) &&
        !_inline.contains(existing)) {
      // A table made implicit by an earlier deeper header may be defined once.
      _explicit.add(existing);
      return existing;
    }
    _duplicate();
  }

  // The table a path segment names inside [table], made when missing (implicitly).
  Document _descend(Document table, String key) {
    final existing = table[key];
    if (existing == null) {
      final created = <String, Object>{};
      table[key] = created;
      return created;
    }
    if (existing is Document) {
      if (_inline.contains(existing)) _duplicate();
      if (_dotted.contains(existing)) _duplicate();
      return existing;
    }
    if (existing is List<Object> && _tableArrays.contains(existing)) {
      return existing.last as Document;
    }
    _duplicate();
  }

  void _keyValue(Document table) {
    final path = _keyPath();
    _blank();
    if (_code() != 0x3d) _fail();
    at += 1;
    _blank();
    _assign(table, path, _value());
  }

  // Sets `path` inside [table]; the tables a dotted key makes on the way are marked so a header
  // cannot reopen them.
  void _assign(Document table, List<String> path, Object value) {
    var current = table;
    for (var index = 0; index < path.length - 1; index += 1) {
      final key = path[index];
      final existing = current[key];
      if (existing == null) {
        final created = <String, Object>{};
        _dotted.add(created);
        current[key] = created;
        current = created;
      } else if (existing is Document &&
          _dotted.contains(existing) &&
          !_inline.contains(existing)) {
        current = existing;
      } else {
        _duplicate();
      }
    }
    final last = path.last;
    if (current.containsKey(last)) _duplicate();
    current[last] = value;
  }

  List<String> _keyPath() {
    final path = <String>[];
    for (;;) {
      _blank();
      path.add(_key());
      _blank();
      if (_code() == 0x2e) {
        at += 1;
        continue;
      }
      return path;
    }
  }

  String _key() {
    final c = _code();
    if (c == 0x22) {
      at += 1;
      return _basicString(false);
    }
    if (c == 0x27) {
      at += 1;
      return _literalString(false);
    }
    final start = at;
    while (_isBare(_code())) {
      at += 1;
    }
    if (at == start) _fail();
    return text.substring(start, at);
  }

  // A basic string after its opening quote(s). Multi-line when [multi].
  String _basicString(bool multi) {
    final out = StringBuffer();
    if (multi) _newline();
    for (;;) {
      if (_done) _fail();
      final c = _code();
      if (c == 0x22) {
        if (!multi) {
          at += 1;
          return out.toString();
        }
        var run = 0;
        while (_code(run) == 0x22) {
          run += 1;
        }
        if (run >= 3) {
          if (run > 5) _fail();
          out.write('"' * (run - 3));
          at += run;
          return out.toString();
        }
        out.write('"' * run);
        at += run;
      } else if (c == 0x5c) {
        at += 1;
        _escape(out, multi);
      } else if (c == 0x0a || (c == 0x0d && _code(1) == 0x0a)) {
        if (!multi) _fail();
        out.write(c == 0x0a ? '\n' : '\r\n');
        at += c == 0x0a ? 1 : 2;
      } else if ((c < 0x20 && c != 0x09) || c == 0x7f) {
        _fail();
      } else {
        out.writeCharCode(c);
        at += 1;
      }
    }
  }

  void _escape(StringBuffer out, bool multi) {
    final c = _code();
    switch (c) {
      case 0x62:
        out.write('\b');
      case 0x74:
        out.write('\t');
      case 0x6e:
        out.write('\n');
      case 0x66:
        out.write('\f');
      case 0x72:
        out.write('\r');
      case 0x22:
        out.write('"');
      case 0x5c:
        out.write(r'\');
      case 0x75:
        at += 1;
        out.write(_unicodeEscape(4));
        return;
      case 0x55:
        at += 1;
        out.write(_unicodeEscape(8));
        return;
      default:
        if (multi && (c == 0x20 || c == 0x09 || c == 0x0a || c == 0x0d)) {
          // A line-ending backslash: blanks and newlines up to the next character are dropped.
          var look = at;
          while (look < text.length &&
              (text.codeUnitAt(look) == 0x20 ||
                  text.codeUnitAt(look) == 0x09)) {
            look += 1;
          }
          if (look < text.length &&
              (text.codeUnitAt(look) == 0x0a ||
                  (text.codeUnitAt(look) == 0x0d &&
                      look + 1 < text.length &&
                      text.codeUnitAt(look + 1) == 0x0a))) {
            at = look;
            while (!_done &&
                (_code() == 0x20 ||
                    _code() == 0x09 ||
                    _code() == 0x0a ||
                    (_code() == 0x0d && _code(1) == 0x0a))) {
              at += _code() == 0x0d ? 2 : 1;
            }
            return;
          }
        }
        _fail();
    }
    at += 1;
  }

  String _unicodeEscape(int digits) {
    if (at + digits > text.length) _fail();
    final hex = text.substring(at, at + digits);
    if (!RegExp(r'^[0-9a-fA-F]+$').hasMatch(hex)) _fail();
    final code = int.parse(hex, radix: 16);
    if ((code >= 0xd800 && code < 0xe000) || code > 0x10ffff) {
      _fail('invalid-unicode', 'an escape that is no character');
    }
    at += digits;
    return String.fromCharCode(code);
  }

  String _literalString(bool multi) {
    final out = StringBuffer();
    if (multi) _newline();
    for (;;) {
      if (_done) _fail();
      final c = _code();
      if (c == 0x27) {
        if (!multi) {
          at += 1;
          return out.toString();
        }
        var run = 0;
        while (_code(run) == 0x27) {
          run += 1;
        }
        if (run >= 3) {
          if (run > 5) _fail();
          out.write("'" * (run - 3));
          at += run;
          return out.toString();
        }
        out.write("'" * run);
        at += run;
      } else if (c == 0x0a || (c == 0x0d && _code(1) == 0x0a)) {
        if (!multi) _fail();
        out.write(c == 0x0a ? '\n' : '\r\n');
        at += c == 0x0a ? 1 : 2;
      } else if ((c < 0x20 && c != 0x09) || c == 0x7f) {
        _fail();
      } else {
        out.writeCharCode(c);
        at += 1;
      }
    }
  }

  // Skips blanks, and inside an array also comments and newlines.
  void _space(bool inArray) {
    for (;;) {
      _blank();
      if (!inArray) return;
      if (_code() == 0x23) {
        _comment();
        continue;
      }
      if (!_newline()) return;
    }
  }

  // One value, arrays and inline tables included, read with an explicit stack.
  Object _value() {
    final stack = <_Frame>[];
    for (;;) {
      final inArray = stack.isNotEmpty && stack.last is _ArrayFrame;
      _space(inArray);
      Object? finished;
      final top = stack.isEmpty ? null : stack.last;
      if (top is _InlineFrame && top.key == null) {
        // Inside an inline table, expecting a key or the closing brace.
        if (_code() == 0x7d) {
          if (top.afterComma) _fail();
          at += 1;
          stack.removeLast();
          _inline.add(top.table);
          finished = top.table;
        } else {
          top.afterComma = false;
          top.key = _keyPath();
          _blank();
          if (_code() != 0x3d) _fail();
          at += 1;
          _blank();
          continue;
        }
      } else if (top is _ArrayFrame && _code() == 0x5d) {
        at += 1;
        stack.removeLast();
        finished = top.items;
      } else {
        final c = _code();
        if (c == 0x5b) {
          at += 1;
          stack.add(_ArrayFrame());
          continue;
        } else if (c == 0x7b) {
          at += 1;
          stack.add(_InlineFrame());
          continue;
        }
        finished = _scalar();
      }
      // Attach the finished value to its parent, then read what follows it.
      for (;;) {
        final parent = stack.isEmpty ? null : stack.last;
        if (parent == null) return finished!;
        if (parent is _ArrayFrame) {
          parent.items.add(finished!);
          _space(true);
          if (_code() == 0x2c) {
            at += 1;
            break;
          }
          if (_code() == 0x5d) {
            at += 1;
            stack.removeLast();
            finished = parent.items;
            continue;
          }
          _fail();
        } else {
          final inline = parent as _InlineFrame;
          _assign(inline.table, inline.key!, finished!);
          inline.key = null;
          _blank();
          if (_code() == 0x2c) {
            at += 1;
            inline.afterComma = true;
            break;
          }
          if (_code() == 0x7d) {
            at += 1;
            stack.removeLast();
            _inline.add(inline.table);
            finished = inline.table;
            continue;
          }
          _fail();
        }
      }
    }
  }

  Object _scalar() {
    final c = _code();
    if (c == 0x22) {
      if (_code(1) == 0x22 && _code(2) == 0x22) {
        at += 3;
        return _basicString(true);
      }
      at += 1;
      return _basicString(false);
    }
    if (c == 0x27) {
      if (_code(1) == 0x27 && _code(2) == 0x27) {
        at += 3;
        return _literalString(true);
      }
      at += 1;
      return _literalString(false);
    }
    // A bare token runs to a delimiter. A date and a time may be joined by one space.
    final start = at;
    while (!_done) {
      final d = _code();
      if (d == 0x20 ||
          d == 0x09 ||
          d == 0x0a ||
          d == 0x0d ||
          d == 0x2c ||
          d == 0x5d ||
          d == 0x7d ||
          d == 0x23) {
        break;
      }
      at += 1;
    }
    var token = text.substring(start, at);
    if (_dateOnly.hasMatch(token) &&
        _code() == 0x20 &&
        _isDigit(_code(1)) &&
        _isDigit(_code(2)) &&
        _code(3) == 0x3a) {
      final timeStart = at + 1;
      at += 1;
      while (!_done) {
        final d = _code();
        if (d == 0x20 ||
            d == 0x09 ||
            d == 0x0a ||
            d == 0x0d ||
            d == 0x2c ||
            d == 0x5d ||
            d == 0x7d ||
            d == 0x23) {
          break;
        }
        at += 1;
      }
      token = '$token ${text.substring(timeStart, at)}';
    }
    if (token == '') _fail();
    return _bare(token, start);
  }

  Object _bare(String token, int start) {
    if (token == 'true') return true;
    if (token == 'false') return false;
    if (_decimalInt.hasMatch(token)) {
      return BigInt.parse(token.replaceAll('_', ''));
    }
    if (_hexInt.hasMatch(token)) {
      return BigInt.parse(token.substring(2).replaceAll('_', ''), radix: 16);
    }
    if (_octInt.hasMatch(token)) {
      return BigInt.parse(token.substring(2).replaceAll('_', ''), radix: 8);
    }
    if (_binInt.hasMatch(token)) {
      return BigInt.parse(token.substring(2).replaceAll('_', ''), radix: 2);
    }
    if (token == 'inf' || token == '+inf') return double.infinity;
    if (token == '-inf') return double.negativeInfinity;
    if (token == 'nan' || token == '+nan' || token == '-nan') return double.nan;
    if (_float.hasMatch(token)) {
      return double.parse(token.replaceAll('_', ''));
    }
    if (_dateOnly.hasMatch(token) ||
        _timeLike.hasMatch(token) ||
        _dateTime2.hasMatch(token)) {
      return _checkDateTime(token);
    }
    at = start;
    _fail();
  }

  Object _checkDateTime(String token) {
    final full = _dateTime2.firstMatch(token);
    if (full != null) {
      if (!_validDate(
        int.parse(full[1]!),
        int.parse(full[2]!),
        int.parse(full[3]!),
      )) {
        _fail();
      }
      if (full[4] != null) {
        if (int.parse(full[5]!) > 23 ||
            int.parse(full[6]!) > 59 ||
            int.parse(full[7]!) > 60) {
          _fail();
        }
        if (full[10] != null &&
            (int.parse(full[10]!) > 23 || int.parse(full[11]!) > 59)) {
          _fail();
        }
      }
      return _dateTime;
    }
    final time = _localTime.firstMatch(token);
    if (time != null &&
        (int.parse(time[1]!) > 23 ||
            int.parse(time[2]!) > 59 ||
            int.parse(time[3]!) > 60)) {
      _fail();
    }
    return _dateTime;
  }
}
