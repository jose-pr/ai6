/// `${NAME}` substitution: the Bourne-shell parameter expansions an overlay's `dotenv` files and
/// environment use.
library;

import 'error.dart';
import 'limits.dart';

/// Looks a variable up: its value, or null when it is not set. It may throw an [AosError] to
/// refuse the read, as `secret-placeholder` does for a variable that holds a secret.
typedef Lookup = String? Function(String name);

typedef _Cps = List<int>;

const int _dollar = 0x24;
const int _open = 0x7b;
const int _close = 0x7d;
const int _backslash = 0x5c;
const int _slash = 0x2f;
const int _hash = 0x23;
const int _percent = 0x25;
const int _colon = 0x3a;
const int _maxSafeInteger = 9007199254740991;

_Cps _toCps(String text) => text.runes.toList();

String _fromCps(_Cps cps, [int start = 0, int? end]) =>
    String.fromCharCodes(cps, start, end ?? cps.length);

/// The code at [at], or -1 past the end.
int _at(_Cps cps, int at) => at >= 0 && at < cps.length ? cps[at] : -1;

int _utf8Length(String text) {
  var size = 0;
  for (var at = 0; at < text.length; at += 1) {
    final code = text.codeUnitAt(at);
    if (code < 0x80) {
      size += 1;
    } else if (code < 0x800) {
      size += 2;
    } else if (code >= 0xd800 && code < 0xdc00) {
      size += 4;
      at += 1;
    } else {
      size += 3;
    }
  }
  return size;
}

bool _isAsciiAlpha(int c) =>
    (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a);
bool _isAsciiDigit(int c) => c >= 0x30 && c <= 0x39;

class _Run {
  _Run(this.lookup, this.limits);

  final Lookup lookup;
  final Settings limits;
  int depth = 0;
  int steps = 0;

  void step(int cost) {
    steps += cost;
    if (!allows(limits.substitutionSteps, steps)) {
      throw AosError('too-complex', 'a pattern').over('substitution_steps');
    }
  }

  /// Refuses an output of [size] UTF-8 bytes past `substitutionBytes`.
  void fits(int size) {
    if (!allows(limits.substitutionBytes, size)) {
      throw AosError(
        'too-large',
        'a substituted value',
      ).over('substitution_bytes');
    }
  }
}

/// Substitutes `${...}` expressions in [text], with the default bounds.
String substitute(String text, Lookup lookup) =>
    substituteWith(text, lookup, defaultSettings);

/// Substitutes `${...}` expressions in [text] within [settings]' `substitution*` bounds. A
/// placeholder's variable or pattern is never quoted in an error.
String substituteWith(String text, Lookup lookup, Settings settings) =>
    _substituteCps(_toCps(text), _Run(lookup, settings));

String _substituteCps(_Cps text, _Run run) {
  if (!allows(run.limits.substitutionDepth, run.depth)) {
    throw AosError('too-deep', 'an expression').over('substitution_depth');
  }
  run.depth += 1;
  try {
    return _substituteInner(text, run);
  } finally {
    run.depth -= 1;
  }
}

String _substituteInner(_Cps text, _Run run) {
  final out = StringBuffer();
  var size = 0;
  void append(String piece) {
    out.write(piece);
    size += _utf8Length(piece);
    run.fits(size);
  }

  var index = 0;
  while (index < text.length) {
    if (text[index] != _dollar) {
      // A run of ordinary text is copied at once.
      var end = index + 1;
      while (end < text.length && text[end] != _dollar) {
        end += 1;
      }
      append(_fromCps(text, index, end));
      index = end;
      continue;
    }
    final next = _at(text, index + 1);
    if (next == _dollar) {
      append(r'$');
      index += 2;
      continue;
    }
    if (next == _open) {
      final close = _closingBrace(text, index + 2);
      if (close != null) {
        final found = _expression(text.sublist(index + 2, close), run);
        if (found != null) {
          append(found);
          index = close + 1;
          continue;
        }
      }
      // A `${` that is no expression is rescanned from the next character, so its scan is
      // charged; one that is an expression is bounded by the depth limit.
      final scanned = (close ?? text.length) - index;
      run.step(scanned < 0 ? 0 : scanned);
    }
    append(r'$');
    index += 1;
  }
  return out.toString();
}

int? _closingBrace(_Cps text, int from) {
  var depth = 0;
  var index = from;
  while (index < text.length) {
    if (text[index] == _dollar && _at(text, index + 1) == _open) {
      depth += 1;
      index += 2;
      continue;
    }
    if (text[index] == _close) {
      if (depth == 0) return index;
      depth -= 1;
    }
    index += 1;
  }
  return null;
}

int _nameAt(_Cps text) {
  if (text.isEmpty || !(_isAsciiAlpha(text[0]) || text[0] == 0x5f)) return 0;
  var length = 1;
  while (length < text.length &&
      (_isAsciiAlpha(text[length]) ||
          _isAsciiDigit(text[length]) ||
          text[length] == 0x5f)) {
    length += 1;
  }
  return length;
}

String _required(String name, _Run run) {
  final value = run.lookup(name);
  if (value == null) throw AosError('undefined-variable', name);
  return value;
}

bool _startsWith(_Cps text, String prefix) {
  final codes = prefix.runes.toList();
  for (var at = 0; at < codes.length; at += 1) {
    if (_at(text, at) != codes[at]) return false;
  }
  return true;
}

String? _expression(_Cps body, _Run run) {
  if (_at(body, 0) == _hash) {
    final length = _nameAt(body.sublist(1));
    if (length > 0 && length + 1 == body.length) {
      return '${_required(_fromCps(body, 1), run).runes.length}';
    }
  }
  final length = _nameAt(body);
  if (length == 0) return null;
  final key = _fromCps(body, 0, length);
  final rest = body.sublist(length);
  if (rest.isEmpty) return _required(key, run);
  for (final operator in const [':-', ':=', ':+', '-', '=', '+']) {
    if (!_startsWith(rest, operator)) continue;
    final word = rest.sublist(operator.length);
    final value = run.lookup(key);
    final present = value != null && !(operator.startsWith(':') && value == '');
    if (operator.endsWith('+')) return present ? _substituteCps(word, run) : '';
    return present ? value : _substituteCps(word, run);
  }
  final lead = rest[0];
  if (lead == _colon) {
    final spec = _sliceSpec(rest.sublist(1));
    if (spec == null) return null;
    return _slice(_required(key, run), spec.$1, spec.$2);
  }
  if (lead == _slash) {
    final spec = rest.sublist(1);
    final first = _at(spec, 0);
    final kind = first == _slash || first == _hash || first == _percent
        ? String.fromCharCode(first)
        : null;
    final (pattern, replacement) = _splitPattern(
      spec.sublist(kind == null ? 0 : 1),
    );
    final value = _required(key, run);
    return _replace(value, kind, pattern, replacement, run);
  }
  if (lead == _hash || lead == _percent) {
    final doubled = _at(rest, 1) == lead;
    final pattern = rest.sublist(doubled ? 2 : 1);
    return _trim(
      _required(key, run),
      lead == _hash ? '#' : '%',
      doubled,
      pattern,
      run,
    );
  }
  final operator = _fromCps(rest);
  if (!const ['^', '^^', ',', ',,'].contains(operator)) return null;
  final value = _required(key, run);
  if (operator == '^^') return _upper(value);
  if (operator == ',,') return _lower(value);
  final chars = value.runes.toList();
  if (chars.isEmpty) return '';
  final first = String.fromCharCode(chars.first);
  final others = String.fromCharCodes(chars.skip(1));
  return (operator == '^' ? _upper(first) : _lower(first)) + others;
}

/// Unicode's unconditional full uppercase mappings (SpecialCasing.txt) that `toUpperCase` leaves
/// out.
const Map<int, List<int>> _specialUpper = {
  0xdf: [0x53, 0x53],
  0x149: [0x2bc, 0x4e],
  0x1f0: [0x4a, 0x30c],
  0x390: [0x399, 0x308, 0x301],
  0x3b0: [0x3a5, 0x308, 0x301],
  0x587: [0x535, 0x552],
  0x1e96: [0x48, 0x331],
  0x1e97: [0x54, 0x308],
  0x1e98: [0x57, 0x30a],
  0x1e99: [0x59, 0x30a],
  0x1e9a: [0x41, 0x2be],
  0x1f50: [0x3a5, 0x313],
  0x1f52: [0x3a5, 0x313, 0x300],
  0x1f54: [0x3a5, 0x313, 0x301],
  0x1f56: [0x3a5, 0x313, 0x342],
  0x1fb2: [0x1fba, 0x399],
  0x1fb3: [0x391, 0x399],
  0x1fb4: [0x386, 0x399],
  0x1fb6: [0x391, 0x342],
  0x1fb7: [0x391, 0x342, 0x399],
  0x1fbc: [0x391, 0x399],
  0x1fc2: [0x1fca, 0x399],
  0x1fc3: [0x397, 0x399],
  0x1fc4: [0x389, 0x399],
  0x1fc6: [0x397, 0x342],
  0x1fc7: [0x397, 0x342, 0x399],
  0x1fcc: [0x397, 0x399],
  0x1fd2: [0x399, 0x308, 0x300],
  0x1fd3: [0x399, 0x308, 0x301],
  0x1fd6: [0x399, 0x342],
  0x1fd7: [0x399, 0x308, 0x342],
  0x1fe2: [0x3a5, 0x308, 0x300],
  0x1fe3: [0x3a5, 0x308, 0x301],
  0x1fe4: [0x3a1, 0x313],
  0x1fe6: [0x3a5, 0x342],
  0x1fe7: [0x3a5, 0x308, 0x342],
  0x1ff2: [0x1ffa, 0x399],
  0x1ff3: [0x3a9, 0x399],
  0x1ff4: [0x38f, 0x399],
  0x1ff6: [0x3a9, 0x342],
  0x1ff7: [0x3a9, 0x342, 0x399],
  0x1ffc: [0x3a9, 0x399],
  0xfb00: [0x46, 0x46],
  0xfb01: [0x46, 0x49],
  0xfb02: [0x46, 0x4c],
  0xfb03: [0x46, 0x46, 0x49],
  0xfb04: [0x46, 0x46, 0x4c],
  0xfb05: [0x53, 0x54],
  0xfb06: [0x53, 0x54],
  0xfb13: [0x544, 0x546],
  0xfb14: [0x544, 0x535],
  0xfb15: [0x544, 0x53b],
  0xfb16: [0x54e, 0x546],
  0xfb17: [0x544, 0x53d],
};

/// Unicode's default full uppercase, as JavaScript's `toUpperCase` gives it.
String _upper(String text) {
  final out = StringBuffer();
  for (final rune in text.runes) {
    final special = _specialUpper[rune];
    if (special != null) {
      out.write(String.fromCharCodes(special));
    } else if (rune >= 0x1f80 && rune <= 0x1faf) {
      // Greek with ypogegrammeni: the capital without it, then a capital iota.
      out.write(
        String.fromCharCodes([
          const [0x1f08, 0x1f28, 0x1f68][(rune - 0x1f80) ~/ 16] + (rune & 0x7),
          0x399,
        ]),
      );
    } else {
      out.write(String.fromCharCode(rune).toUpperCase());
    }
  }
  return out.toString();
}

final RegExp _letter = RegExp(r'\p{L}', unicode: true);

/// Unicode's default full lowercase, as JavaScript's `toLowerCase` gives it: `İ` keeps its dot,
/// and a capital sigma that ends a word is final.
String _lower(String text) {
  final runes = text.runes.toList();
  final out = StringBuffer();
  for (var i = 0; i < runes.length; i += 1) {
    final rune = runes[i];
    if (rune == 0x130) {
      out.write(String.fromCharCodes(const [0x69, 0x307]));
    } else if (rune == 0x3a3 &&
        i > 0 &&
        _letter.hasMatch(String.fromCharCode(runes[i - 1])) &&
        (i + 1 == runes.length ||
            !_letter.hasMatch(String.fromCharCode(runes[i + 1])))) {
      out.writeCharCode(0x3c2);
    } else {
      out.write(String.fromCharCode(rune).toLowerCase());
    }
  }
  return out.toString();
}

bool _isSpace(int c) =>
    c == 0x20 || c == 0x09 || c == 0x0a || c == 0x0d || c == 0x0b || c == 0x0c;

int _number(_Cps digits, bool negative) {
  var value = 0;
  for (final digit in digits) {
    final next = value * 10 + (digit - 0x30);
    value = next > _maxSafeInteger ? _maxSafeInteger : next;
  }
  return negative ? -value : value;
}

(int, int?)? _sliceSpec(_Cps text) {
  var at = 0;
  int skip() {
    final start = at;
    while (at < text.length && _isSpace(text[at])) {
      at += 1;
    }
    return at - start;
  }

  _Cps digits() {
    final start = at;
    while (at < text.length && _isAsciiDigit(text[at])) {
      at += 1;
    }
    return text.sublist(start, at);
  }

  final spaces = skip();
  int offset;
  final lead = _at(text, at);
  if (lead == 0x28) {
    at += 1;
    skip();
    final negative = _at(text, at) == 0x2d;
    if (negative) at += 1;
    final found = digits();
    if (found.isEmpty) return null;
    offset = _number(found, negative);
    skip();
    if (_at(text, at) != 0x29) return null;
    at += 1;
  } else if (lead != -1 && _isAsciiDigit(lead)) {
    offset = _number(digits(), false);
  } else if (lead == 0x2d && spaces > 0) {
    at += 1;
    final found = digits();
    if (found.isEmpty) return null;
    offset = _number(found, true);
  } else {
    return null;
  }
  int? length;
  if (_at(text, at) == _colon) {
    at += 1;
    skip();
    final negative = _at(text, at) == 0x2d;
    if (negative) at += 1;
    final found = digits();
    if (found.isEmpty) return null;
    length = _number(found, negative);
  }
  skip();
  return at == text.length ? (offset, length) : null;
}

String _slice(String value, int givenOffset, int? length) {
  final chars = _toCps(value);
  final size = chars.length;
  var offset = givenOffset;
  if (offset < 0) {
    offset += size;
    if (offset < 0) return '';
  }
  final end = length == null
      ? size
      : length >= 0
      ? offset + length
      : size + length;
  if (end <= offset) return '';
  final from = offset < size ? offset : size;
  final to = end < size ? end : size;
  return _fromCps(chars, from, to > from ? to : from);
}

(_Cps, _Cps) _splitPattern(_Cps spec) {
  var index = 0;
  while (index < spec.length) {
    if (spec[index] == _backslash) {
      index += 2;
      continue;
    }
    if (spec[index] == _slash) {
      return (spec.sublist(0, index), spec.sublist(index + 1));
    }
    index += 1;
  }
  return (spec, <int>[]);
}

enum _Kind { star, any, char, cls }

class _Token {
  const _Token(
    this.kind, {
    this.code = 0,
    this.negate = false,
    this.members = const [],
  });

  final _Kind kind;
  final int code;
  final bool negate;
  final List<(int, int)> members;

  bool matches(int c) {
    switch (kind) {
      case _Kind.star:
      case _Kind.any:
        return true;
      case _Kind.char:
        return code == c;
      case _Kind.cls:
        return members.any((m) => c >= m.$1 && c <= m.$2) != negate;
    }
  }
}

List<_Token> _compile(_Cps pattern) {
  AosError invalid() => AosError('invalid-pattern', 'a bracket expression');
  final out = <_Token>[];
  // nextClose[i] is the first `]` at or after i, or -1.
  final nextClose = List<int>.filled(pattern.length + 1, -1);
  for (var at = pattern.length - 1; at >= 0; at -= 1) {
    nextClose[at] = pattern[at] == 0x5d ? at : nextClose[at + 1];
  }
  var index = 0;
  while (index < pattern.length) {
    final c = pattern[index];
    if (c == _backslash && index + 1 < pattern.length) {
      out.add(_Token(_Kind.char, code: pattern[index + 1]));
      index += 2;
      continue;
    }
    final found =
        nextClose[index + 2 < pattern.length ? index + 2 : pattern.length];
    final close = found == -1 ? null : found;
    if (c == 0x2a) {
      out.add(const _Token(_Kind.star));
    } else if (c == 0x3f) {
      out.add(const _Token(_Kind.any));
    } else if (c == 0x5b && close != null) {
      final body = pattern.sublist(index + 1, close);
      final negate = _at(body, 0) == 0x21 || _at(body, 0) == 0x5e;
      final items = body.sublist(negate ? 1 : 0);
      if (items.isEmpty) throw invalid();
      final members = <(int, int)>[];
      var at = 0;
      while (at < items.length) {
        if (at + 2 < items.length && items[at + 1] == 0x2d) {
          final low = items[at];
          final high = items[at + 2];
          if (high < low) throw invalid();
          members.add((low, high));
          at += 3;
        } else {
          members.add((items[at], items[at]));
          at += 1;
        }
      }
      out.add(_Token(_Kind.cls, negate: negate, members: members));
      index = close + 1;
      continue;
    } else {
      out.add(_Token(_Kind.char, code: c));
    }
    index += 1;
  }
  return out;
}

/// Whether [tokens] match all of `text[from, to)`; the work is charged to the run.
bool _whole(List<_Token> tokens, _Cps text, int from, int to, _Run run) {
  var t = 0;
  var s = from;
  (int, int)? star;
  var steps = 0;
  while (s < to) {
    steps += 1;
    final token = t < tokens.length ? tokens[t] : null;
    if (token != null && token.kind == _Kind.cls) steps += token.members.length;
    if (token != null && token.kind != _Kind.star && token.matches(text[s])) {
      t += 1;
      s += 1;
    } else if (token != null && token.kind == _Kind.star) {
      star = (t, s);
      t += 1;
    } else if (star != null) {
      t = star.$1 + 1;
      s = star.$2 + 1;
      star = (star.$1, star.$2 + 1);
    } else {
      run.step(steps);
      return false;
    }
  }
  run.step(steps + 1);
  for (var i = t; i < tokens.length; i += 1) {
    if (tokens[i].kind != _Kind.star) return false;
  }
  return true;
}

List<int> _ends(List<_Token> tokens, _Cps text, int start, _Run run) {
  final found = <int>[];
  for (var end = start; end <= text.length; end += 1) {
    if (_whole(tokens, text, start, end, run)) found.add(end);
  }
  return found;
}

int? _suffixStart(List<_Token> tokens, _Cps text, bool last, _Run run) {
  int? found;
  for (var start = 0; start <= text.length; start += 1) {
    if (_whole(tokens, text, start, text.length, run)) {
      found = start;
      if (!last) break;
    }
  }
  return found;
}

String _replace(
  String value,
  String? kind,
  _Cps pattern,
  _Cps replacementCps,
  _Run run,
) {
  if (pattern.isEmpty) return value;
  final tokens = _compile(pattern);
  final text = _toCps(value);
  final replacement = _fromCps(replacementCps);
  String out;
  if (kind == '#') {
    final ends = _ends(tokens, text, 0, run);
    final end = ends.isEmpty ? null : ends.last;
    out = end == null ? value : replacement + _fromCps(text, end);
  } else if (kind == '%') {
    final start = _suffixStart(tokens, text, false, run);
    out = start == null ? value : _fromCps(text, 0, start) + replacement;
  } else {
    final buffer = StringBuffer();
    var size = 0;
    var position = 0;
    for (;;) {
      (int, int)? found;
      for (var start = position; start < text.length; start += 1) {
        final ends = _ends(tokens, text, start, run);
        final end = ends.isEmpty ? null : ends.last;
        if (end != null && end > start) {
          found = (start, end);
          break;
        }
      }
      if (found == null) break;
      final piece = _fromCps(text, position, found.$1) + replacement;
      buffer.write(piece);
      size += _utf8Length(piece);
      run.fits(size);
      position = found.$2;
      if (kind != '/') break;
    }
    buffer.write(_fromCps(text, position));
    out = buffer.toString();
  }
  run.fits(_utf8Length(out));
  return out;
}

String _trim(
  String value,
  String operator,
  bool longest,
  _Cps pattern,
  _Run run,
) {
  final tokens = _compile(pattern);
  final text = _toCps(value);
  if (operator == '#') {
    final found = _ends(tokens, text, 0, run);
    final int? end = found.isEmpty
        ? null
        : (longest ? found.last : found.first);
    return end == null ? value : _fromCps(text, end);
  }
  // The shortest suffix starts last, the longest first.
  final start = _suffixStart(tokens, text, !longest, run);
  return start == null ? value : _fromCps(text, 0, start);
}
