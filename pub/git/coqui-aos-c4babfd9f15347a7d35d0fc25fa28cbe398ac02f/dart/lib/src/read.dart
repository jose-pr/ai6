/// Reading a document: the format line, the three formats, `dotenv` text and an env script's
/// output.
library;

import 'dart:typed_data';

import 'error.dart';
import 'json.dart';
import 'limits.dart';
import 'names.dart';
import 'text.dart';
import 'toml.dart';
import 'value.dart';
import 'yaml.dart';

const List<int> _bom = [0xef, 0xbb, 0xbf];

/// The result of [chooseFormat].
class ChosenFormat {
  /// Creates the result.
  const ChosenFormat(this.format, this.payload, this.lined);

  /// `toml`, `json` or `yaml`.
  final String format;

  /// The bytes after the format line, if there was one.
  final Uint8List payload;

  /// Whether a format line was removed.
  final bool lined;
}

final RegExp _formatSeparators = RegExp(r'[ \t\f\r]');
final RegExp _pathSeparators = RegExp(r'[/\\]');
final RegExp _leadingDots = RegExp(r'^\.+');
final RegExp _safeName = RegExp(r'^[A-Za-z0-9_.-]*$');

/// Splits off a `#!` format line. [byDefault] is the format when there is none.
ChosenFormat chooseFormat(Uint8List data, String byDefault) {
  if (!(data.length >= 2 && data[0] == 0x23 && data[1] == 0x21)) {
    return ChosenFormat(byDefault, data, false);
  }
  final rest = Uint8List.sublistView(data, 2);
  final end = rest.indexOf(0x0a);
  final lineBytes = end == -1 ? rest : Uint8List.sublistView(rest, 0, end);
  final payload = end == -1
      ? Uint8List(0)
      : Uint8List.sublistView(rest, end + 1);
  final line = decodeUtf8(lineBytes, 'the format line is not UTF-8');
  final words = line.split(_formatSeparators).where((part) => part != '');
  final word = words.isEmpty ? '' : words.last;
  final last = word.split(_pathSeparators).last;
  final name = asciiLower(last.replaceFirst(_leadingDots, ''));
  if (name == '') {
    throw AosError('no-format-named', 'the format line names no format');
  }
  final String format;
  if (name == 'toml') {
    format = 'toml';
  } else if (name == 'json') {
    format = 'json';
  } else if (name == 'yaml' || name == 'yml') {
    format = 'yaml';
  } else {
    final safe = _safeName.hasMatch(name);
    throw AosError(
      'unsupported-format',
      safe
          ? '$name is not toml, json or yaml'
          : 'the format line names no supported format',
    );
  }
  return ChosenFormat(format, payload, true);
}

/// Parses a payload in a format. [lined] is whether a format line came first, so positions count
/// it.
Document parsePayload(
  Uint8List payload,
  String format,
  Settings settings,
  bool lined,
) {
  final text = decodeUtf8(payload);
  final shift = lined ? 1 : 0;
  if (text.startsWith('﻿')) {
    throw AosError(
      'parse-error',
      'a byte-order mark where the document starts',
    ).at(1 + shift, 1);
  }
  if (trimAscii(text) == '') return <String, Object>{};
  final depth = settings.manifestDepth;
  Object? value;
  try {
    value = switch (format) {
      'toml' => readToml(text, depth),
      'json' => readJson(text, depth, false),
      _ => readYaml(text, depth),
    };
  } on AosError catch (error) {
    final line = error.line;
    if (line != null) {
      final column = error.column;
      if (column == null) {
        error.onLine(line + shift);
      } else {
        error.at(line + shift, column);
      }
    }
    rethrow;
  }
  if (value is! Document) {
    throw AosError('not-a-mapping', 'the document is not a mapping');
  }
  return value;
}

/// Reads one AOS document: an optional byte-order mark, an optional format line, else TOML, then
/// the data model.
Document readDocument(Uint8List data, Settings settings) {
  if (!allows(settings.manifestBytes, data.length)) {
    throw AosError(
      'too-large',
      'over ${limitText(settings.manifestBytes)} bytes',
    ).over('manifest_bytes');
  }
  final bytes = startsWithBytes(data, _bom)
      ? Uint8List.sublistView(data, 3)
      : data;
  final chosen = chooseFormat(bytes, 'toml');
  return parsePayload(chosen.payload, chosen.format, settings, chosen.lined);
}

/// One assignment of a `dotenv` file: its text is not yet substituted unless [literal].
class DotenvLine {
  /// Creates the line.
  const DotenvLine(this.name, this.text, this.literal);

  /// The variable's name.
  final String name;

  /// The value as written, outer quotes removed.
  final String text;

  /// Whether the value was single-quoted, so nothing is substituted in it.
  final bool literal;
}

final RegExp _nameChars = RegExp(r'^[A-Za-z0-9_]*');
final RegExp _leadingBlank = RegExp(r'^[ \t]+');

({String name, String rest})? _assignment(String text) {
  final name = _nameChars.firstMatch(text)?[0] ?? '';
  if (!isVariableName(name)) return null;
  final after = text.substring(name.length).replaceFirst(_leadingBlank, '');
  if (!after.startsWith('=')) return null;
  return (name: name, rest: after.substring(1));
}

/// Parses `dotenv` text into lines, without substituting.
List<DotenvLine> parseDotenv(Uint8List data) {
  final bytes = startsWithBytes(data, _bom)
      ? Uint8List.sublistView(data, 3)
      : data;
  final text = decodeUtf8(bytes);
  final lines = <DotenvLine>[];
  final seen = <String>{};
  final numbered = text.split('\n');
  for (var number = 0; number < numbered.length; number += 1) {
    var line = numbered[number];
    if (line.endsWith('\r')) line = line.substring(0, line.length - 1);
    final stripped = trimBlank(line);
    if (stripped == '' || stripped.startsWith('#')) continue;
    final start = line.replaceFirst(_leadingBlank, '');
    ({String name, String rest})? found;
    if (start.startsWith('export')) {
      final rest = start.substring('export'.length);
      final after = rest.replaceFirst(_leadingBlank, '');
      if (after.length < rest.length) found = _assignment(after);
    }
    found ??= _assignment(start);
    if (found == null) {
      throw AosError(
        'dotenv-syntax',
        'a line that is no NAME=VALUE',
      ).onLine(number + 1);
    }
    var value = trimBlank(found.rest);
    var literal = false;
    final chars = value.runes.toList();
    if (chars.length >= 2 &&
        chars.first == chars.last &&
        (chars.first == 0x22 || chars.first == 0x27)) {
      literal = chars.first == 0x27;
      value = String.fromCharCodes(chars.sublist(1, chars.length - 1));
    }
    final lower = asciiLower(found.name);
    if (seen.contains(lower)) {
      throw AosError(
        'duplicate-key',
        '${found.name} is set twice',
      ).onLine(number + 1);
    }
    seen.add(lower);
    lines.add(DotenvLine(found.name, value, literal));
  }
  return lines;
}

/// What an env script printed: each variable it set, in order; a null value unsets one.
typedef ScriptOutput = List<(String, String?)>;

/// Reads an env script's output: one JSON object, or one per line.
ScriptOutput readEnvScriptOutput(Uint8List data, Settings settings) {
  final text = decodeUtf8(data);
  final whole = trimAscii(text);
  if (whole == '') return [];
  final depth = settings.manifestDepth;
  Object? json(String piece) => readJson(piece, depth, true);
  List<Object?> objects;
  try {
    objects = [json(whole)];
  } on AosError catch (error) {
    if (error.errorClass != 'parse-error') rethrow;
    objects = [];
    for (var line in text.split('\n')) {
      if (line.endsWith('\r')) line = line.substring(0, line.length - 1);
      if (trimBlank(line) != '') objects.add(json(line));
    }
  }
  final out = <String, String?>{};
  for (final object in objects) {
    if (object is! Map<String, Object?>) {
      throw AosError('env-script-output', 'not an object');
    }
    for (final entry in object.entries) {
      final name = entry.key;
      if (!isVariableName(name) || isReservedVariable(name)) {
        throw AosError('env-script-output', 'a name that may not be set');
      }
      final Object? value = entry.value;
      String? set;
      if (value == null) {
        set = null;
      } else if (value is String && !value.contains('\u0000')) {
        set = value;
      } else {
        throw AosError('env-script-output', 'a value that is not text');
      }
      out.remove(name);
      out[name] = set;
    }
  }
  return [for (final entry in out.entries) (entry.key, entry.value)];
}
