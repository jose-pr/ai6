/// `OVERLAY.md`: the front matter's split, its reading, and a context file's text.
library;

import 'dart:typed_data';

import 'error.dart';
import 'limits.dart';
import 'read.dart';
import 'text.dart';
import 'value.dart';

const List<int> _byteOrderMark = [0xef, 0xbb, 0xbf];
const int _space = 0x20;
const int _tab = 0x09;
const int _cr = 0x0d;
const int _lf = 0x0a;

/// The division of a file, by bytes and unchanged: offsets count bytes from the first byte,
/// lines from 1.
class Split {
  /// Creates the division.
  const Split({
    required this.splitCase,
    this.metadata,
    this.metadataOffset,
    this.metadataLine,
    this.bodyOffset,
    this.bodyLine,
  });

  /// How the file divides: `none`, `unterminated`, `too-large`, `empty` or `present`.
  final String splitCase;

  /// The bytes between the fences.
  final Uint8List? metadata;

  /// The offset of the first metadata byte.
  final int? metadataOffset;

  /// The line of the first metadata byte.
  final int? metadataLine;

  /// The offset of the body.
  final int? bodyOffset;

  /// The line of the body.
  final int? bodyLine;

  /// The body of [data], the same bytes this was split from; empty when there is none.
  Uint8List body(Uint8List data) {
    final offset = bodyOffset;
    if (offset == null) return Uint8List(0);
    return Uint8List.sublistView(
      data,
      offset < data.length ? offset : data.length,
    );
  }
}

/// The fence a line is: `---` or `...`, trailing blanks allowed, and a `\r` when the line ended.
String? _fence(Uint8List line, bool ended) {
  var end = line.length;
  if (ended && end > 0 && line[end - 1] == _cr) end -= 1;
  while (end > 0 && (line[end - 1] == _space || line[end - 1] == _tab)) {
    end -= 1;
  }
  if (end != 3) return null;
  final text = String.fromCharCodes([line[0], line[1], line[2]]);
  return text == '---' || text == '...' ? text : null;
}

int? _find(Uint8List data, int from, num to) {
  final end = to < data.length ? to.toInt() : data.length;
  for (var at = from; at < end; at += 1) {
    if (data[at] == _lf) return at;
  }
  return null;
}

/// Whether [head] could be the beginning of an opening fence.
bool _mayOpen(Uint8List head) {
  var tail = Uint8List.sublistView(head, head.length < 3 ? head.length : 3);
  if (tail.isNotEmpty && tail.last == _cr) {
    tail = Uint8List.sublistView(tail, 0, tail.length - 1);
  }
  final prefix = Uint8List.sublistView(
    head,
    0,
    head.length < 3 ? head.length : 3,
  );
  return prefix.every((byte) => byte == 0x2d) &&
      tail.every((byte) => byte == _space || byte == _tab);
}

Uint8List _trimBlank(Uint8List bytes) {
  bool white(int byte) =>
      byte == _space || byte == _tab || byte == _cr || byte == _lf;
  var start = 0;
  while (start < bytes.length && white(bytes[start])) {
    start += 1;
  }
  var end = bytes.length;
  while (end > start && white(bytes[end - 1])) {
    end -= 1;
  }
  return Uint8List.sublistView(bytes, start, end);
}

Split _blank(String kind, int start) =>
    Split(splitCase: kind, bodyOffset: start, bodyLine: 1);

/// Splits a file into front matter and body: after one optional byte-order mark, a first line
/// that is `---`, then everything to the next `---` or `...` line. The head, through the closing
/// fence's line end, is bounded by `manifestBytes`, and nothing past it is read.
Split splitFrontMatter(Uint8List data, Settings settings) {
  final start = startsWithBytes(data, _byteOrderMark) ? 3 : 0;
  final bound = settings.manifestBytes;
  final num limit = bound == null ? double.infinity : bound + start;
  const tooLarge = Split(splitCase: 'too-large');
  final end = _find(data, start, limit);
  if (end == null && data.length > limit) {
    return _mayOpen(Uint8List.sublistView(data, start, limit.toInt()))
        ? tooLarge
        : _blank('none', start);
  }
  final first = end == null
      ? Uint8List.sublistView(data, start)
      : Uint8List.sublistView(data, start, end);
  if (_fence(first, end != null) != '---') return _blank('none', start);
  if (end == null) return _blank('unterminated', start);
  final metadataOffset = end + 1;
  var at = metadataOffset;
  for (;;) {
    final int bodyOffset;
    final lf = _find(data, at, limit);
    if (lf == null) {
      if (data.length > limit) return tooLarge;
      if (_fence(Uint8List.sublistView(data, at), false) == null) {
        return _blank('unterminated', start);
      }
      bodyOffset = data.length;
    } else if (_fence(Uint8List.sublistView(data, at, lf), true) == null) {
      at = lf + 1;
      continue;
    } else {
      bodyOffset = lf + 1;
    }
    final metadata = Uint8List.sublistView(data, metadataOffset, at);
    var lines = 0;
    for (var index = start; index < bodyOffset; index += 1) {
      if (data[index] == _lf) lines += 1;
    }
    return Split(
      splitCase: _trimBlank(metadata).isEmpty ? 'empty' : 'present',
      metadata: metadata,
      metadataOffset: metadataOffset,
      metadataLine: 2,
      bodyOffset: bodyOffset,
      bodyLine: 1 + lines,
    );
  }
}

/// A file's front matter read. [error] is what a gate refuses with; [frontMatterCase] is what a
/// viewer shows.
class FrontMatter {
  /// Creates the reading.
  const FrontMatter({
    required this.frontMatterCase,
    this.data,
    this.error,
    this.format,
    this.bodyOffset,
    this.bodyLine,
  });

  /// What a viewer shows: `none`, `malformed`, `unterminated`, `too-large`, `empty`,
  /// `unknown-format`, `does-not-parse`, `not-a-mapping` or `mapping`.
  final String frontMatterCase;

  /// The metadata, when it read as a mapping.
  final Document? data;

  /// The refusal a gate reports.
  final AosError? error;

  /// `toml`, `json` or `yaml`, when the metadata was parsed.
  final String? format;

  /// The offset of the body.
  final int? bodyOffset;

  /// The line of the body.
  final int? bodyLine;
}

/// Whether a file that has no front matter was meant to: a fence a character off, or one under a
/// format line.
bool _almostAFence(Uint8List data) {
  var lead = 0;
  for (;;) {
    if (startsWithBytes(data, _byteOrderMark, lead)) {
      lead += 3;
    } else if (lead < data.length &&
        const [_space, _tab, _cr, _lf].contains(data[lead])) {
      lead += 1;
    } else {
      break;
    }
  }
  final rest = Uint8List.sublistView(data, lead);
  if (startsWithBytes(rest, const [0x2d, 0x2d, 0x2d]) ||
      startsWithBytes(rest, const [0x2e, 0x2e, 0x2e])) {
    return true;
  }
  final start = startsWithBytes(data, _byteOrderMark) ? 3 : 0;
  if (startsWithBytes(data, const [0x23, 0x21], start)) {
    final end = data.indexOf(_lf, start);
    if (end != -1) {
      final after = end + 1;
      final second = data.indexOf(_lf, after);
      final line = second == -1
          ? Uint8List.sublistView(data, after)
          : Uint8List.sublistView(data, after, second);
      return _fence(line, second != -1) != null;
    }
  }
  return false;
}

/// Splits, then reads the metadata: YAML unless a format line says otherwise.
FrontMatter readFrontMatter(Uint8List data, Settings settings) {
  final split = splitFrontMatter(data, settings);
  FrontMatter found(
    String kind,
    Document? document,
    AosError? error, [
    String? format,
  ]) => FrontMatter(
    frontMatterCase: kind,
    data: document,
    error: error,
    format: format,
    bodyOffset: split.bodyOffset,
    bodyLine: split.bodyLine,
  );
  switch (split.splitCase) {
    case 'none':
      return _almostAFence(data)
          ? found('malformed', null, AosError('malformed-fence', '').onLine(1))
          : found('none', <String, Object>{}, null);
    case 'empty':
      return found('empty', <String, Object>{}, null);
    case 'unterminated':
      return found(
        'unterminated',
        null,
        AosError('unterminated-front-matter', '').onLine(1),
      );
    case 'too-large':
      return FrontMatter(
        frontMatterCase: 'too-large',
        error: AosError(
          'too-large',
          'front matter',
        ).over('manifest_bytes').onLine(1),
      );
    default:
      break;
  }
  final metadata = split.metadata ?? Uint8List(0);
  final firstLine = split.metadataLine ?? 2;
  final ChosenFormat chosen;
  try {
    chosen = chooseFormat(metadata, 'yaml');
  } on AosError catch (error) {
    final kind =
        error.errorClass == 'unsupported-format' ||
            error.errorClass == 'no-format-named'
        ? 'unknown-format'
        : 'does-not-parse';
    return found(kind, null, error.onLine(firstLine));
  }
  try {
    final document = parsePayload(
      chosen.payload,
      chosen.format,
      settings,
      chosen.lined,
    );
    return found(
      _trimBlank(chosen.payload).isEmpty ? 'empty' : 'mapping',
      document,
      null,
      chosen.format,
    );
  } on AosError catch (error) {
    final kind = error.errorClass == 'not-a-mapping'
        ? 'not-a-mapping'
        : 'does-not-parse';
    final errorLine = error.line;
    final line = errorLine == null ? firstLine : firstLine + errorLine - 1;
    return found(kind, null, error.onLine(line), chosen.format);
  }
}

/// A context file's text: the body when the file has front matter, the whole file when it has
/// none. `too-large` when the front matter is.
Uint8List contextFileText(Uint8List data, Settings settings) {
  final split = splitFrontMatter(data, settings);
  if (split.splitCase == 'too-large') {
    throw AosError(
      'too-large',
      'front matter',
    ).over('manifest_bytes').onLine(1);
  }
  return split.body(data);
}
