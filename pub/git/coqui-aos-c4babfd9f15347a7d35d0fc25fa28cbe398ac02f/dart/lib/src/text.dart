/// Text and byte helpers the rest of the library shares.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'error.dart';

/// Bytes as text, or `invalid-utf8`. A byte-order mark is kept (`utf8.decode` would drop it).
String decodeUtf8(Uint8List bytes, [String subject = 'not UTF-8']) {
  try {
    final text = const Utf8Decoder(allowMalformed: false).convert(bytes);
    if (bytes.length >= 3 &&
        bytes[0] == 0xef &&
        bytes[1] == 0xbb &&
        bytes[2] == 0xbf) {
      return '﻿$text';
    }
    return text;
  } on FormatException {
    throw AosError('invalid-utf8', subject);
  }
}

/// Text as UTF-8 bytes. Text holding a lone surrogate is encoded with U+FFFD in its place.
Uint8List encodeUtf8(String text) => Uint8List.fromList(utf8.encode(text));

/// The one-based line and column of a UTF-16 [offset] in [text]; a column counts Unicode scalar
/// values.
(int, int) locateText(String text, int offset) {
  final end = offset < 0 ? 0 : (offset > text.length ? text.length : offset);
  var line = 1;
  var start = 0;
  for (
    var at = text.indexOf('\n');
    at != -1 && at < end;
    at = text.indexOf('\n', at + 1)
  ) {
    line += 1;
    start = at + 1;
  }
  var column = 1;
  for (var at = start; at < end; at += 1) {
    final unit = text.codeUnitAt(at);
    final pairStart = unit >= 0xd800 && unit < 0xdc00 && at + 1 < end;
    if (pairStart) at += 1;
    column += 1;
  }
  return (line, column);
}

/// Whether [bytes] start with [prefix] at [at].
bool startsWithBytes(Uint8List bytes, List<int> prefix, [int at = 0]) {
  if (bytes.length - at < prefix.length) return false;
  for (var index = 0; index < prefix.length; index += 1) {
    if (bytes[at + index] != prefix[index]) return false;
  }
  return true;
}

/// Concatenates byte arrays.
Uint8List concatBytes(Iterable<Uint8List> parts) {
  final out = BytesBuilder(copy: false);
  parts.forEach(out.add);
  return out.takeBytes();
}

bool _isWhiteSpace(int code) =>
    (code >= 0x09 && code <= 0x0d) ||
    code == 0x20 ||
    code == 0x85 ||
    code == 0xa0 ||
    code == 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code == 0x2028 ||
    code == 0x2029 ||
    code == 0x202f ||
    code == 0x205f ||
    code == 0x3000;

/// Trims the characters [strip] accepts from both ends, in one pass over each end.
String trimBy(String text, bool Function(int code) strip) {
  var start = 0;
  var end = text.length;
  while (start < end && strip(text.codeUnitAt(start))) {
    start += 1;
  }
  while (end > start && strip(text.codeUnitAt(end - 1))) {
    end -= 1;
  }
  return text.substring(start, end);
}

/// Trims Unicode white space from both ends, as Rust's `str::trim` does (a byte-order mark is not
/// white space).
String trimWhitespace(String text) => trimBy(text, _isWhiteSpace);

/// Trims spaces and tabs from both ends.
String trimBlank(String text) =>
    trimBy(text, (code) => code == 0x20 || code == 0x09);

/// Trims spaces, tabs, carriage returns and line feeds from both ends.
String trimAscii(String text) => trimBy(
  text,
  (code) => code == 0x20 || code == 0x09 || code == 0x0d || code == 0x0a,
);

/// Orders text by Unicode scalar value, which is UTF-8 byte order; the order a sorted map of
/// names keeps. (`String.compareTo` compares UTF-16 units, which differ above U+FFFF.)
int compareText(String a, String b) {
  final left = a.runes.iterator;
  final right = b.runes.iterator;
  for (;;) {
    final x = left.moveNext();
    final y = right.moveNext();
    if (!x) return y ? -1 : 0;
    if (!y) return 1;
    final difference = left.current - right.current;
    if (difference != 0) return difference;
  }
}

/// Whether [text] holds a code unit that is not part of a well-formed surrogate pair.
bool hasLoneSurrogate(String text) {
  for (var at = 0; at < text.length; at += 1) {
    final unit = text.codeUnitAt(at);
    if (unit >= 0xd800 && unit < 0xdc00) {
      if (at + 1 < text.length) {
        final next = text.codeUnitAt(at + 1);
        if (next >= 0xdc00 && next < 0xe000) {
          at += 1;
          continue;
        }
      }
      return true;
    }
    if (unit >= 0xdc00 && unit < 0xe000) return true;
  }
  return false;
}

const int _subjectCap = 256;
final RegExp _unsafeInSubject = RegExp(
  r'^[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Cs}\p{Co}\p{Cn}]$',
  unicode: true,
);

/// Text for an error's subject: each control, format, separator, surrogate, private-use or
/// unassigned character written as `\xNN`, `\uNNNN` or `\UNNNNNNNN` in lowercase hexadecimal,
/// and the whole cut to 256 characters, counted after the escapes, with `...` after them when
/// cut.
String safeSubject(String text) {
  final out = <String>[];
  var seen = 0;
  for (final code in _codePoints(text)) {
    if (seen++ > _subjectCap) break;
    final c = String.fromCharCode(code);
    if (!_unsafeInSubject.hasMatch(c)) {
      out.add(c);
    } else if (code < 0x100) {
      out.add('\\x${code.toRadixString(16).padLeft(2, '0')}');
    } else if (code < 0x10000) {
      out.add('\\u${code.toRadixString(16).padLeft(4, '0')}');
    } else {
      out.add('\\U${code.toRadixString(16).padLeft(8, '0')}');
    }
  }
  final chars = out.join().runes.toList();
  if (chars.length <= _subjectCap) return out.join();
  return '${String.fromCharCodes(chars.sublist(0, _subjectCap))}...';
}

// A lone surrogate is its own code point, as a JavaScript iterator yields it.
Iterable<int> _codePoints(String text) sync* {
  for (var at = 0; at < text.length; at += 1) {
    final unit = text.codeUnitAt(at);
    if (unit >= 0xd800 && unit < 0xdc00 && at + 1 < text.length) {
      final next = text.codeUnitAt(at + 1);
      if (next >= 0xdc00 && next < 0xe000) {
        at += 1;
        yield 0x10000 + ((unit - 0xd800) << 10) + (next - 0xdc00);
        continue;
      }
    }
    yield unit;
  }
}

/// The number of bytes [text] is as UTF-8.
int utf8Length(String text) => utf8.encode(text).length;
