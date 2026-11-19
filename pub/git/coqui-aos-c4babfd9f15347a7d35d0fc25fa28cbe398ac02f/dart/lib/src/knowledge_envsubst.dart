/// The one place an `envsubst` file's text is rendered, so the engine can be swapped without
/// touching how a file is found, counted or served.
///
/// No Dart package implements GNU `envsubst` with the options the specification defines
/// (`pattern`, `features`, `undefined`), so this is a small implementation of just those. It
/// reads `$NAME` and `${NAME}` as the pattern allows, and of the features only `escape` (`$$`)
/// and `default` (`${NAME:-word}`, `${NAME-word}`); a file that asks for another feature is
/// served as a file for an engine this server lacks.
library;

import 'knowledge_core.dart';

/// What a name with no value is when a file's first line does not say: kept as it was written.
const String knowledgeUndefinedDefault = 'debug';

/// The values `undefined` takes.
const List<String> knowledgeUndefinedModes = ['strict', 'lenient', 'debug'];

/// The patterns a file may ask for.
const List<String> knowledgePatterns = ['braced', 'unbraced', 'all'];

/// The features of the shell-style dialects a file may name.
const List<String> knowledgeShellFeatures = [
  'escape',
  'default',
  'assign',
  'alternate',
  'nested',
  'unquote',
  'slice',
  'replace',
  'trim',
  'case',
  'length',
];

/// The features this server implements.
const Set<String> knowledgeImplementedFeatures = {'escape', 'default'};

const Map<String, (String, Set<String>)> _dialects = {
  'gnu': ('all', <String>{}),
  'a8m': ('all', {'escape', 'default'}),
  'drone': ('braced', {'escape', 'default'}),
};

/// The pattern, features and undefined mode the options of an `envsubst` first line ask for.
/// Throws [KnowledgeError] `template-error` for a value an option does not have.
({String pattern, Set<String> features, String undefined}) envsubstOptions(
  Map<String, String> options,
) {
  final dialect = _dialects[options['dialect'] ?? 'gnu'];
  if (dialect == null) {
    throw KnowledgeError('template-error', 'dialect is gnu, a8m or drone');
  }
  var (pattern, features) = dialect;
  final asked = options['pattern'];
  if (asked != null) {
    if (!knowledgePatterns.contains(asked)) {
      throw KnowledgeError(
        'template-error',
        'pattern is braced, unbraced or all',
      );
    }
    pattern = asked;
  }
  final list = options['features'];
  if (list != null) {
    final named = list.split(',').toSet();
    if (!named.every(knowledgeShellFeatures.contains)) {
      throw KnowledgeError('template-error', 'a feature that is not defined');
    }
    features = named;
  }
  final undefined = options['undefined'] ?? knowledgeUndefinedDefault;
  if (!knowledgeUndefinedModes.contains(undefined)) {
    throw KnowledgeError(
      'template-error',
      'undefined is strict, lenient or debug',
    );
  }
  return (pattern: pattern, features: features, undefined: undefined);
}

final RegExp _name = RegExp(r'[A-Za-z_][A-Za-z0-9_]*');

/// [text] with each `$NAME` and `${NAME}` (as [pattern] allows) replaced by the value of
/// `NAME` in [values]; a name with no value follows [undefined]. With the `escape` feature `$$`
/// is `$`; with `default`, `${NAME:-word}` and `${NAME-word}` give the word where the name has
/// no value (`:-` also where it is empty). Anything else is text.
String renderEnvsubst(
  String text,
  Map<String, String> values, {
  String pattern = 'all',
  Set<String> features = const {},
  String undefined = knowledgeUndefinedDefault,
}) {
  final out = StringBuffer();
  var i = 0;
  while (i < text.length) {
    if (text[i] != r'$') {
      out.write(text[i]);
      i += 1;
      continue;
    }
    if (features.contains('escape') && text.startsWith(r'$$', i)) {
      out.write(r'$');
      i += 2;
      continue;
    }
    String? name;
    String? word;
    var colon = false;
    var end = i;
    if ((pattern == 'braced' || pattern == 'all') &&
        text.startsWith(r'${', i)) {
      final found = _name.matchAsPrefix(text, i + 2);
      if (found != null) {
        final j = found.end;
        if (text.startsWith('}', j)) {
          name = found.group(0);
          end = j + 1;
        } else if (features.contains('default') &&
            (text.startsWith(':-', j) || text.startsWith('-', j))) {
          colon = text.startsWith(':-', j);
          final close = text.indexOf('}', j);
          if (close >= 0) {
            name = found.group(0);
            word = text.substring(j + (colon ? 2 : 1), close);
            end = close + 1;
          }
        }
      }
    }
    if (name == null && (pattern == 'unbraced' || pattern == 'all')) {
      final found = _name.matchAsPrefix(text, i + 1);
      if (found != null) {
        name = found.group(0);
        end = found.end;
      }
    }
    if (name == null) {
      out.write(r'$');
      i += 1;
      continue;
    }
    final value = values[name];
    if (word != null && (value == null || (colon && value.isEmpty))) {
      out.write(word);
    } else if (value != null) {
      out.write(value);
    } else if (undefined == 'strict') {
      throw KnowledgeError('template-error', 'a name has no value: $name');
    } else if (undefined == 'debug') {
      out.write(text.substring(i, end));
    }
    i = end;
  }
  return out.toString();
}
