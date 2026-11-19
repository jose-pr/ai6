/// Applying an overlay: the environment it builds and what it needs, on one operating system.
library;

import 'dart:typed_data';

import 'defaults.dart';
import 'environment.dart';
import 'error.dart';
import 'hooks.dart';
import 'kinds.dart';
import 'names.dart';
import 'read.dart';
import 'same_digest.dart';
import 'text.dart';
import 'tree.dart';
import 'validate.dart';
import 'value.dart';

/// A setting a reading turned out to use that is not the default: its id and what it is about,
/// never a value.
class NonDefaultSetting {
  /// Creates the report. [id] is a word of `nonDefaultId`, or a host's own word
  /// (`read-only-not-enforced`), which is shown as given.
  const NonDefaultSetting(this.id, this.subject);

  /// The setting's id.
  final String id;

  /// What it is about: a variable, a path, a name.
  final String subject;
}

NonDefaultSetting _setting(String id, String subject) =>
    NonDefaultSetting(id, subject);

Document? _mapOf(Object? value) => value is Map<String, Object> ? value : null;

List<Object> _listOf(Object? value) => value is List<Object> ? value : const [];

List<String> _stringsOf(Object? entry, String key) {
  final list = entry is Map<String, Object> ? entry[key] : null;
  return [
    for (final item in _listOf(list))
      if (item is String) item,
  ];
}

/// A non-negative whole number a manifest wrote, or [fallback].
int _unsignedOf(Object? entry, String key, int fallback) {
  final value = entry is Map<String, Object> ? entry[key] : null;
  return value is BigInt && value >= BigInt.zero ? value.toInt() : fallback;
}

bool _isTrue(Document entry, String key) => entry[key] == true;

String _referenceName(Document entry) {
  final secret = entry['secret'];
  if (secret is String) return 'secret:$secret';
  return '${textOf(entry, 'provider')}:${textOf(entry, 'key')}';
}

final RegExp _trailingSlashes = RegExp(r'[/\\]+$');

/// An overlay-relative path under [root], with the platform's separator; a path that is not a
/// valid relative path is `invalid-reference`.
String resolvePath(String root, String relative, bool windows) {
  if (!validRelativePath(relative)) {
    throw AosError('invalid-reference', relative);
  }
  final separator = windows ? r'\' : '/';
  return '${root.replaceFirst(_trailingSlashes, '')}$separator'
      '${relative.split('/').join(separator)}';
}

bool _isReference(Object? entry) =>
    entry is Map<String, Object> &&
    (entry.containsKey('provider') || entry.containsKey('secret'));

void _applyMapping(
  Environment env,
  Document mapping,
  Facts facts,
  String root,
  List<NonDefaultSetting> nonDefaults,
) {
  final windows = env.windows;
  final before = env.clone();
  for (final MapEntry(key: name, value: given) in mapping.entries) {
    if (!isVariableName(name)) throw AosError('invalid-reference', name);
    if (isReservedVariable(name)) throw AosError('reserved-variable', name);
    final Document entry;
    if (given is String) {
      entry = <String, Object>{'value': given};
    } else if (given is Map<String, Object>) {
      entry = given;
    } else {
      continue;
    }
    if (_isTrue(entry, 'if_unset') && before.get(name) != null) continue;
    var secret = false;
    String value;
    final text = entry['value'];
    final path = entry['path'];
    if (text is String) {
      // Mapping order carries no meaning, so no entry sees another of the same mapping.
      value = before.substitute(text, name);
    } else if (path is String) {
      value = resolvePath(root, path, windows);
    } else {
      final held = facts.secret(entry);
      if (held == null) {
        if (_isTrue(entry, 'optional')) continue;
        throw AosError('unresolved-reference', name);
      }
      value = held;
      secret = true;
      if (entry['provider'] == 'env') {
        nonDefaults.add(_setting('env-passthrough', textOf(entry, 'key')));
      } else {
        nonDefaults.add(_setting('secret-read', _referenceName(entry)));
      }
    }
    if (isSensitiveVariable(name)) {
      nonDefaults.add(_setting('env-sensitive', name));
    }
    env.set(name, value, secret);
  }
}

class _PathGroup {
  _PathGroup(this.variable);

  final String variable;
  final List<Document> entries = [];
}

void _applyPaths(
  Environment env,
  List<Object> paths,
  String root,
  String os,
  List<NonDefaultSetting> nonDefaults,
) {
  final windows = env.windows;
  final separator = windows ? ';' : ':';
  final order = <String>[];
  final grouped = <String, _PathGroup>{};
  for (final entry in paths) {
    if (entry is! Map<String, Object> || !entryOn(entry, os)) continue;
    final variable = textOf(entry, 'variable');
    final key = windows ? asciiUpper(variable) : variable;
    var group = grouped[key];
    if (group == null) {
      order.add(key);
      group = _PathGroup(variable);
      grouped[key] = group;
    }
    group.entries.add(entry);
  }
  for (final key in order) {
    final group = grouped[key]!;
    final variable = group.variable;
    if (isSensitiveVariable(variable) && !isPathVariable(variable)) {
      nonDefaults.add(_setting('env-sensitive', variable));
    }
    final prepend = <String>[];
    final append = <String>[];
    final seen = <String>{};
    for (final entry in group.entries) {
      final packages = entry['packages'];
      final resolved = packages is String
          ? packages
          : resolvePath(root, textOf(entry, 'path'), windows);
      if (seen.contains(resolved)) continue;
      seen.add(resolved);
      if (entry['position'] == 'append') {
        append.add(resolved);
      } else {
        prepend.add(resolved);
      }
    }
    final kept = (env.get(variable) ?? '')
        .split(separator)
        .where((part) => part != '' && !seen.contains(part))
        .toList();
    final secret = env.isSecret(variable);
    env.set(variable, [...prepend, ...kept, ...append].join(separator), secret);
  }
}

/// An env script to run: the interpreter, the file, its arguments and its time in seconds (0 is
/// none).
class ScriptRun {
  /// Creates the script.
  const ScriptRun({
    required this.interpreter,
    required this.path,
    required this.args,
    required this.timeout,
  });

  /// The interpreter that runs it.
  final String interpreter;

  /// The script's path in the overlay.
  final String path;

  /// Its arguments.
  final List<String> args;

  /// Seconds it may run; 0 is no bound.
  final int timeout;
}

/// A setup script to run, and whether it may use the network.
class SetupRun extends ScriptRun {
  /// Creates the setup script.
  const SetupRun({
    required super.interpreter,
    required super.path,
    required super.args,
    required super.timeout,
    required this.network,
  });

  /// Whether it may use the network.
  final bool network;
}

/// Refuses a manifest that would make the consumer touch what a manifest may not name: a
/// variable whose name is not a name, or a path that leaves the overlay. Every other fault is
/// for `validate`.
void _requireApplicable(Document manifest) {
  void check(Object? path) {
    if (path is String && !validRelativePath(path)) {
      throw AosError('invalid-reference', path);
    }
  }

  final env = manifest['env'];
  if (env is Map<String, Object>) {
    for (final name in env.keys) {
      if (!isVariableName(name)) throw AosError('invalid-reference', name);
    }
  }
  for (final field in const ['paths', 'env_files', 'env_scripts', 'setup']) {
    final entries = manifest[field];
    if (entries is List<Object>) {
      for (final entry in entries) {
        check(entry is Map<String, Object> ? entry['path'] : entry);
      }
    }
  }
  final scripts = manifest['scripts'];
  if (scripts is Map<String, Object>) {
    for (final target in scripts.values) {
      check(target is Map<String, Object> ? target['path'] : target);
    }
  }
  final runtime = manifest['runtime'];
  if (runtime is Map<String, Object>) {
    for (final variants in runtime.values) {
      if (variants is! List<Object>) continue;
      for (final variant in variants) {
        if (variant is! Map<String, Object>) continue;
        for (final key in const ['lock', 'requirements', 'package']) {
          check(variant[key]);
        }
      }
    }
  }
}

List<ScriptRun> _applyOverlay(
  Environment env,
  Document manifest,
  Facts facts,
  List<NonDefaultSetting> nonDefaults,
  List<(String, String)> reports,
) {
  _requireApplicable(manifest);
  env.extra = [
    ('AOS_OVERLAY_ROOT', facts.root),
    ('AOS_OVERLAY_NAME', normalizeName(facts.identity)),
  ];
  final state = facts.state;
  if (state != null) env.extra.add(('AOS_OVERLAY_STATE', state));
  try {
    return _applyOverlayInner(env, manifest, facts, nonDefaults, reports);
  } finally {
    env.extra = [];
  }
}

List<Dependency> _dependenciesOrRefuse(
  Document manifest,
  OverlayTree tree,
  String os,
) {
  try {
    return dependenciesOf(manifest, tree, os);
  } on PartialLock catch (error) {
    throw AosError('partial-lock', error.subject);
  }
}

List<ScriptRun> _applyOverlayInner(
  Environment env,
  Document manifest,
  Facts facts,
  List<NonDefaultSetting> nonDefaults,
  List<(String, String)> reports,
) {
  final os = facts.os;
  final root = facts.root;
  final tree = facts.tree;
  final paths = <Object>[...defaultPaths(manifest, tree)];
  for (final dependency in _dependenciesOrRefuse(manifest, tree, os)) {
    final packages = facts.packages[dependency.runtime];
    if (packages != null) {
      final variable = dependency.runtime == 'python'
          ? 'PYTHONPATH'
          : 'NODE_PATH';
      paths.add(<String, Object>{'variable': variable, 'packages': packages});
    }
  }
  paths.addAll(declared(manifest, 'paths'));
  final listed = <String>{};
  for (final entry in paths) {
    final variable = entry is Map<String, Object> ? entry['variable'] : null;
    if (variable is String) listed.add(asciiUpper(variable));
  }
  final ownEnv = _mapOf(manifest['env']) ?? <String, Object>{};
  for (final name in ownEnv.keys) {
    if (listed.contains(asciiUpper(name))) {
      throw AosError('env-path-conflict', name);
    }
  }
  for (final item in envFilesOf(manifest, tree)) {
    if (!entryOn(item, os)) continue;
    final path = named(item) ?? '';
    final dotenv = item is Map<String, Object> && item['format'] == 'dotenv';
    final text = tree.text(path);
    if (text == null) throw AosError('missing-file', path);
    AosError invalid() => AosError('invalid-env-file', path);
    if (dotenv) {
      List<DotenvLine> lines;
      try {
        lines = parseDotenv(encodeUtf8(text));
      } on Object {
        throw invalid();
      }
      for (final line in lines) {
        if (listed.contains(asciiUpper(line.name))) {
          throw AosError('env-path-conflict', line.name);
        }
      }
      for (final line in lines) {
        if (isReservedVariable(line.name)) {
          throw AosError('reserved-variable', line.name);
        }
        final value = line.literal
            ? line.text
            : env.substitute(line.text, line.name);
        if (isSensitiveVariable(line.name)) {
          nonDefaults.add(_setting('env-sensitive', line.name));
        }
        env.set(line.name, value, false);
      }
    } else {
      Document mapping;
      try {
        mapping = readDocument(encodeUtf8(text), facts.settings);
      } on Object {
        throw invalid();
      }
      if (validate(<String, Object>{'env': mapping}).isNotEmpty) {
        throw invalid();
      }
      for (final name in mapping.keys) {
        if (listed.contains(asciiUpper(name))) {
          throw AosError('env-path-conflict', name);
        }
      }
      _applyMapping(env, mapping, facts, root, nonDefaults);
    }
  }
  _applyMapping(env, ownEnv, facts, root, nonDefaults);
  _applyPaths(env, paths, root, os, nonDefaults);
  final scripts = <ScriptRun>[];
  if (facts.envScriptsEnabled != false) {
    final choice = defaultScript(
      manifest,
      'env_scripts',
      'env',
      tree,
      os,
      facts.interpreters ?? interpreters,
    );
    final digest = facts.digest;
    final allowed = facts.allowed;
    if (facts.gateEnvScripts &&
        !(digest != null && allowed != null && sameDigest(digest, allowed))) {
      // Off until the user allows the digest: the overlay is otherwise applied.
      if (choice.chosen != null ||
          choice.present.isNotEmpty ||
          choice.others.any((script) => entryOn(script, os))) {
        reports.add(('env-script-not-allowed', facts.identity));
      }
      return scripts;
    }
    if (choice.chosen == null) {
      for (final path in choice.present) {
        nonDefaults.add(_setting('script-not-run', path));
      }
    }
    final chosen = choice.chosen;
    for (final script in <Object>[?chosen, ...choice.others]) {
      if (!entryOn(script, os)) continue;
      final run = ScriptRun(
        interpreter: textOf(script, 'interpreter'),
        path: textOf(script, 'path'),
        args: _stringsOf(script, 'args'),
        timeout: _unsignedOf(script, 'timeout', 60),
      );
      nonDefaults.add(_setting('env-script', run.path));
      if (run.timeout == 0) nonDefaults.add(_setting('no-timeout', run.path));
      scripts.add(run);
    }
  }
  return scripts;
}

/// Reads `dotenv` text, substituting each value against [env] and what the lines before it set;
/// sets them in [env] and returns them.
List<(String, String)> readDotenv(Uint8List data, Environment env) {
  final set = <(String, String)>[];
  for (final line in parseDotenv(data)) {
    final value = line.literal
        ? line.text
        : env.substitute(line.text, line.name);
    env.set(line.name, value, false);
    set.add((line.name, value));
  }
  return set;
}

/// What running an overlay needs on the machine.
class Needs {
  /// Creates the needs.
  const Needs({required this.interpreters, required this.commands});

  /// Interpreters, sorted.
  final List<String> interpreters;

  /// Commands, sorted.
  final List<String> commands;
}

/// An overlay's effective settings on one operating system.
class Effective {
  /// Creates the result.
  const Effective({
    required this.ungranted,
    required this.supported,
    required this.env,
    required this.envScripts,
    required this.setup,
    required this.python,
    required this.node,
    required this.dependencies,
    required this.needs,
    required this.nonDefaults,
    this.reports = const [],
  });

  /// References the user has not granted, by name.
  final List<String> ungranted;

  /// Whether `runtime.os` admits this system.
  final bool supported;

  /// The environment the overlay builds.
  final Environment env;

  /// The env scripts to run, in order.
  final List<ScriptRun> envScripts;

  /// The setup script, if any.
  final SetupRun? setup;

  /// The Python runtime variant for this system, without its `os`.
  final Document? python;

  /// The Node runtime variant for this system, without its `os`.
  final Document? node;

  /// The dependency files to install.
  final List<Dependency> dependencies;

  /// What must be on the machine.
  final Needs needs;

  /// The non-default settings in use, sorted and without duplicates.
  final List<NonDefaultSetting> nonDefaults;

  /// `(id, subject)` reports, sorted and without duplicates: `env-script-not-allowed`.
  final List<(String, String)> reports;
}

List<String> _sortedUnique(Iterable<String> items) =>
    items.toSet().toList()..sort(compareText);

const Map<String, String> _scriptInterpreters = {
  '.py': 'python',
  '.sh': 'sh',
  '.bash': 'bash',
  '.ps1': 'powershell',
};

/// What [manifest] builds and needs on the system [facts] describes.
Effective effective(Document manifest, Facts facts) {
  final os = facts.os;
  final nonDefaults = <NonDefaultSetting>[];
  if (!facts.pinned) nonDefaults.add(_setting('unpinned', ''));
  final runtime = manifest['runtime'];
  final supported =
      !(runtime is Map<String, Object> && runtime.containsKey('os')) ||
      entryOn(runtime, os);
  final env = Environment([
    for (final e in facts.baseEnv.entries) (e.key, e.value),
  ], facts.windows).settings(facts.settings);
  env.set(rootVariable(facts.identity), facts.root, false);
  final state = facts.state;
  if (state != null) env.set(stateVariable(facts.identity), state, false);
  final reports = <(String, String)>[];
  final scripts = _applyOverlay(env, manifest, facts, nonDefaults, reports);
  final interpretersKnown = facts.interpreters ?? interpreters;
  final choice = defaultScript(
    manifest,
    'setup',
    'setup',
    facts.tree,
    os,
    interpretersKnown,
  );
  // Declared variants come first; the default setup is the last one.
  var network = false;
  Object? variantFound = firstVariant(choice.others, os);
  if (variantFound == null) {
    variantFound = choice.chosen;
    // A default setup that declares nothing is taken to need the network.
    network = !choice.configured;
    if (variantFound == null) {
      for (final path in choice.present) {
        nonDefaults.add(_setting('script-not-run', path));
      }
    }
  }
  final networkSet = variantFound is Map<String, Object>
      ? variantFound['network']
      : null;
  final SetupRun? setup = variantFound == null
      ? null
      : SetupRun(
          interpreter: textOf(variantFound, 'interpreter'),
          path: textOf(variantFound, 'path'),
          args: _stringsOf(variantFound, 'args'),
          timeout: _unsignedOf(variantFound, 'timeout', 300),
          network: networkSet is bool ? networkSet : network,
        );
  if (setup != null) {
    if (setup.network) {
      nonDefaults.add(_setting('setup-network', setup.path));
    } else if (facts.enforcesNetwork == false) {
      nonDefaults.add(_setting('network-not-enforced', setup.path));
    }
    if (setup.timeout == 0) nonDefaults.add(_setting('no-timeout', setup.path));
  }
  Document? variant(String name) {
    final variants = runtime is Map<String, Object>
        ? _listOf(runtime[name])
        : const <Object>[];
    final first = firstVariant(variants, os);
    if (first is! Map<String, Object>) return null;
    final copy = <String, Object>{...first};
    copy.remove('os');
    return copy;
  }

  final python = variant('python');
  final dependencies = _dependenciesOrRefuse(manifest, facts.tree, os);
  for (final dependency in dependencies) {
    // Installing fetches packages: it needs the network, locked or not.
    nonDefaults.add(_setting('deps-network', dependency.file));
    if (dependency.lock == null) {
      nonDefaults.add(_setting('unlocked-dependencies', dependency.file));
    }
  }
  final node = variant('node');
  final needed = <String>{for (final script in scripts) script.interpreter};
  if (setup != null) needed.add(setup.interpreter);
  final declaredScripts = _mapOf(manifest['scripts']);
  if (declaredScripts != null) {
    for (final target in declaredScripts.values) {
      if (target is String) {
        final dot = target.lastIndexOf('.');
        needed.add(
          _scriptInterpreters[dot == -1 ? '' : target.substring(dot)] ?? 'node',
        );
      } else if (target is Map<String, Object> && entryOn(target, os)) {
        needed.add(textOf(target, 'interpreter'));
      }
    }
  }
  if (python != null) needed.add('python');
  if (node != null) needed.add('node');
  final commands = <String>{};
  for (final command
      in runtime is Map<String, Object>
          ? _listOf(runtime['commands'])
          : const <Object>[]) {
    if (command is String) {
      commands.add(command);
    } else if (entryOn(command, os)) {
      commands.add(textOf(command, 'name'));
    }
  }
  for (final dependency in dependencies) {
    needed.add(dependency.runtime);
    if (dependency.runtime == 'node') commands.add('npm');
  }
  final ungranted = <String>{};
  final hostGrants = facts.grants['env'] ?? const <String>[];
  final mcp = _mapOf(manifest['mcp']);
  for (final server
      in mcp == null ? const <Object>[] : _listOf(mcp['servers'])) {
    final serverEnv = server is Map<String, Object>
        ? _mapOf(server['env'])
        : null;
    for (final value in serverEnv?.values ?? const <Object>[]) {
      if (value is! Map<String, Object> || !_isReference(value)) continue;
      if (!facts.granted(value)) {
        ungranted.add(_referenceName(value));
      } else if (value['provider'] == 'env') {
        nonDefaults.add(_setting('env-passthrough', textOf(value, 'key')));
      } else {
        nonDefaults.add(_setting('secret-read', _referenceName(value)));
      }
    }
    final headersEnv = server is Map<String, Object>
        ? _mapOf(server['headers_env'])
        : null;
    final headers = [
      for (final item in headersEnv?.values ?? const <Object>[])
        if (item is String) item,
    ];
    for (final name in [..._stringsOf(server, 'env_passthrough'), ...headers]) {
      if (hostGrants.contains(name)) {
        nonDefaults.add(_setting('env-passthrough', name));
      } else {
        ungranted.add('env:$name');
      }
    }
    final url = textOf(server, 'url');
    if (url.substring(0, url.length < 7 ? url.length : 7).toLowerCase() ==
        'http://') {
      final authority = url.substring(7).split('/').first.split('?').first;
      final colon = authority.lastIndexOf(':');
      final host = asciiLower(
        colon == -1 ? authority : authority.substring(0, colon),
      );
      if (host != 'localhost' && host != '[::1]' && !host.startsWith('127.')) {
        nonDefaults.add(_setting('insecure-transport', textOf(server, 'name')));
      }
    }
  }
  final own = _mapOf(manifest['env']);
  for (final entry in own?.values ?? const <Object>[]) {
    if (entry is Map<String, Object> &&
        _isReference(entry) &&
        _isTrue(entry, 'optional') &&
        !facts.granted(entry)) {
      ungranted.add(_referenceName(entry));
    }
  }
  final unique = <String, NonDefaultSetting>{};
  for (final item in nonDefaults) {
    unique['${item.id}\u0000${item.subject}'] = item;
  }
  final sorted = unique.values.toList()
    ..sort((a, b) {
      final byId = compareText(a.id, b.id);
      return byId != 0 ? byId : compareText(a.subject, b.subject);
    });
  return Effective(
    ungranted: _sortedUnique(ungranted),
    supported: supported,
    env: env,
    envScripts: scripts,
    setup: setup,
    python: python,
    node: node,
    dependencies: dependencies,
    needs: Needs(
      interpreters: _sortedUnique(needed),
      commands: _sortedUnique(commands),
    ),
    nonDefaults: sorted,
    reports: [...reports.toSet()]
      ..sort((a, b) {
        final byId = compareText(a.$1, b.$1);
        return byId != 0 ? byId : compareText(a.$2, b.$2);
      }),
  );
}

/// The environment of several overlays together, or the refusal that stopped it, with what
/// `pre-env` hooks reported.
class ComposedEnvironment {
  /// Creates the result.
  const ComposedEnvironment({
    required this.env,
    required this.error,
    required this.hookReports,
    required this.hookNonDefaults,
  });

  /// The environment, unless a refusal stopped it.
  final Environment? env;

  /// What stopped it; set when [env] is not.
  final AosError? error;

  /// What the `pre-env` hooks reported.
  final List<HookReport> hookReports;

  /// `(id, subject)` of the hooks' non-default settings.
  final List<(String, String)> hookNonDefaults;
}

/// The environment [active] overlays (each manifest with its facts, in composition order) build
/// in turn, on [base]: every overlay's root first, then each `pre-env` hook's variables, then
/// each overlay's own. Non-default settings are appended to [nonDefaults].
ComposedEnvironment composedEnvironment(
  List<(Document, Facts)> active,
  Environment base,
  Map<String, HookRun> preEnv,
  List<NonDefaultSetting> nonDefaults,
) {
  final env = base;
  for (final (_, facts) in active) {
    env.set(rootVariable(facts.identity), facts.root, false);
    final state = facts.state;
    if (state != null) env.set(stateVariable(facts.identity), state, false);
  }
  final hookReports = <HookReport>[];
  final hookNonDefaults = <(String, String)>[];
  ComposedEnvironment refused(AosError error) => ComposedEnvironment(
    env: null,
    error: error,
    hookReports: hookReports,
    hookNonDefaults: hookNonDefaults,
  );
  for (final (_, facts) in active) {
    final given = preEnv[facts.identity];
    if (given == null) continue;
    final run = HookRun(facts.identity, given.stdout)
      ..onFailure = given.onFailure
      ..seconds = given.seconds
      ..notRun = given.notRun
      ..payloadSize = given.payloadSize
      ..timedOut = given.timedOut
      ..exit = given.exit
      ..sets = given.sets;
    final fired = fireEvent(
      'pre-env',
      [run],
      null,
      facts.settings,
      env.windows,
    );
    hookReports.addAll(fired.reports);
    hookNonDefaults.addAll(fired.nonDefaults);
    if (fired.decision == 'refuse') {
      return refused(AosError('hook-refused', facts.identity));
    }
    for (final (name, value) in fired.env) {
      if (value == null) {
        env.unset(name);
      } else {
        env.set(name, value, false);
      }
    }
  }
  for (final (manifest, facts) in active) {
    try {
      _applyOverlay(env, manifest, facts, nonDefaults, []);
    } on AosError catch (error) {
      return refused(error);
    }
  }
  return ComposedEnvironment(
    env: env,
    error: null,
    hookReports: hookReports,
    hookNonDefaults: hookNonDefaults,
  );
}

/// What one earlier hook of an event set; a null value unsets the variable.
typedef EnvSet = List<(String, String?)>;

void _setAll(Environment env, EnvSet set) {
  for (final (name, value) in set) {
    if (value == null) {
      env.unset(name);
    } else {
      env.set(name, value, false);
    }
  }
}

void _ownVariables(Environment env, Facts chosen) {
  env.set('AOS_OVERLAY_ROOT', chosen.root, false);
  env.set('AOS_OVERLAY_NAME', normalizeName(chosen.identity), false);
  final state = chosen.state;
  if (state != null) env.set('AOS_OVERLAY_STATE', state, false);
}

Facts _chosenFacts(List<(Document, Facts)> active, String identity) {
  for (final (_, facts) in active) {
    if (facts.identity == identity) return facts;
  }
  throw AosError('unresolved-reference', identity);
}

/// The session's environment at one point of the order of application: the base, every overlay's
/// root and state variable, the `pre-env` output of the overlays before [until] (all when null),
/// then, past `pre-env`, every overlay's declarations up to and including [through] (all when
/// null) and what the [earlier] hooks set.
Environment _sessionEnvironment(
  String event,
  List<(Document, Facts)> active,
  Environment base,
  Map<String, HookRun> preEnv,
  String? until,
  String? through,
  List<EnvSet> earlier,
) {
  final env = base.clone();
  for (final (_, facts) in active) {
    env.set(rootVariable(facts.identity), facts.root, false);
    final state = facts.state;
    if (state != null) env.set(stateVariable(facts.identity), state, false);
  }
  for (final (_, facts) in active) {
    if (facts.identity == until) break;
    final given = preEnv[facts.identity];
    if (given == null) continue;
    final run = HookRun(facts.identity, given.stdout)
      ..onFailure = given.onFailure
      ..seconds = given.seconds
      ..notRun = given.notRun
      ..payloadSize = given.payloadSize
      ..timedOut = given.timedOut
      ..exit = given.exit
      ..sets = given.sets;
    _setAll(
      env,
      fireEvent('pre-env', [run], null, facts.settings, env.windows).env,
    );
  }
  if (event == 'pre-env') return env;
  for (final (manifest, facts) in active) {
    _applyOverlay(env, manifest, facts, [], []);
    if (facts.identity == through) break;
  }
  for (final set in earlier) {
    _setAll(env, set);
  }
  return env;
}

/// The environment a hook of [event] runs with: the session's.
///
/// [active] and [base] are as [composedEnvironment] takes them. A hook gets the base, every
/// overlay's root (and state) variable, what every overlay's `pre-env` hook set, every overlay's
/// declarations in the order of application, and what the hooks of the event before it set
/// ([earlier], one set for each, in run order). The hook of overlay [identity] also gets its
/// `AOS_OVERLAY_ROOT`, `AOS_OVERLAY_NAME` and `AOS_OVERLAY_STATE` (when it has one); every hook
/// gets `AOS_HOOK_EVENT`. A `pre-env` hook runs before any declaration: it gets the base, the
/// root and state variables and the output of the `pre-env` hooks before it in composition
/// order, which for a user's or a project's own hook (a null [identity]) is every overlay's.
///
/// Throws what an overlay's declarations throw when applied.
Environment hookEnvironment(
  String event,
  List<(Document, Facts)> active,
  Environment base,
  Map<String, HookRun> preEnv,
  String? identity, [
  List<EnvSet> earlier = const [],
]) {
  final own = identity == null ? null : _chosenFacts(active, identity);
  final env = _sessionEnvironment(
    event,
    active,
    base,
    preEnv,
    event == 'pre-env' ? identity : null,
    null,
    earlier,
  );
  if (own != null) _ownVariables(env, own);
  return env..set('AOS_HOOK_EVENT', event, false);
}

/// The processes of an overlay: a word of [overlayProcesses].
const overlayProcesses = Words(['setup', 'server', 'command', 'env-script']);

/// The environment a [process] of the overlay [identity] runs with.
///
/// A `setup`, a `server` or a `command` gets the session's, whole; an `env-script` gets it as it
/// stands when the script runs: every earlier overlay's declarations and its own, no later
/// overlay's. Each also gets the overlay's `AOS_OVERLAY_*` variables. Arguments are as for
/// [hookEnvironment].
Environment processEnvironment(
  String process,
  List<(Document, Facts)> active,
  Environment base,
  Map<String, HookRun> preEnv,
  String identity,
) {
  if (!overlayProcesses.contains(process)) {
    throw ArgumentError.value(process, 'process');
  }
  final own = _chosenFacts(active, identity);
  final env = _sessionEnvironment(
    'session-start',
    active,
    base,
    preEnv,
    null,
    process == 'env-script' ? identity : null,
    [],
  );
  _ownVariables(env, own);
  return env;
}
