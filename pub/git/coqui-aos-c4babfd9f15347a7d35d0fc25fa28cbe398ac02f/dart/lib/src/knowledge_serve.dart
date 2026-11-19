/// What `kb_read` serves for a file (`spec/v0/knowledge.md#formats-and-templates`): the text
/// as written, expanded (`${NAME}` and `@path`), rendered by an engine this server has, or kept
/// with the context an engine this server lacks would have used.
library;

import 'dart:typed_data';

import 'error.dart';
import 'front_matter.dart';
import 'knowledge_core.dart';
import 'knowledge_envsubst.dart';
import 'knowledge_mustache.dart';
import 'limits.dart';
import 'names.dart';
import 'template.dart';
import 'text.dart';

bool _over(int value, Limit bound) => bound != null && value > bound;

String _optionText(Object? declared, String name, Object value) {
  final kind = declared is Map ? declared['type'] : null;
  if (kind == 'string' && value is String) return value;
  if (kind == 'integer' && value is BigInt) return value.toString();
  if (kind == 'boolean' && value is bool) return value ? 'true' : 'false';
  throw KnowledgeError('invalid-option', name);
}

TemplateFacts _facts(KnowledgeOverlay overlay) {
  final given = overlay.facts;
  return TemplateFacts(
    root: given.root,
    state: given.state,
    name: given.name ?? normalizeName(overlay.identity),
    scope: given.scope,
    project: given.project,
    options: given.options,
  );
}

/// What a template reads: the roots `overlay`, `project`, `caller`, `env`, `options` and
/// `context`. A sensitive option or variable is never in it.
Map<String, Object?> _data(
  KnowledgeOverlay overlay,
  Map<String, Object?> context,
  CallerFacts call,
) {
  final facts = _facts(overlay);
  final options = <String, Object?>{};
  final declaredOptions = overlay.manifest['options'];
  if (declaredOptions is Map<String, Object>) {
    for (final MapEntry(key: name, value: option) in declaredOptions.entries) {
      if (option is! Map || option['sensitive'] == true) continue;
      if (facts.options.containsKey(name)) {
        _optionText(option, name, facts.options[name]!);
        options[name] = facts.options[name];
      } else if (option.containsKey('default')) {
        _optionText(option, name, option['default']! as Object);
        options[name] = option['default'];
      }
    }
  }
  return {
    'overlay': {
      if (facts.root != null) 'root': facts.root,
      if (facts.state != null) 'state': facts.state,
      if (facts.name != null) 'name': facts.name,
      if (facts.scope != null) 'scope': facts.scope,
    },
    'project': {if (facts.project != null) 'root': facts.project},
    'caller': {
      if (call.role != null) 'role': call.role,
      if (call.profile != null) 'profile': call.profile,
    },
    'env': {
      for (final name in call.envAllow)
        if (call.env.containsKey(name) && !isSensitiveVariable(name))
          name: call.env[name],
    },
    'options': options,
    'context': context,
  };
}

Map<String, String> _envsubstValues(
  KnowledgeOverlay overlay,
  Map<String, Object?> data,
  Map<String, Object?> context,
) {
  final Map<String, String> values;
  try {
    values = Map.of(templateValues(overlay.manifest, _facts(overlay)));
  } on AosError catch (error) {
    throw KnowledgeError('invalid-option', error.subject);
  }
  final caller = data['caller']! as Map;
  if (caller.containsKey('role')) {
    values['AOS_CALLER_ROLE'] = caller['role']! as String;
  }
  if (caller.containsKey('profile')) {
    values['AOS_CALLER_PROFILE'] = caller['profile']! as String;
  }
  for (final MapEntry(key: name, value: value)
      in (data['env']! as Map).entries) {
    values['AOS_ENV_$name'] = value! as String;
  }
  for (final MapEntry(key: key, value: value) in context.entries) {
    if (value is bool) {
      values['AOS_ARG_$key'] = value ? 'true' : 'false';
    } else if (value is String || value is int || value is BigInt) {
      values['AOS_ARG_$key'] = '$value';
    }
  }
  return values;
}

String _flow(Object? value) {
  if (value is bool) return value ? 'true' : 'false';
  if (value is int || value is BigInt) return '$value';
  if (value is String) {
    final out = StringBuffer('"');
    for (final unit in value.codeUnits) {
      if (unit == 0x22 || unit == 0x5c) {
        out
          ..write(r'\')
          ..writeCharCode(unit);
      } else if (unit < 0x20) {
        out.write('\\u${unit.toRadixString(16).padLeft(4, '0')}');
      } else {
        out.writeCharCode(unit);
      }
    }
    return (out..write('"')).toString();
  }
  if (value is List) return '[${value.map(_flow).join(', ')}]';
  final map = value! as Map;
  final keys = map.keys.cast<String>().toList()..sort(compareText);
  return '{${keys.map((key) => '${_flow(key)}: ${_flow(map[key])}').join(', ')}}';
}

String _contextBlock(String engine, Map<String, Object?> data) {
  final parts = ['"engine": ${_flow(engine)}'];
  for (final root in const [
    'overlay',
    'project',
    'caller',
    'env',
    'options',
    'context',
  ]) {
    if ((data[root]! as Map).isNotEmpty) {
      parts.add('"$root": ${_flow(data[root])}');
    }
  }
  return '---\naos_template_context: {${parts.join(', ')}}\n---\n';
}

/// Whether this server renders the file's engine with the options its first line asks for.
/// A file whose options it cannot honour (a value `engines` lists as `-engine:option=value`,
/// or an envsubst feature this package lacks), or an engine it lacks, is kept with its context.
bool knowledgeRenders(KnowledgeFile item, List<String> engines) {
  final engine = item.engine;
  if (engine == null || !engines.contains(engine)) return false;
  for (final MapEntry(key: key, value: value) in item.options.entries) {
    if (engines.contains('-$engine:$key=$value')) return false;
  }
  if (engine == 'envsubst') {
    try {
      return knowledgeImplementedFeatures.containsAll(
        envsubstOptions(item.options).features,
      );
    } on KnowledgeError {
      return true;
    }
  }
  return true;
}

String _render(
  KnowledgeOverlay overlay,
  KnowledgeSet held,
  KnowledgeFile item,
  Map<String, Object?> context,
  CallerFacts call,
  KnowledgeSettings limit,
  bool here,
) {
  final data = _data(overlay, context, call);
  final engine = item.engine!;
  final String out;
  if (!here) {
    final text = item.text;
    final end = text.indexOf('\n');
    final first = end < 0 ? '$text\n' : text.substring(0, end + 1);
    final rest = end < 0 ? '' : text.substring(end + 1);
    out = first + _contextBlock(engine, data) + rest;
  } else if (engine == 'envsubst') {
    final asked = envsubstOptions(item.options);
    out = renderEnvsubst(
      item.rest,
      _envsubstValues(overlay, data, context),
      pattern: asked.pattern,
      features: asked.features,
      undefined: asked.undefined,
    );
  } else {
    final escape = item.options['escape'] ?? 'html';
    if (escape != 'html' && escape != 'none') {
      throw KnowledgeError('template-error', 'escape is html or none');
    }
    out = renderMustache(
      item.rest,
      data,
      path: item.path,
      escape: escape,
      settings: limit,
      partials: (name, fromPath) {
        final target = name.isEmpty ? null : _resolve(fromPath, name, held);
        return target == null
            ? null
            : (path: target, text: held.files[target]!.rest);
      },
    );
  }
  if (_over(utf8Length(out), limit.maxRenderedBytes)) {
    throw KnowledgeError('too-large', 'the rendered text');
  }
  return out;
}

class _Context {
  _Context(
    this.overlay,
    this.held,
    this.context,
    this.call,
    this.limit,
    this.engines,
  );

  final KnowledgeOverlay overlay;
  final KnowledgeSet held;
  final Map<String, Object?> context;
  final CallerFacts call;
  final KnowledgeSettings limit;
  final List<String> engines;
  Map<String, String>? values;
  int added = 0;

  Map<String, String> get names => values ??= _envsubstValues(
    overlay,
    _data(overlay, context, call),
    context,
  );
}

/// The text `kb_read` serves for the file at [path] of [held], before it is cut into lines,
/// for a call with [context] and [call]'s facts, by a server that renders [engines].
///
/// Throws [KnowledgeError] (`include-error`, `invalid-option`, `template-error`, `too-large`).
String knowledgeServe(
  KnowledgeOverlay overlay,
  KnowledgeSet held,
  String path,
  Map<String, Object?> context,
  CallerFacts call,
  KnowledgeSettings limit,
  List<String> engines,
) {
  final item = held.files[path]!;
  if (item.asWritten) return item.rest;
  if (item.engine != null) {
    return _render(
      overlay,
      held,
      item,
      context,
      call,
      limit,
      knowledgeRenders(item, engines),
    );
  }
  final ctx = _Context(overlay, held, context, call, limit, engines);
  final out = _markdown(item.rest, path, ctx, [path]);
  if (_over(utf8Length(out), limit.maxRenderedBytes)) {
    throw KnowledgeError('too-large', 'the served text');
  }
  return out;
}

final RegExp _fence = RegExp(r'^( {0,3})(`{3,}|~{3,})');
final RegExp _placeholder = RegExp(r'\$\{([A-Za-z_][A-Za-z0-9_]*)\}');
final RegExp _lines = RegExp(r'[^\n]*\n|[^\n]+');
final RegExp _drive = RegExp(r'^[A-Za-z]:');

(String, int)? _opensFence(String line) {
  final found = _fence.firstMatch(line);
  if (found == null) return null;
  final run = found.group(2)!;
  return (run[0], run.length);
}

bool _blank(String text) => text
    .split('')
    .every((c) => c == ' ' || c == '\t' || c == '\r' || c == '\n');

bool _closesFence(String line, (String, int) fence) {
  final found = _fence.firstMatch(line);
  if (found == null) return false;
  final run = found.group(2)!;
  return run[0] == fence.$1 &&
      run.length >= fence.$2 &&
      _blank(line.substring(found.end));
}

/// Which characters of a line are in a code span: a run of backticks, to the next run of as
/// many on the same line; a run with none is itself.
List<bool> _codeMask(String line) {
  final mask = List<bool>.filled(line.length, false);
  var i = 0;
  while (i < line.length) {
    if (line[i] != '`') {
      i += 1;
      continue;
    }
    var j = i;
    while (j < line.length && line[j] == '`') {
      j += 1;
    }
    var k = j;
    int? end;
    while (k < line.length) {
      if (line[k] == '`') {
        var m = k;
        while (m < line.length && line[m] == '`') {
          m += 1;
        }
        if (m - k == j - i) {
          end = m;
          break;
        }
        k = m;
      } else {
        k += 1;
      }
    }
    if (end == null) {
      i = j;
    } else {
      for (var n = i; n < end; n += 1) {
        mask[n] = true;
      }
      i = end;
    }
  }
  return mask;
}

String _markdown(String text, String path, _Context ctx, List<String> chain) {
  final out = StringBuffer();
  (String, int)? fence;
  for (final match in _lines.allMatches(text)) {
    final line = match.group(0)!;
    final all = List<bool>.filled(line.length, true);
    if (fence != null) {
      out.write(_line(line, all, path, ctx, chain));
      if (_closesFence(line, fence)) fence = null;
      continue;
    }
    fence = _opensFence(line);
    out.write(
      _line(line, fence != null ? all : _codeMask(line), path, ctx, chain),
    );
  }
  return out.toString();
}

const String _trailing = '.,;:!?)]}\'"';
const String _spaces = ' \t\r\n\f\v';

String _line(
  String line,
  List<bool> inCode,
  String path,
  _Context ctx,
  List<String> chain,
) {
  final out = StringBuffer();
  var i = 0;
  while (i < line.length) {
    if (!inCode[i] && line.startsWith(r'$$', i)) {
      out.write(r'$');
      i += 2;
      continue;
    }
    final found = inCode[i] ? null : _placeholder.matchAsPrefix(line, i);
    if (found != null) {
      final names = ctx.names;
      final value = names[found.group(1)!];
      if (value != null) {
        out.write(value);
        i = found.end;
        continue;
      }
    }
    if (line[i] == '@' &&
        !inCode[i] &&
        (i == 0 || ' \t\r\f\v'.contains(line[i - 1]))) {
      var end = i + 1;
      while (end < line.length && !_spaces.contains(line[end])) {
        end += 1;
      }
      var name = line.substring(i + 1, end);
      while (name.isNotEmpty && _trailing.contains(name[name.length - 1])) {
        name = name.substring(0, name.length - 1);
      }
      final target = name.isEmpty ? null : _resolve(path, name, ctx.held);
      final included = target == null ? null : _include(target, ctx, chain);
      if (included != null) {
        out.write(included);
        i += 1 + name.length;
        continue;
      }
    }
    out.write(line[i]);
    i += 1;
  }
  return out.toString();
}

/// The file of the knowledge an include names, or null: the name is resolved from the
/// including file's folder, `..` may rise only to the root, and what is not a served file of
/// the overlay is not an include.
String? _resolve(String path, String name, KnowledgeSet held) {
  if (name.startsWith('/') ||
      name.startsWith('~') ||
      name.contains(r'\') ||
      _drive.hasMatch(name)) {
    return null;
  }
  final parts = path.split('/')..removeLast();
  for (final part in name.split('/')) {
    if (part.isEmpty) return null;
    if (part == '.') continue;
    if (part == '..') {
      if (parts.isEmpty) return null;
      parts.removeLast();
    } else {
      parts.add(part);
    }
  }
  final target = parts.join('/');
  return validRelativePath(target) && held.files.containsKey(target)
      ? target
      : null;
}

String? _include(String target, _Context ctx, List<String> chain) {
  final item = ctx.held.files[target]!;
  final limit = ctx.limit;
  final String text;
  if (item.asWritten) {
    text = item.rest;
  } else if (item.engine != null) {
    if (!knowledgeRenders(item, ctx.engines)) return null;
    text = _render(
      ctx.overlay,
      ctx.held,
      item,
      ctx.context,
      ctx.call,
      limit,
      true,
    );
  } else {
    if (chain.contains(target)) {
      throw KnowledgeError('include-error', 'an include comes back to $target');
    }
    if (_over(chain.length, limit.maxIncludeDepth)) {
      throw KnowledgeError('include-error', 'includes are nested too deep');
    }
    var body = item.rest;
    final bytes = encodeUtf8(body);
    final split = splitFrontMatter(bytes, Settings());
    if (split.splitCase == 'present' || split.splitCase == 'empty') {
      body = decodeUtf8(Uint8List.sublistView(bytes, split.bodyOffset!));
    }
    text = _markdown(body, target, ctx, [...chain, target]);
  }
  ctx.added += utf8Length(text);
  if (_over(ctx.added, limit.maxRenderedBytes)) {
    throw KnowledgeError('too-large', 'the included text');
  }
  if (text.endsWith('\r\n')) return text.substring(0, text.length - 2);
  return text.endsWith('\n') ? text.substring(0, text.length - 1) : text;
}
