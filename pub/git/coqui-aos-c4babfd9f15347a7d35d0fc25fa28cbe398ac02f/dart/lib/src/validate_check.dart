/// The checks that validate a manifest's data model, field by field.
library;

import 'defaults.dart' show defaultKeys;
import 'names.dart';
import 'source.dart';
import 'validate_types.dart';
import 'value.dart';

const List<String> _hookEvents = [
  'pre-env',
  'session-start',
  'session-end',
  'prompt',
  'turn-end',
  'cd',
  'pre-compact',
  'pre-tool',
  'post-tool',
];
const List<String> _scriptExtensions = [
  '.py',
  '.sh',
  '.bash',
  '.ps1',
  '.js',
  '.mjs',
  '.cjs',
];

bool _noNul(String text) => !text.contains('\u0000');
bool _anyText(String text) => true;

final RegExp _whiteSpace = RegExp(
  r'[\t-\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]',
  unicode: true,
);

final RegExp _lineBreaksAndControls = RegExp(
  r'[\u0000-\u001f\u007f-\u009f\u2028\u2029\u202a-\u202e\u2066-\u2069]',
  unicode: true,
);

bool _isLine(String text) =>
    text != '' && !_lineBreaksAndControls.hasMatch(text);

bool _isControl(int c) => c <= 0x20 || (c >= 0x7f && c <= 0x9f);

int _searchAny(String text, String chars) {
  for (var i = 0; i < text.length; i += 1) {
    if (chars.contains(text[i])) return i;
  }
  return -1;
}

/// An `https://` URL with a host and no user information or control characters.
bool isWebUrl(String text) {
  if (!text.startsWith('https://')) return false;
  final rest = text.substring(8);
  final end = _searchAny(rest, '/?#');
  final host = end == -1 ? rest : rest.substring(0, end);
  final tail = end == -1 ? '' : rest.substring(end);
  return host != '' &&
      !host.runes.any((c) => _isControl(c) || c == 0x40 || c == 0x5c) &&
      !tail.runes.any(_isControl);
}

bool _isMcpUrl(String text) {
  final lower = asciiLower(String.fromCharCodes(text.runes.take(8)));
  final String rest;
  if (lower.startsWith('https://')) {
    rest = text.substring(8);
  } else if (lower.startsWith('http://')) {
    rest = text.substring(7);
  } else {
    return false;
  }
  final end = _searchAny(rest, '/?');
  final host = end == -1 ? rest : rest.substring(0, end);
  final tail = end == -1 ? '' : rest.substring(end);
  bool bad(String c) => c == '#' || _whiteSpace.hasMatch(c);
  return host != '' &&
      !_chars(host).any((c) => c == '@' || bad(c)) &&
      !_chars(tail).any(bad);
}

Iterable<String> _chars(String text) => text.runes.map(String.fromCharCode);

final RegExp _versionPart = RegExp(r'^[0-9]{1,9}$');

bool _isVersion(String text) {
  final parts = text.split('.');
  return parts.length <= 3 && parts.every(_versionPart.hasMatch);
}

final RegExp _environmentName = RegExp(
  r'^[A-Za-z0-9]([A-Za-z0-9_.-]*[A-Za-z0-9])?$',
);

bool _isEnvironmentName(String text) => _environmentName.hasMatch(text);

final RegExp _digest = RegExp(r'^sha256:[0-9A-Fa-f]{64}$');

/// `sha256:` and 64 hexadecimal digits.
bool isDigest(String text) => _digest.hasMatch(text);

final RegExp _lowerIdFirstDigit = RegExp(r'^[a-z0-9][a-z0-9-]*$');
final RegExp _lowerId = RegExp(r'^[a-z][a-z0-9-]*$');

bool _isLowercaseId(String text, bool firstDigit) =>
    (firstDigit ? _lowerIdFirstDigit : _lowerId).hasMatch(text);

final RegExp _scriptComponent = RegExp(r'^[A-Za-z0-9_+-][A-Za-z0-9_+.-]*$');

bool _isScriptPath(String text) =>
    validRelativePath(text) && text.split('/').every(_scriptComponent.hasMatch);

/// A script a hook runs: a script path with an extension that names an interpreter.
bool _isHookRun(String text) =>
    _isScriptPath(text) && _scriptExtensions.any(text.endsWith);

final RegExp _commandName = RegExp(r'^[A-Za-z0-9][A-Za-z0-9._+-]*$');
final RegExp _commandNameStrict = RegExp(r'^[A-Za-z0-9][A-Za-z0-9._-]*$');
final RegExp _headerName = RegExp(r"^[A-Za-z0-9!#$%&'*+.^_`|~-]+$");

bool _isCommandName(String text) => _commandName.hasMatch(text);

/// A Git ref a skill table names: no `-` first, and none of `/ : \ @ #`, controls or spaces.
bool isGitRef(String text) {
  if (text.isEmpty) return false;
  final chars = text.runes.toList();
  bool bad(int c) =>
      '/:\\@#'.contains(String.fromCharCode(c)) || c <= 0x20 || c == 0x7f;
  return chars.first != 0x2d && chars.every((c) => !bad(c));
}

bool _osListOk(Object? value) =>
    value is List<Object> &&
    value.every((name) => name is String && validOsName(name));

/// One problem collector; each method checks one kind of value and reports at a pointer.
class Check {
  /// The problems found so far.
  final List<Issue> issues = [];

  /// Records a problem.
  void fail(String code, String at) => issues.add(Issue(at, code));

  /// Records a `schema` problem unless [ok].
  void require(bool ok, String at) {
    if (!ok) fail('schema', at);
  }

  /// [value] must be text that passes [ok].
  void string(Object? value, String at, bool Function(String text) ok) =>
      require(value is String && ok(value), at);

  /// [value] must be a boolean.
  void boolean(Object? value, String at) => require(value is bool, at);

  /// An integer in `low..=high`; a float of any value is `integer-required`.
  void integer(Object? value, String at, BigInt low, BigInt high) {
    if (value is double) {
      fail('integer-required', at);
    } else {
      require(value is BigInt && value >= low && value <= high, at);
    }
  }

  /// The list a field holds; a `schema` problem and no entries otherwise.
  List<Object> array(Object? value, String at) {
    if (value is List<Object>) return value;
    fail('schema', at);
    return const [];
  }

  /// Checks each entry of a list.
  void each(
    Object? value,
    String at,
    void Function(Object entry, String at) item,
  ) {
    final list = array(value, at);
    for (var index = 0; index < list.length; index += 1) {
      item(list[index], join(at, index));
    }
  }

  /// A list's entries must be distinct.
  void unique(Object? value, String at) {
    if (value is! List<Object>) return;
    final seen = <String>{};
    for (final item in value) {
      final key = canon(item);
      if (seen.contains(key)) {
        fail('schema', at);
        return;
      }
      seen.add(key);
    }
  }

  /// A mapping whose keys are only [known]; null for something that is not one.
  Document? object(Object? value, String at, List<String> known) {
    if (value is! Map<String, Object>) {
      fail('schema', at);
      return null;
    }
    for (final key in value.keys) {
      if (!known.contains(key)) fail('schema', join(at, key));
    }
    return value;
  }

  /// The mapping must hold each of [keys].
  void required(Document map, String at, List<String> keys) {
    for (final key in keys) {
      if (!map.containsKey(key)) fail('schema', at);
    }
  }

  /// A relative path an overlay may name.
  void relPath(Object? value, String at) =>
      string(value, at, validRelativePath);

  /// A list of operating system names.
  void osList(Object? value, String at) =>
      each(value, at, (name, there) => string(name, there, validOsName));

  /// A variable name an overlay may set.
  void variable(String name, String at) =>
      require(isVariableName(name) && !isReservedVariable(name), at);

  /// Checks a whole manifest, field by field.
  void document(Document doc) {
    for (final MapEntry(key: key, value: value) in doc.entries) {
      final at = '/${token(key)}';
      switch (key) {
        case 'name' || 'description':
          string(value, at, _anyText);
        case 'requires':
          each(
            value,
            at,
            (name, there) => string(name, there, overlayNameShaped),
          );
        case 'routing':
          each(value, at, (line, there) => string(line, there, _anyText));
        case 'rules' || 'context':
          each(value, at, (path, there) => relPath(path, there));
        case 'priority':
          integer(
            value,
            at,
            -(BigInt.one << 63),
            (BigInt.one << 63) - BigInt.one,
          );
        case 'defaults':
          final map = object(value, at, defaultKeys);
          if (map != null) {
            for (final entry in map.entries) {
              boolean(entry.value, join(at, entry.key));
            }
          }
        case 'skills':
          each(value, at, skillEntry);
          unique(value, at);
        case 'options':
          options(value, at);
        case 'env':
          envMap(value, at);
        case 'env_files':
          each(value, at, envFile);
        case 'paths':
          each(value, at, pathAddition);
        case 'env_scripts':
          each(value, at, (entry, there) => script(entry, there, false));
        case 'setup':
          each(value, at, (entry, there) => script(entry, there, true));
        case 'runtime':
          runtime(value, at);
        case 'scripts':
          scripts(value, at);
        case 'mcp':
          mcp(value, at);
        case 'harness':
          harness(value, at);
        case 'hooks':
          hooks(value, at);
        case 'about':
          about(value, at);
        case 'knowledge':
          final map = object(value, at, const ['serve']);
          if (map != null && map.containsKey('serve')) {
            boolean(map['serve'], join(at, 'serve'));
          }
        default:
      }
    }
  }

  /// A skill's name, or a table naming its source.
  void skillEntry(Object entry, String at) {
    if (entry is String) {
      require(validToolName(entry), at);
    } else if (entry is Map<String, Object>) {
      final map = object(entry, at, const ['name', 'source', 'ref', 'pin']);
      if (map == null) return;
      required(map, at, const ['name', 'source']);
      final sourceText = RegExp(r'[^ \t\r\n]');
      for (final MapEntry(key: key, value: value) in map.entries) {
        final there = join(at, key);
        if (key == 'name') {
          string(value, there, validToolName);
        } else if (key == 'source') {
          string(value, there, sourceText.hasMatch);
        } else if (key == 'ref') {
          string(value, there, isGitRef);
        } else if (key == 'pin') {
          string(value, there, isDigest);
        }
      }
    } else {
      fail('schema', at);
    }
  }

  /// What runs at each event: a script, a table, or false for nothing.
  void hooks(Object? value, String at) {
    if (value is! Map<String, Object>) {
      fail('schema', at);
      return;
    }
    for (final MapEntry(key: name, value: entry) in value.entries) {
      if (!_hookEvents.contains(name)) {
        fail('schema', at);
        continue;
      }
      final here = join(at, name);
      if (entry == false) continue;
      if (entry is String) {
        require(_isHookRun(entry), here);
      } else if (entry is Map<String, Object>) {
        hookTable(name, entry, here);
      } else {
        fail('schema', here);
      }
    }
  }

  /// A hook's table: which keys the [event] allows.
  void hookTable(String event, Document entry, String at) {
    var selects = false;
    var sets = event == 'cd';
    var refuses = false;
    if (event == 'pre-env' || event == 'pre-compact') {
      refuses = true;
    } else if (event == 'session-start' || event == 'pre-tool') {
      selects = true;
      sets = true;
      refuses = true;
    } else if (event == 'prompt') {
      sets = true;
      refuses = true;
    } else if (event == 'post-tool') {
      selects = true;
      sets = false;
    }
    final known = ['run', 'timeout', 'on_failure'];
    if (selects) known.add('match');
    if (sets) known.add('sets');
    final map = object(entry, at, known);
    if (map == null) return;
    for (final MapEntry(key: key, value: value) in map.entries) {
      final here = join(at, key);
      if (key == 'run') {
        string(value, here, _isHookRun);
      } else if (key == 'timeout') {
        integer(value, here, BigInt.one, BigInt.from(3600));
      } else if (key == 'on_failure') {
        string(
          value,
          here,
          (text) => text == 'report' || (refuses && text == 'refuse'),
        );
      } else if (key == 'match') {
        final choices = event == 'session-start'
            ? const ['new', 'resume', 'clear', 'compact', 'fork']
            : const [
                'read',
                'edit',
                'delete',
                'move',
                'search',
                'execute',
                'think',
                'fetch',
                'switch_mode',
                'other',
              ];
        hookList(value, here, choices.contains);
      } else if (key == 'sets') {
        hookList(
          value,
          here,
          (text) => isVariableName(text) && !isReservedVariable(text),
        );
      }
    }
  }

  /// A list of one or more distinct strings that pass [ok].
  void hookList(Object? value, String at, bool Function(String text) ok) {
    if (value is! List<Object>) {
      fail('schema', at);
      return;
    }
    require(value.isNotEmpty, at);
    for (var index = 0; index < value.length; index += 1) {
      string(value[index], join(at, index), ok);
    }
    unique(value, at);
  }

  /// An `options` table: each variable name that is not reserved has a table of `type` and
  /// optionally `default`, `sensitive` and `description`.
  void options(Object? value, String at) {
    if (value is! Map<String, Object>) {
      fail('schema', at);
      return;
    }
    for (final MapEntry(key: name, value: declared) in value.entries) {
      final here = join(at, name);
      variable(name, here);
      final table = object(declared, here, const [
        'type',
        'default',
        'sensitive',
        'description',
      ]);
      if (table == null) continue;
      required(table, here, const ['type']);
      final kind = table['type'];
      if (kind != null) {
        require(
          kind == 'string' || kind == 'integer' || kind == 'boolean',
          join(here, 'type'),
        );
      }
      final sensitive = table['sensitive'];
      if (sensitive != null) boolean(sensitive, join(here, 'sensitive'));
      final description = table['description'];
      if (description != null) {
        string(description, join(here, 'description'), _anyText);
      }
      final fallback = table['default'];
      if (fallback != null) {
        final there = join(here, 'default');
        if (!(fallback is String ||
            fallback is BigInt ||
            fallback is double ||
            fallback is bool)) {
          fail('schema', there);
        } else if (sensitive == true) {
          fail('option-sensitive-default', there);
        } else if (!((kind == 'string' && fallback is String) ||
            (kind == 'integer' && fallback is BigInt) ||
            (kind == 'boolean' && fallback is bool))) {
          fail('option-default-type', there);
        }
      }
    }
  }

  /// An `env` table: variable names and their value forms.
  void envMap(Object? value, String at) {
    if (value is! Map<String, Object>) {
      fail('schema', at);
      return;
    }
    for (final MapEntry(key: name, value: entry) in value.entries) {
      final here = join(at, name);
      variable(name, here);
      require(envValueOk(entry), here);
    }
  }

  /// An `env_files` entry.
  void envFile(Object entry, String at) {
    if (entry is String) {
      relPath(entry, at);
      return;
    }
    final map = object(entry, at, const ['path', 'format', 'os']);
    if (map == null) return;
    required(map, at, const ['path']);
    for (final MapEntry(key: key, value: value) in map.entries) {
      final here = join(at, key);
      if (key == 'path') {
        relPath(value, here);
      } else if (key == 'format') {
        string(value, here, (format) => format == 'aos' || format == 'dotenv');
      } else if (key == 'os') {
        osList(value, here);
      }
    }
  }

  /// A `paths` entry.
  void pathAddition(Object entry, String at) {
    final map = object(entry, at, const ['variable', 'path', 'position', 'os']);
    if (map == null) return;
    required(map, at, const ['variable', 'path']);
    for (final MapEntry(key: key, value: value) in map.entries) {
      final here = join(at, key);
      if (key == 'variable') {
        require(
          value is String &&
              isVariableName(value) &&
              !isReservedVariable(value),
          here,
        );
      } else if (key == 'path') {
        relPath(value, here);
      } else if (key == 'position') {
        string(
          value,
          here,
          (position) => position == 'prepend' || position == 'append',
        );
      } else if (key == 'os') {
        osList(value, here);
      }
    }
  }

  /// An `env_scripts` or `setup` entry.
  void script(Object entry, String at, bool setup) {
    final known = setup
        ? const ['interpreter', 'path', 'args', 'timeout', 'network', 'os']
        : const ['interpreter', 'path', 'args', 'timeout', 'os'];
    final map = object(entry, at, known);
    if (map == null) return;
    required(map, at, const ['interpreter', 'path']);
    for (final MapEntry(key: key, value: value) in map.entries) {
      final here = join(at, key);
      if (key == 'interpreter') {
        string(value, here, (name) => _isLowercaseId(name, false));
      } else if (key == 'path') {
        relPath(value, here);
      } else if (key == 'args') {
        each(value, here, (arg, there) => string(arg, there, _noNul));
      } else if (key == 'timeout') {
        integer(value, here, BigInt.zero, BigInt.from(2147483647));
      } else if (key == 'network') {
        boolean(value, here);
      } else if (key == 'os') {
        osList(value, here);
      }
    }
  }

  /// The `runtime` table.
  void runtime(Object? value, String at) {
    final map = object(value, at, const ['os', 'commands', 'python', 'node']);
    if (map == null) return;
    for (final MapEntry(key: key, value: entry) in map.entries) {
      final here = join(at, key);
      if (key == 'os') {
        osList(entry, here);
      } else if (key == 'commands') {
        each(entry, here, (command, there) {
          if (command is String) {
            string(command, there, _isCommandName);
          } else {
            final table = object(command, there, const ['name', 'os']);
            if (table == null) return;
            required(table, there, const ['name']);
            final name = table['name'];
            if (name != null) string(name, join(there, 'name'), _isCommandName);
            final os = table['os'];
            if (os != null) osList(os, join(there, 'os'));
          }
        });
      } else if (key == 'python') {
        each(
          entry,
          here,
          (variant, there) => this.variant(variant, there, true),
        );
      } else if (key == 'node') {
        each(
          entry,
          here,
          (variant, there) => this.variant(variant, there, false),
        );
      }
    }
  }

  /// A Python or Node runtime variant.
  void variant(Object value, String at, bool python) {
    final known = python
        ? const [
            'version',
            'requirements',
            'lock',
            'lock_digest',
            'environment',
            'os',
          ]
        : const ['version', 'package', 'environment', 'os'];
    final map = object(value, at, known);
    if (map == null) return;
    if (map.containsKey('requirements') && map.containsKey('lock')) {
      fail('schema', at);
    }
    if (map.containsKey('lock_digest') && !map.containsKey('lock')) {
      fail('schema', at);
    }
    for (final MapEntry(key: key, value: entry) in map.entries) {
      final here = join(at, key);
      if (key == 'version') {
        string(entry, here, _isVersion);
      } else if (key == 'requirements' || key == 'lock') {
        relPath(entry, here);
      } else if (key == 'lock_digest') {
        string(entry, here, isDigest);
      } else if (key == 'environment') {
        string(entry, here, _isEnvironmentName);
      } else if (key == 'package') {
        string(
          entry,
          here,
          (path) =>
              validRelativePath(path) &&
              (path == 'package.json' || path.endsWith('/package.json')),
        );
      } else if (key == 'os') {
        osList(entry, here);
      }
    }
  }

  /// The `scripts` table.
  void scripts(Object? value, String at) {
    if (value is! Map<String, Object>) {
      fail('schema', at);
      return;
    }
    for (final MapEntry(key: name, value: target) in value.entries) {
      final here = join(at, name);
      require(
        _commandNameStrict.hasMatch(name) && !asciiLower(name).endsWith('.exe'),
        here,
      );
      var fine = false;
      if (target == false) {
        fine = true;
      } else if (target is String) {
        fine = _isScriptPath(target) && _scriptExtensions.any(target.endsWith);
      } else if (target is Map<String, Object>) {
        final path = target['path'];
        final interpreter = target['interpreter'];
        final os = target['os'];
        fine =
            target.keys.every(
              (key) => key == 'path' || key == 'interpreter' || key == 'os',
            ) &&
            path is String &&
            _isScriptPath(path) &&
            interpreter is String &&
            _isLowercaseId(interpreter, false) &&
            (os == null || _osListOk(os));
      }
      require(fine, here);
    }
  }

  /// The `mcp` table.
  void mcp(Object? value, String at) {
    final map = object(value, at, const ['servers', 'skills']);
    if (map == null) return;
    final servers = map['servers'];
    if (servers != null) {
      each(servers, join(at, 'servers'), server);
    }
    final skills = map['skills'];
    if (skills != null) {
      each(skills, join(at, 'skills'), (link, there) {
        final table = object(link, there, const ['name', 'server']);
        if (table == null) return;
        required(table, there, const ['name', 'server']);
        for (final MapEntry(key: key, value: entry) in table.entries) {
          string(entry, join(there, key), validToolName);
        }
      });
    }
  }

  /// An MCP server.
  void server(Object value, String at) {
    const known = [
      'name',
      'transport',
      'timeout',
      'command',
      'args',
      'env',
      'env_passthrough',
      'url',
      'headers_env',
    ];
    final map = object(value, at, known);
    if (map == null) return;
    required(map, at, const ['name', 'transport']);
    final transport = map['transport'];
    var misshapen = false;
    if (transport == 'stdio') {
      misshapen =
          !map.containsKey('command') ||
          map.containsKey('url') ||
          map.containsKey('headers_env');
    } else if (transport == 'streamable-http') {
      misshapen =
          !map.containsKey('url') ||
          const [
            'command',
            'args',
            'env',
            'env_passthrough',
          ].any(map.containsKey);
    }
    if (misshapen) fail('schema', at);
    for (final MapEntry(key: key, value: entry) in map.entries) {
      final here = join(at, key);
      if (key == 'name') {
        string(entry, here, validToolName);
      } else if (key == 'transport') {
        string(
          entry,
          here,
          (text) => text == 'stdio' || text == 'streamable-http',
        );
      } else if (key == 'timeout') {
        final num? seconds = entry is BigInt
            ? entry.toDouble()
            : entry is double
            ? entry
            : null;
        require(
          seconds != null && seconds >= 0.001 && seconds <= 2147483647,
          here,
        );
      } else if (key == 'command') {
        string(entry, here, (command) => command != '' && _noNul(command));
      } else if (key == 'args') {
        each(entry, here, (arg, there) => string(arg, there, _noNul));
      } else if (key == 'env') {
        envMap(entry, here);
      } else if (key == 'env_passthrough') {
        each(entry, here, (name, there) => string(name, there, isVariableName));
        unique(entry, here);
      } else if (key == 'url') {
        string(entry, here, _isMcpUrl);
      } else if (key == 'headers_env') {
        if (entry is Map<String, Object>) {
          for (final MapEntry(key: header, value: variable) in entry.entries) {
            require(
              _headerName.hasMatch(header) &&
                  variable is String &&
                  isVariableName(variable),
              join(here, header),
            );
          }
        } else {
          fail('schema', here);
        }
      }
    }
  }

  /// The `harness` table.
  void harness(Object? value, String at) {
    if (value is! Map<String, Object>) {
      fail('schema', at);
      return;
    }
    for (final MapEntry(key: id, value: hints) in value.entries) {
      final here = join(at, id);
      require(_isLowercaseId(id, true), here);
      if (hints is! Map<String, Object>) {
        fail('schema', here);
        continue;
      }
      for (final MapEntry(key: key, value: entry) in hints.entries) {
        final there = join(here, key);
        if (key == 'context') {
          each(entry, there, (path, deeper) => relPath(path, deeper));
        } else if (key == 'skills') {
          boolean(entry, there);
        } else if (key == 'model' || key == 'effort') {
          string(entry, there, (text) => text != '');
        }
      }
    }
  }

  /// The informational `about` table.
  void about(Object? value, String at) {
    const known = [
      'summary',
      'icon',
      'homepage',
      'repository',
      'license',
      'authors',
      'tags',
    ];
    final map = object(value, at, known);
    if (map == null) return;
    final licenseChars = RegExp(r'^[A-Za-z0-9.+:() -]+$');
    final tagRun = RegExp(r'^[a-z0-9]+$');
    for (final MapEntry(key: key, value: entry) in map.entries) {
      final here = join(at, key);
      if (key == 'summary') {
        string(entry, here, _isLine);
      } else if (key == 'icon') {
        string(entry, here, (path) {
          final lower = asciiLower(path);
          return validRelativePath(path) &&
              (lower.endsWith('.png') || lower.endsWith('.webp'));
        });
      } else if (key == 'homepage' || key == 'repository') {
        string(entry, here, isWebUrl);
      } else if (key == 'license') {
        string(entry, here, licenseChars.hasMatch);
        if (entry is String && !validLicense(entry)) {
          fail('license-expression', here);
        }
      } else if (key == 'authors') {
        each(entry, here, (author, there) => string(author, there, _isLine));
      } else if (key == 'tags') {
        each(
          entry,
          here,
          (tag, there) => string(
            tag,
            there,
            (text) =>
                text.runes.length <= 32 &&
                text != '' &&
                text.split('-').every(tagRun.hasMatch),
          ),
        );
        unique(entry, here);
      }
    }
  }
}

/// Whether [entry] is exactly one form of env value.
bool envValueOk(Object? entry) {
  final forms = [
    entry is String && _noNul(entry),
    _objectForm(entry, 'value', const [
      'if_unset',
    ], (text) => text is String && _noNul(text)),
    _objectForm(entry, 'path', const [
      'if_unset',
    ], (path) => path is String && validRelativePath(path)),
    _objectForm(entry, 'secret', const [
      'if_unset',
      'optional',
    ], (name) => name is String && name != ''),
    _providerForm(entry),
  ];
  return forms.where((form) => form).length == 1;
}

bool _objectForm(
  Object? entry,
  String source,
  List<String> flags,
  bool Function(Object? value) ok,
) {
  if (entry is! Map<String, Object>) return false;
  final found = entry[source];
  if (found == null || !ok(found)) return false;
  for (final MapEntry(key: key, value: value) in entry.entries) {
    if (key != source && !(flags.contains(key) && value is bool)) return false;
  }
  return true;
}

final RegExp _providerName = RegExp(r'^[a-z][a-z0-9_-]*$');

bool _providerForm(Object? entry) {
  if (entry is! Map<String, Object>) return false;
  final provider = entry['provider'];
  final key = entry['key'];
  if (provider is! String || key is! String) return false;
  final providerOk = _providerName.hasMatch(provider);
  final keyOk =
      key != '' &&
      _noNul(key) &&
      (provider == 'file'
          ? validRelativePath(key)
          : provider == 'env'
          ? isVariableName(key)
          : true);
  if (!providerOk || !keyOk) return false;
  for (final MapEntry(key: field, value: value) in entry.entries) {
    if (field == 'provider' || field == 'key') continue;
    if (field == 'if_unset' || field == 'optional') {
      if (value is! bool) return false;
    } else if (field == 'value' || field == 'path' || field == 'secret') {
      return false;
    } else if (!(value is String || value is BigInt || value is bool)) {
      return false;
    }
  }
  return true;
}

final RegExp _pathSeparator = RegExp(r'[/\\]');

/// The rules JSON Schema cannot state.
void semantic(Document doc, Check check) {
  final requires = declared(doc, 'requires');
  for (var index = 0; index < requires.length; index += 1) {
    final name = requires[index];
    if (name is String && overlayNameShaped(name) && !validOverlayName(name)) {
      check.fail('reserved-name', '/requires/$index');
    }
  }
  final envNames = <String>{};
  final env = doc['env'];
  if (env is Map<String, Object>) {
    for (final name in env.keys) {
      if (envNames.contains(asciiUpper(name))) {
        check.fail('env-name-case-duplicate', '/env/${token(name)}');
      }
      envNames.add(asciiUpper(name));
      if (isReservedVariable(name)) {
        check.fail('reserved-variable', '/env/${token(name)}');
      }
    }
  }
  final paths = declared(doc, 'paths');
  for (var index = 0; index < paths.length; index += 1) {
    final variable = memberText(paths[index], 'variable');
    if (variable == null) continue;
    if (envNames.contains(asciiUpper(variable))) {
      check.fail('env-path-conflict', '/paths/$index/variable');
    }
    if (isReservedVariable(variable)) {
      check.fail('reserved-variable', '/paths/$index/variable');
    }
  }
  _skills(doc, check);
  _manifestNamed(doc, check);
  final mcp = doc['mcp'];
  if (mcp is! Map<String, Object>) return;
  final servers = declared(mcp, 'servers');
  final names = <String>{};
  for (var index = 0; index < servers.length; index += 1) {
    final server = servers[index];
    if (server is! Map<String, Object>) continue;
    final name = canon(server['name']);
    if (names.contains(name)) {
      check.fail('mcp-duplicate-server', '/mcp/servers/$index/name');
    }
    names.add(name);
    final command = server['command'];
    if (command is String &&
        _pathSeparator.hasMatch(command) &&
        !validRelativePath(command)) {
      check.fail('mcp-command', '/mcp/servers/$index/command');
    }
    final url = server['url'];
    if (url is String) {
      final split = url.indexOf('://');
      final after = split == -1 ? url : url.substring(split + 3);
      final authority = after.split('/').first;
      if (url.contains('#') || authority.contains('@')) {
        check.fail('mcp-url', '/mcp/servers/$index/url');
      }
    }
    final serverEnv = server['env'];
    if (serverEnv is Map<String, Object>) {
      final seen = <String>{};
      for (final variable in serverEnv.keys) {
        if (isReservedVariable(variable)) {
          check.fail(
            'reserved-variable',
            '/mcp/servers/$index/env/${token(variable)}',
          );
        }
        if (seen.contains(asciiUpper(variable))) {
          check.fail(
            'env-name-case-duplicate',
            '/mcp/servers/$index/env/${token(variable)}',
          );
        }
        seen.add(asciiUpper(variable));
      }
    }
  }
  final linked = <String>{};
  final links = declared(mcp, 'skills');
  for (var index = 0; index < links.length; index += 1) {
    final link = links[index];
    if (link is! Map<String, Object>) continue;
    final name = canon(link['name']);
    if (linked.contains(name)) {
      check.fail('mcp-duplicate-skill', '/mcp/skills/$index/name');
    }
    linked.add(name);
    if (!names.contains(canon(link['server']))) {
      check.fail('mcp-unknown-server', '/mcp/skills/$index/server');
    }
  }
}

/// The name of an entry of `skills`: the string, or a table's `name`.
String? skillName(Object? entry) => entry is Map<String, Object>
    ? memberText(entry, 'name')
    : entry is String
    ? entry
    : null;

/// The pointer of an entry's name: the entry, or the `name` of a table.
String skillAt(Object? entry, int index) =>
    entry is Map<String, Object> ? '/skills/$index/name' : '/skills/$index';

/// The rules of `skills` that need no files: names, and what a table's source may be.
void _skills(Document doc, Check check) {
  final seen = <String>{};
  final entries = declared(doc, 'skills');
  for (var index = 0; index < entries.length; index += 1) {
    final entry = entries[index];
    final name = skillName(entry);
    if (name == null) continue;
    final at = skillAt(entry, index);
    if (validToolName(name) && !validRelativePath(name)) {
      check.fail('skill-name', at);
    }
    if (seen.contains(name.toLowerCase())) check.fail('skill-duplicate', at);
    seen.add(name.toLowerCase());
    if (entry is! Map<String, Object>) continue;
    final source = entry['source'];
    if (source is! String) continue;
    final there = '/skills/$index';
    final Source parsed;
    try {
      parsed = Source.parse(source);
    } on Object {
      check.fail('invalid-source', '$there/source');
      continue;
    }
    if (parsed.isLocal()) {
      check.fail('local-source', '$there/source');
    } else if (parsed.kind == 'url' && !parsed.isWebUrl()) {
      check.fail('invalid-source', '$there/source');
    } else if (parsed.holdsCredentials()) {
      check.fail('source-credentials', '$there/source');
    }
    if (entry.containsKey('ref') &&
        (parsed.kind != 'git' || parsed.reference != null)) {
      check.fail('ref-not-allowed', '$there/ref');
    }
  }
}

/// A manifest is never another field's file: only `context` names `OVERLAY.md`, for its body, and
/// nothing names `overlay.cfg`.
void _manifestNamed(Document doc, Check check) {
  for (final (at, path) in namedFilesOf(doc)) {
    final isContext =
        at.startsWith('/context/') ||
        (at.startsWith('/harness/') && at.contains('/context/'));
    if (path == 'overlay.cfg' || (path == 'OVERLAY.md' && !isContext)) {
      check.fail('manifest-file', at);
    }
  }
}

/// Every `(pointer, relative path)` of a file a field requires to exist.
List<(String, String)> namedFilesOf(Document doc) {
  final out = <(String, String)>[];
  void each(String key, String? field) {
    final items = declared(doc, key);
    for (var index = 0; index < items.length; index += 1) {
      final item = items[index];
      if (field == null && item is String) {
        out.add(('/$key/$index', item));
      } else if (item is Map<String, Object>) {
        final name = field ?? 'path';
        final path = item[name];
        if (path is String) out.add(('/$key/$index/$name', path));
      }
    }
  }

  each('rules', null);
  each('context', null);
  each('env_files', null);
  each('env_scripts', 'path');
  each('setup', 'path');
  final runtime = doc['runtime'];
  if (runtime is Map<String, Object>) {
    for (final (runtimeName, fields) in const [
      ('python', ['requirements', 'lock']),
      ('node', ['package']),
    ]) {
      final variants = declared(runtime, runtimeName);
      for (var index = 0; index < variants.length; index += 1) {
        for (final field in fields) {
          final path = memberText(variants[index], field);
          if (path != null) {
            out.add(('/runtime/$runtimeName/$index/$field', path));
          }
        }
      }
    }
  }
  final hooks = doc['hooks'];
  if (hooks is Map<String, Object>) {
    for (final MapEntry(key: event, value: entry) in hooks.entries) {
      if (entry is String) {
        out.add(('/hooks/${token(event)}', entry));
      } else {
        final run = memberText(entry, 'run');
        if (run != null) out.add(('/hooks/${token(event)}/run', run));
      }
    }
  }
  final scripts = doc['scripts'];
  if (scripts is Map<String, Object>) {
    for (final MapEntry(key: name, value: target) in scripts.entries) {
      if (target is String) {
        out.add(('/scripts/${token(name)}', target));
      } else {
        final path = memberText(target, 'path');
        if (path != null) out.add(('/scripts/${token(name)}/path', path));
      }
    }
  }
  final harness = doc['harness'];
  if (harness is Map<String, Object>) {
    for (final MapEntry(key: id, value: hints) in harness.entries) {
      final paths = declared(hints, 'context');
      for (var index = 0; index < paths.length; index += 1) {
        final path = paths[index];
        if (path is String) {
          out.add(('/harness/${token(id)}/context/$index', path));
        }
      }
    }
  }
  final icon = memberText(doc['about'], 'icon');
  if (icon != null) out.add(('/about/icon', icon));
  return out;
}

/// Whether one informational field of `about` is valid; an unknown field is not.
bool aboutFieldValid(String field, Object value) {
  final check = Check();
  check.about(<String, Object>{field: value}, '/about');
  return check.issues.isEmpty;
}

/// Whether one field of a catalog's own (`name`, `description`, `homepage`) is valid.
bool catalogFieldValid(String field, Object? value) {
  if (value is! String) return false;
  if (field == 'name') return _isLine(value);
  if (field == 'description') return true;
  if (field == 'homepage') return isWebUrl(value);
  return false;
}

/// How deep parentheses may nest in a license expression: past it the text is no expression.
const int _licenseDepth = 128;

final RegExp _idChars = RegExp(r'^[A-Za-z0-9.-]+$');

bool _idCharsOk(String text) => _idChars.hasMatch(text);

bool _reference(String text, String kind) {
  var rest = text;
  if (text.startsWith('DocumentRef-')) {
    final body = text.substring('DocumentRef-'.length);
    final colon = body.indexOf(':');
    if (colon != -1) {
      if (!_idCharsOk(body.substring(0, colon))) return false;
      rest = body.substring(colon + 1);
    }
  }
  return rest.startsWith(kind) && _idCharsOk(rest.substring(kind.length));
}

/// Whether [expression] is an SPDX license expression: identifiers and `LicenseRef-`
/// references, `+`, `WITH`, `AND` and `OR` in upper case, and parentheses.
bool validLicense(String expression) {
  if (!RegExp(r'^[A-Za-z0-9.+:() -]+$').hasMatch(expression)) return false;
  final tokens = <String>[];
  final word = StringBuffer();
  void flush() {
    if (word.isNotEmpty) tokens.add(word.toString());
    word.clear();
  }

  for (final c in _chars(expression)) {
    if (c == '(' || c == ')' || c == ' ') {
      flush();
      if (c != ' ') tokens.add(c);
    } else {
      word.write(c);
    }
  }
  flush();
  final parser = _License(tokens);
  return parser.compound() && parser.at == tokens.length;
}

class _License {
  _License(this.tokens);

  int at = 0;
  int depth = 0;
  final List<String> tokens;

  String? _peek() => at < tokens.length ? tokens[at] : null;

  String? _take() {
    final token = at < tokens.length ? tokens[at] : null;
    at += 1;
    return token;
  }

  bool _licenseOrGroup() {
    final token = _take();
    if (token == '(') {
      depth += 1;
      final closed = depth <= _licenseDepth && compound() && _take() == ')';
      depth -= 1;
      return closed;
    }
    if (token == null ||
        token == 'AND' ||
        token == 'OR' ||
        token == 'WITH' ||
        token == ')') {
      return false;
    }
    final license =
        _reference(token, 'LicenseRef-') ||
        _idCharsOk(
          token.endsWith('+') ? token.substring(0, token.length - 1) : token,
        );
    if (!license) return false;
    if (_peek() == 'WITH') {
      _take();
      final exception = _take();
      return exception != null &&
          (_idCharsOk(exception) || _reference(exception, 'AdditionRef-'));
    }
    return true;
  }

  bool _term() {
    if (!_licenseOrGroup()) return false;
    while (_peek() == 'AND') {
      _take();
      if (!_licenseOrGroup()) return false;
    }
    return true;
  }

  bool compound() {
    if (!_term()) return false;
    while (_peek() == 'OR') {
      _take();
      if (!_term()) return false;
    }
    return true;
  }
}
