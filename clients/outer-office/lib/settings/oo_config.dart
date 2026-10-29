import 'dart:convert';

import 'package:toml/toml.dart';
import 'package:yaml/yaml.dart';
import 'package:yaml_edit/yaml_edit.dart';

import '../theme/oo_tokens.dart';

/// The formats an `OO.cfg` can be written in.
enum OoConfigFormat { toml, json, yaml }

/// Outer Office's own configuration, `OO.cfg`: where Universal Exports is and the deployment's
/// default theme. The person's own choices (`oo.theme`) win over it.
///
/// A document names its format on its first line, `#!json`, `#!yaml` (`#!yml`) or `#!toml`
/// (the last word of the line, after any `/`, read without case or leading dots); a document
/// starting `%YAML` is YAML; anything else is TOML. Nothing is guessed from the content.
/// Unknown tables and keys are refused; `[assets]` belongs to the web page's bootstrap and is
/// left alone.
class OoConfig {
  const OoConfig({this.unexUrl, this.theme});

  /// `[unex] url`: Universal Exports' origin.
  final Uri? unexUrl;

  /// `[appearance] theme`: a theme id or `system`, the default under the person's choice.
  final String? theme;

  /// The defaults: no origin and no theme.
  static const none = OoConfig();

  /// Reads an `OO.cfg`'s text.
  ///
  /// Throws a [FormatException] naming the format, the table or the key that is wrong.
  factory OoConfig.parse(String text) {
    final document = decodeOoConfig(text);
    const known = {'unex', 'appearance', 'assets'};
    for (final key in document.keys) {
      if (!known.contains(key)) {
        throw FormatException('OO.cfg: unknown table [$key]');
      }
    }
    Map<String, Object?> table(String name, Set<String> keys) {
      final value = document[name];
      if (value == null) return const {};
      if (value is! Map) {
        throw FormatException('OO.cfg: [$name] is not a table');
      }
      final entries = value.cast<String, Object?>();
      for (final key in entries.keys) {
        if (!keys.contains(key)) {
          throw FormatException('OO.cfg: unknown key $name.$key');
        }
      }
      return entries;
    }

    String? string(Map<String, Object?> table, String name, String key) {
      final value = table[key];
      if (value == null) return null;
      if (value is! String) {
        throw FormatException('OO.cfg: $name.$key is not text');
      }
      return value;
    }

    final unex = table('unex', {'url'});
    final appearance = table('appearance', {'theme'});
    final url = string(unex, 'unex', 'url');
    final origin = url == null ? null : Uri.tryParse(url);
    if (url != null &&
        (origin == null || !origin.hasScheme || origin.host.isEmpty)) {
      throw FormatException('OO.cfg: unex.url $url is not an origin');
    }
    return OoConfig(
      unexUrl: origin,
      theme: string(appearance, 'appearance', 'theme'),
    );
  }

  /// The theme to start with when the person has chosen none: [theme] when it is a theme id
  /// or `system`; else Night Ops, with [warn] told of an unknown id.
  String defaultTheme({void Function(String message)? warn}) {
    final theme = this.theme;
    if (theme == null) return OoThemeId.nightOps.id;
    final valid = ooThemePreference(theme);
    if (valid != theme) {
      warn?.call(
        'OO.cfg: appearance.theme $theme is not a theme; using $valid',
      );
    }
    return valid;
  }
}

/// The format `text` declares by its first line ([OoConfig]).
///
/// Throws a [FormatException] for a name no format answers to, listing json, toml and yaml.
OoConfigFormat declaredFormat(String text) {
  // Only the first 256 characters are looked at.
  final plain = _withoutBom(text);
  final head = plain.length > 256 ? plain.substring(0, 256) : plain;
  final firstLine = head.split('\n').first.trimRight();
  if (firstLine.startsWith('#!')) {
    final words = firstLine.substring(2).trim().split(RegExp(r'\s+'));
    var name = words.isEmpty ? '' : words.last;
    name = name
        .split(RegExp(r'[/\\]'))
        .last
        .toLowerCase()
        .replaceFirst(RegExp(r'^\.+'), '');
    return switch (name) {
      'json' => OoConfigFormat.json,
      'yaml' || 'yml' => OoConfigFormat.yaml,
      'toml' || 'cfg' => OoConfigFormat.toml,
      _ => throw FormatException(
        'OO.cfg: #!$name is not one of json, toml, yaml',
      ),
    };
  }
  if (head.startsWith('%YAML')) return OoConfigFormat.yaml;
  return OoConfigFormat.toml;
}

/// The document of an `OO.cfg`'s text, in the format its first line declares.
Map<String, Object?> decodeOoConfig(String text) {
  final format = declaredFormat(text);
  final body = _body(text);
  final Object? decoded;
  try {
    decoded = switch (format) {
      OoConfigFormat.json =>
        body.trim().isEmpty ? <String, Object?>{} : jsonDecode(body),
      OoConfigFormat.yaml => _plain(loadYaml(body)),
      OoConfigFormat.toml => TomlDocument.parse(body).toMap(),
    };
  } on Object catch (error) {
    throw FormatException('OO.cfg is not valid ${format.name}: $error');
  }
  if (decoded == null) return {};
  if (decoded is! Map) throw const FormatException('OO.cfg is not a table');
  return decoded.cast<String, Object?>();
}

/// `text` with `[unex] url` set to [url], in the format it declares, keeping its `#!` line.
/// TOML and JSON are written anew, so their comments are lost; YAML is edited in place.
String withUnexUrl(String text, Uri url) {
  final format = declaredFormat(text);
  final origin = url.toString();
  final firstLine = _withoutBom(text).split('\n').first;
  final header = firstLine.startsWith('#!') ? '$firstLine\n' : '';
  final body = _body(text);
  switch (format) {
    case OoConfigFormat.yaml:
      final editor = YamlEditor(body.trim().isEmpty ? '{}' : body);
      final unex = decodeOoConfig(text)['unex'];
      if (unex is Map) {
        editor.update(['unex', 'url'], origin);
      } else {
        editor.update(['unex'], {'url': origin});
      }
      return '$header${editor.toString()}';
    case OoConfigFormat.json:
      final document = Map<String, Object?>.of(decodeOoConfig(text));
      document['unex'] = {
        ...?(document['unex'] as Map?)?.cast<String, Object?>(),
        'url': origin,
      };
      return '$header${const JsonEncoder.withIndent('  ').convert(document)}\n';
    case OoConfigFormat.toml:
      final document = Map<String, Object?>.of(decodeOoConfig(text));
      document['unex'] = {
        ...?(document['unex'] as Map?)?.cast<String, Object?>(),
        'url': origin,
      };
      return '$header${TomlDocument.fromMap(document)}';
  }
}

String _withoutBom(String text) =>
    text.startsWith('﻿') ? text.substring(1) : text;

/// The text without its `#!` line, which no parser sees.
String _body(String text) {
  final plain = _withoutBom(text);
  if (!plain.startsWith('#!')) return plain;
  final end = plain.indexOf('\n');
  return end < 0 ? '' : plain.substring(end + 1);
}

/// A YAML node as plain maps, lists and scalars.
Object? _plain(Object? node) => switch (node) {
  YamlMap map => {
    for (final entry in map.entries) '${entry.key}': _plain(entry.value),
  },
  YamlList list => [for (final item in list) _plain(item)],
  _ => node,
};
