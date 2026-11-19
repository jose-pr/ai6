/// RFC 8259 JSON read into the data model, and the builder every format shares.
library;

import 'error.dart';
import 'limits.dart';
import 'text.dart';

final BigInt _i64Min = -(BigInt.one << 63);
final BigInt _i64Max = (BigInt.one << 63) - BigInt.one;

AosError _refusal(String cls, String detail) => AosError(cls, detail);

/// [value] itself, or `integer-range` outside the signed 64-bit range.
BigInt integer64(BigInt value) {
  if (value < _i64Min || value > _i64Max) {
    throw _refusal(
      'integer-range',
      'an integer outside the signed 64-bit range',
    );
  }
  return value;
}

/// [value] itself, or `non-finite-float` when it is infinite or not a number.
double finiteFloat(double value) {
  if (value.isInfinite || value.isNaN) {
    throw _refusal('non-finite-float', 'a float that is not finite');
  }
  return value;
}

/// The text itself, or `invalid-unicode` when it holds a lone surrogate.
String wellFormed(String text) {
  if (hasLoneSurrogate(text)) {
    throw _refusal('invalid-unicode', 'a lone surrogate');
  }
  return text;
}

/// A `too-deep` refusal for a bound of [depth].
AosError tooDeep(Limit depth) => _refusal(
  'too-deep',
  'nested deeper than ${limitText(depth)}',
).over('manifest_depth');

sealed class _Frame {}

final class _ListFrame extends _Frame {
  _ListFrame(bool nulls) : list = nulls ? <Object?>[] : <Object>[];
  final List<Object?> list;
}

final class _MapFrame extends _Frame {
  _MapFrame(bool nulls)
    : map = nulls ? <String, Object?>{} : <String, Object>{};
  final Map<String, Object?> map;
  String? key;
}

/// Builds a value from a stream of scalars, keys and containers, refusing nesting past `depth`
/// as it opens.
class Builder {
  /// A builder bounded by [_depth]; [_nulls] keeps a null (an env script's output, where it
  /// unsets) instead of refusing it.
  Builder(this._depth, this._nulls);

  final Limit _depth;
  final bool _nulls;
  final List<_Frame> _stack = [];

  /// The finished value, once the outermost container has closed or a bare scalar was given.
  Object? root;

  /// Whether a root has been set.
  bool hasRoot = false;

  /// The number of open containers.
  int get depth => _stack.length;

  /// Whether the next item is a mapping key.
  bool wantsKey() {
    if (_stack.isEmpty) return false;
    final top = _stack.last;
    return top is _MapFrame && top.key == null;
  }

  /// Whether the innermost open container is a mapping.
  bool inMap() => _stack.isNotEmpty && _stack.last is _MapFrame;

  /// Whether the innermost open container is a list.
  bool inList() => _stack.isNotEmpty && _stack.last is _ListFrame;

  /// Sets the next mapping key, refusing a duplicate.
  void key(String key) {
    if (_stack.isEmpty) return;
    final top = _stack.last;
    if (top is _MapFrame) {
      if (top.map.containsKey(key)) {
        throw _refusal('duplicate-key', 'a key appears twice in one mapping');
      }
      top.key = key;
    }
  }

  /// Adds a finished value (null only where nulls are allowed).
  void value(Object? given) {
    if (given == null && !_nulls) throw _refusal('null-value', 'a null');
    final top = _stack.isEmpty ? null : _stack.last;
    if (top == null) {
      if (hasRoot) throw _refusal('parse-error', 'more than one document');
      root = given;
      hasRoot = true;
    } else if (top is _ListFrame) {
      top.list.add(given);
    } else if (top is _MapFrame) {
      final key = top.key;
      if (key == null) {
        throw _refusal('non-string-key', 'a mapping key that is not a string');
      }
      top.map[key] = given;
      top.key = null;
    }
  }

  void _open(_Frame frame) {
    if (!allows(_depth, _stack.length + 1)) throw tooDeep(_depth);
    _stack.add(frame);
  }

  /// Opens a list.
  void openList() => _open(_ListFrame(_nulls));

  /// Opens a mapping.
  void openMap() => _open(_MapFrame(_nulls));

  /// Closes the innermost container and adds it as a value.
  void close() {
    if (_stack.isEmpty) {
      throw _refusal('parse-error', 'a container closed twice');
    }
    final frame = _stack.removeLast();
    value(frame is _ListFrame ? frame.list : (frame as _MapFrame).map);
  }
}

/// The 1-based line and column of [offset] in [text], put on [error] unless it has a position.
AosError atOffset(AosError error, String text, int offset) {
  final (line, column) = locateText(text, offset);
  return error.at(line, column);
}

/// RFC 8259 JSON: a duplicate key, a lone surrogate, `NaN`, `Infinity` and comments refused, and
/// a number kept as the text it was written as. A null is refused unless [nulls].
Object? readJson(String text, Limit depth, bool nulls) {
  final reader = _JsonReader(text, depth, nulls);
  try {
    return reader.document();
  } on AosError catch (error) {
    if (error.line == null) throw atOffset(error, text, reader.at);
    rethrow;
  }
}

bool _isDigit(int code) => code >= 0x30 && code <= 0x39;

class _JsonReader {
  _JsonReader(this.text, this.depth, this.nulls);

  int at = 0;
  final String text;
  final Limit depth;
  final bool nulls;

  Never _fail(String cls, String detail) => throw _refusal(cls, detail);

  int _code(int index) => index < text.length ? text.codeUnitAt(index) : -1;

  void _space() {
    for (;;) {
      final c = _code(at);
      if (c == 0x20 || c == 0x09 || c == 0x0a || c == 0x0d) {
        at += 1;
      } else {
        return;
      }
    }
  }

  Object? document() {
    final builder = Builder(depth, nulls);
    var expectValue = true;
    for (;;) {
      _space();
      if (at >= text.length) _fail('parse-error', 'the document ends early');
      final c = text.codeUnitAt(at);
      if (expectValue) {
        final isKey = builder.wantsKey();
        if ((c == 0x7b || c == 0x5b) && !isKey) {
          at += 1;
          if (c == 0x7b) {
            builder.openMap();
          } else {
            builder.openList();
          }
          _space();
          if (_code(at) == (c == 0x7b ? 0x7d : 0x5d)) {
            at += 1;
            builder.close();
            expectValue = false;
          }
          // The first item of a container is read next; a close ended it.
        } else {
          if (c == 0x22) {
            final string = _string();
            if (isKey) {
              builder.key(string);
              _space();
              if (_code(at) != 0x3a) {
                _fail('parse-error', 'a key without a colon');
              }
              at += 1;
              continue;
            }
            builder.value(string);
          } else if (isKey) {
            _fail('parse-error', 'a key that is not a string');
          } else if (c == 0x74) {
            _word('true', builder, true);
          } else if (c == 0x66) {
            _word('false', builder, false);
          } else if (c == 0x6e) {
            _word('null', builder, null);
          } else if (c == 0x2d || _isDigit(c)) {
            builder.value(_number());
          } else {
            _fail('parse-error', 'not a JSON value');
          }
          expectValue = false;
        }
        if (expectValue) continue;
      } else {
        if (c == 0x2c && builder.depth > 0) {
          at += 1;
          expectValue = true;
        } else if (builder.depth > 0 &&
            ((c == 0x7d && builder.inMap()) ||
                (c == 0x5d && builder.inList()))) {
          at += 1;
          builder.close();
        } else {
          _fail('parse-error', 'an unexpected character');
        }
      }
      if (builder.depth == 0 && builder.hasRoot) {
        _space();
        if (at != text.length) _fail('parse-error', 'text after the document');
        return builder.root;
      }
    }
  }

  void _word(String word, Builder builder, bool? value) {
    if (!text.startsWith(word, at)) _fail('parse-error', 'not a JSON value');
    at += word.length;
    builder.value(value);
  }

  int _digits() {
    final start = at;
    while (_isDigit(_code(at))) {
      at += 1;
    }
    return at - start;
  }

  Object _number() {
    final start = at;
    if (_code(at) == 0x2d) at += 1;
    final leadingZero = _code(at) == 0x30;
    final whole = _digits();
    if (whole == 0 || (leadingZero && whole > 1)) {
      _fail('parse-error', 'a malformed number');
    }
    var fraction = false;
    if (_code(at) == 0x2e) {
      at += 1;
      fraction = true;
      if (_digits() == 0) _fail('parse-error', 'a malformed number');
    }
    if (_code(at) == 0x65 || _code(at) == 0x45) {
      at += 1;
      fraction = true;
      if (_code(at) == 0x2b || _code(at) == 0x2d) at += 1;
      if (_digits() == 0) _fail('parse-error', 'a malformed number');
    }
    final written = text.substring(start, at);
    try {
      return fraction
          ? finiteFloat(double.parse(written))
          : integer64(BigInt.parse(written));
    } on AosError {
      at = start;
      rethrow;
    }
  }

  int _hex4() {
    if (at + 4 > text.length) _fail('parse-error', r'a malformed \u escape');
    final digits = text.substring(at, at + 4);
    if (!RegExp(r'^[0-9a-fA-F]{4}$').hasMatch(digits)) {
      _fail('parse-error', r'a malformed \u escape');
    }
    at += 4;
    return int.parse(digits, radix: 16);
  }

  String _string() {
    at += 1;
    final out = StringBuffer();
    for (;;) {
      if (at >= text.length) _fail('parse-error', 'a string that does not end');
      final c = text.codeUnitAt(at);
      if (c == 0x22) {
        at += 1;
        return out.toString();
      }
      if (c == 0x5c) {
        at += 1;
        if (at >= text.length) {
          _fail('parse-error', 'a string that does not end');
        }
        final escape = text.codeUnitAt(at);
        at += 1;
        switch (escape) {
          case 0x22:
          case 0x5c:
          case 0x2f:
            out.writeCharCode(escape);
          case 0x62:
            out.write('\b');
          case 0x66:
            out.write('\f');
          case 0x6e:
            out.write('\n');
          case 0x72:
            out.write('\r');
          case 0x74:
            out.write('\t');
          case 0x75:
            final first = _hex4();
            var code = first;
            if (first >= 0xd800 && first < 0xdc00) {
              if (!text.startsWith(r'\u', at)) {
                _fail('invalid-unicode', 'a lone surrogate');
              }
              at += 2;
              final second = _hex4();
              if (!(second >= 0xdc00 && second < 0xe000)) {
                _fail('invalid-unicode', 'a lone surrogate');
              }
              code = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
            } else if (first >= 0xdc00 && first < 0xe000) {
              _fail('invalid-unicode', 'a lone surrogate');
            }
            out.writeCharCode(code);
          default:
            _fail('parse-error', 'an unknown escape');
        }
      } else if (c < 0x20) {
        _fail('parse-error', 'a control character in a string');
      } else {
        final start = at;
        while (at < text.length) {
          final code = text.codeUnitAt(at);
          if (code == 0x22 || code == 0x5c || code < 0x20) break;
          at += 1;
        }
        out.write(text.substring(start, at));
      }
    }
  }
}
