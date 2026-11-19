/// Templating (`spec/v0/harness.md#templating`): `${NAME}` in the context a consumer assembles
/// and in the body of a bundled skill it serves. Only the names listed, the overlay's
/// non-sensitive options among them, are replaced; every other text is itself. Nothing runs, no
/// file is read, and an installed file is never changed.
library;

import 'dart:typed_data';

import 'error.dart';
import 'front_matter.dart';
import 'limits.dart';
import 'names.dart';
import 'text.dart';
import 'value.dart';

/// What a consumer knows about the overlay a text is read for. A name with no value is left as
/// written in the text.
class TemplateFacts {
  /// Facts with the values given; each null is a name with no value.
  const TemplateFacts({
    this.root,
    this.state,
    this.name,
    this.scope,
    this.project,
    this.options = const {},
  });

  /// `AOS_OVERLAY_ROOT`: the absolute root the consumer exposes, as it holds it.
  final String? root;

  /// `AOS_OVERLAY_STATE`: the overlay's state directory, when it has one.
  final String? state;

  /// `AOS_OVERLAY_NAME`: the overlay's normalized identity.
  final String? name;

  /// `AOS_OVERLAY_SCOPE`: the label the consumer gives the scope the overlay is active in.
  final String? scope;

  /// `AOS_PROJECT_ROOT`: the absolute root of the project the session works in.
  final String? project;

  /// The values the user configured for the overlay's options, by name: a `String`, a `BigInt`
  /// or a `bool`. A name the overlay does not declare is ignored.
  final Map<String, Object> options;
}

String _optionText(Object? kind, String name, Object value) {
  if (kind == 'string' && value is String) return value;
  if (kind == 'integer' && value is BigInt) return value.toString();
  if (kind == 'boolean' && value is bool) return value ? 'true' : 'false';
  throw AosError('invalid-option', name);
}

/// The names that are replaced and their text, to give to [template], [contextText] and
/// [skillText]. A name with no value is not in it, and a sensitive option never is. An option's
/// value is the configured one, else its default, else none.
///
/// Throws `invalid-option`, the option's name its subject, when a configured value or a default
/// is not of the declared type: it is never converted.
Map<String, String> templateValues(Document manifest, TemplateFacts facts) {
  final values = <String, String>{};
  for (final (name, value) in [
    ('AOS_OVERLAY_ROOT', facts.root),
    ('AOS_OVERLAY_STATE', facts.state),
    ('AOS_OVERLAY_NAME', facts.name),
    ('AOS_OVERLAY_SCOPE', facts.scope),
    ('AOS_PROJECT_ROOT', facts.project),
  ]) {
    if (value != null) values[name] = value;
  }
  final options = manifest['options'];
  if (options is Map<String, Object>) {
    for (final MapEntry(key: name, value: declared) in options.entries) {
      if (declared is! Map<String, Object> ||
          declared['sensitive'] == true ||
          !isVariableName(name)) {
        continue;
      }
      final configured = facts.options[name];
      final value = configured ?? declared['default'];
      if (value != null) {
        values[name] = _optionText(declared['type'], name, value);
      }
    }
  }
  return values;
}

/// [text] with each `${NAME}` whose name [values] holds replaced, read from left to right: `$${`
/// is `${`, and every other text is itself (an unknown name, an operator, `$NAME`, `$(...)`, a
/// lone `$$`). A value is inserted as it is and never read again.
String template(String text, Map<String, String> values) {
  final out = StringBuffer();
  var at = 0;
  while (at < text.length) {
    if (text.startsWith(r'$${', at)) {
      out.write(r'${');
      at += 3;
      continue;
    }
    if (text.startsWith(r'${', at)) {
      final end = text.indexOf('}', at + 2);
      if (end != -1) {
        final value = values[text.substring(at + 2, end)];
        if (value != null) {
          out.write(value);
          at = end + 1;
          continue;
        }
      }
    }
    final dollar = text.indexOf(r'$', at + 1);
    final stop = dollar == -1 ? text.length : dollar;
    out.write(text.substring(at, stop));
    at = stop;
  }
  return out.toString();
}

/// A context file as the agent receives it: a closed front matter block dropped, the rest
/// templated. Throws `too-large` for a head past `manifest_bytes` and `invalid-utf8`.
String contextText(
  Uint8List data,
  Map<String, String> values,
  Settings settings,
) => template(decodeUtf8(contextFileText(data, settings)), values);

/// A bundled skill's `SKILL.md` as served: the front matter exactly as written, the body
/// templated. Throws as [contextText] does.
String skillText(
  Uint8List data,
  Map<String, String> values,
  Settings settings,
) {
  final split = splitFrontMatter(data, settings);
  if (split.splitCase == 'too-large') {
    throw AosError(
      'too-large',
      'front matter',
    ).over('manifest_bytes').onLine(1);
  }
  final offset = split.bodyOffset ?? 0;
  return decodeUtf8(Uint8List.sublistView(data, 0, offset)) +
      template(decodeUtf8(Uint8List.sublistView(data, offset)), values);
}
