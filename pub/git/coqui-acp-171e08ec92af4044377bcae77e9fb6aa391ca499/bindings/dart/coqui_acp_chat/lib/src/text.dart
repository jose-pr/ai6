import 'limits.dart';

final _unprintable = RegExp(
  r'[\u{0}-\u{1f}\u{7f}-\u{9f}\u{200e}\u{200f}\u{2028}-\u{202e}\u{2066}-\u{2069}]',
  unicode: true,
);

bool _isHigh(int unit) => unit >= 0xD800 && unit <= 0xDBFF;
bool _isLow(int unit) => unit >= 0xDC00 && unit <= 0xDFFF;

/// The first [n] code units of [text], ending before a character that [n] would cut in two.
String _head(String text, int n) {
  if (n <= 0) return '';
  if (n >= text.length) return text;
  return text.substring(0, _isHigh(text.codeUnitAt(n - 1)) ? n - 1 : n);
}

/// The last [n] code units of [text], starting after a character that the cut would split.
String _tail(String text, int n) {
  if (n <= 0) return '';
  if (n >= text.length) return text;
  final start = text.length - n;
  return text.substring(_isLow(text.codeUnitAt(start)) ? start + 1 : start);
}

/// [value] as an identifier the agent chose, to be sent back to it as it came: a string that is not empty, no longer than
/// [ChatLimits.shortText], and with no control or direction-changing character in it. Null for anything else, which is then not used.
String? idText(Object? value, ChatLimits limits) {
  if (value is! String ||
      value.isEmpty ||
      value.length > limits.shortText ||
      _unprintable.hasMatch(value)) {
    return null;
  }
  return value;
}

/// [value] as one short line of text a person may be shown: control and direction-changing characters become spaces, since they can
/// reorder or hide what surrounds them, and it is cut at [ChatLimits.shortText] without splitting a character. Null when it is not a
/// string or is blank.
String? shortText(Object? value, ChatLimits limits) {
  if (value is! String) return null;
  final clean = value.replaceAll(_unprintable, ' ').trim();
  if (clean.isEmpty) return null;
  return clean.length > limits.shortText
      ? '${_head(clean, limits.shortText)}…'
      : clean;
}

/// The last [limit] characters of [text].
String boundedTail(String text, int limit) => _tail(text, limit);

/// The first [limit] characters of [text], with a `…` line after them when it was cut.
String boundedHead(String text, int limit) =>
    text.length > limit ? '${_head(text, limit)}\n…' : text;

/// The first [n] characters of [text], cut where a character is not split; for a title.
String headOf(String text, int n) => _head(text, n);

/// Text that grows by appending and is bounded, without copying all of it for each piece. Kept from its start ([BoundedText.head]:
/// once cut, later pieces are dropped, and a `…` line ends it) or from its end ([BoundedText.tail]: the oldest goes).
final class BoundedText {
  BoundedText.head(this.limit) : _fromEnd = false;
  BoundedText.tail(this.limit) : _fromEnd = true;

  final int limit;
  final bool _fromEnd;
  final StringBuffer _buffer = StringBuffer();
  int _length = 0;
  bool _cut = false;
  String? _value;

  /// What is kept.
  String get value {
    final held = _value;
    if (held != null) return held;
    final all = _buffer.toString();
    final shown = _fromEnd
        ? _tail(all, limit)
        : _cut
        ? '${_head(all, limit)}\n…'
        : all;
    // A tail kept is the whole buffer from here on, so the next trim is cheap.
    if (_fromEnd && all.length != shown.length) {
      _buffer
        ..clear()
        ..write(shown);
      _length = shown.length;
    }
    return _value = shown;
  }

  bool get isEmpty => _length == 0;

  /// Adds [piece] after what is held.
  void append(String piece) {
    if (piece.isEmpty || (!_fromEnd && _cut)) return;
    _value = null;
    _buffer.write(piece);
    _length += piece.length;
    if (!_fromEnd && _length > limit) {
      _cut = true;
      final head = _head(_buffer.toString(), limit);
      _buffer
        ..clear()
        ..write(head);
      _length = head.length;
    } else if (_fromEnd && _length > 2 * limit + 16) {
      // Trim now and then, not for each piece.
      value;
    }
  }

  /// Replaces what is held with [text].
  void set(String text) {
    _buffer.clear();
    _length = 0;
    _cut = false;
    _value = null;
    append(text);
  }
}
