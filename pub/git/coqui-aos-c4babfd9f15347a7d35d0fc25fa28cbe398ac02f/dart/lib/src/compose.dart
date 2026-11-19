/// Composition: which overlays a request activates, in what order, and what they contribute
/// together.
library;

import 'dart:convert';

import 'defaults.dart';
import 'error.dart';
import 'names.dart';
import 'skills.dart';
import 'text.dart';
import 'tree.dart';
import 'value.dart';

/// A `requires` chain that returns to an overlay already in it. Thrown by [resolve] and
/// [compose].
class Cycle extends AosError {
  /// A cycle through [chain], the first overlay repeated at the end.
  Cycle(this.chain) : super('cycle', chain.join(' -> '));

  /// The overlays of the cycle, the first repeated at the end.
  final List<String> chain;
}

/// What a request needs, in the order to install it.
class Resolution {
  /// Creates the resolution.
  const Resolution({required this.installOrder, required this.missing});

  /// The overlays to install, dependencies first.
  final List<String> installOrder;

  /// `(required by, name)` for each required overlay that is not available; the first is empty
  /// for a requested one.
  final List<(String, String)> missing;
}

/// Resolves [requested] and everything it `requires`, dependencies first. Throws [Cycle].
Resolution resolve(Map<String, Document> available, List<String> requested) {
  final installOrder = <String>[];
  final missing = <(String, String)>[];
  void visit(String name, List<String> chain) {
    if (installOrder.contains(name)) return;
    final at = chain.indexOf(name);
    if (at != -1) throw Cycle([...chain.sublist(at), name]);
    final manifest = available[name];
    if (manifest == null) {
      missing.add((chain.isEmpty ? '' : chain.last, name));
      return;
    }
    chain.add(name);
    for (final required in declared(manifest, 'requires')) {
      if (required is String) visit(normalizeName(required), chain);
    }
    chain.removeLast();
    installOrder.add(name);
  }

  for (final name in requested) {
    visit(normalizeName(name), []);
  }
  return Resolution(installOrder: installOrder, missing: missing);
}

/// [active] ordered by `priority` (default 500), then name, then identity.
List<String> compositionOrder(
  Map<String, Document> overlays,
  List<String> active,
) {
  (BigInt, String, String) key(String identity) {
    final manifest = overlays[identity];
    final priority = manifest?['priority'];
    final name = manifest?['name'];
    return (
      priority is BigInt ? priority : BigInt.from(500),
      name is String && name != '' ? name : identity,
      identity,
    );
  }

  final keyed = [for (final identity in active) (identity, key(identity))];
  keyed.sort((a, b) {
    final byPriority = a.$2.$1.compareTo(b.$2.$1);
    if (byPriority != 0) return byPriority;
    final byName = compareText(a.$2.$2, b.$2.$2);
    return byName != 0 ? byName : compareText(a.$2.$3, b.$2.$3);
  });
  return [for (final item in keyed) item.$1];
}

/// What a request activates and what the active overlays contribute together.
class Composition {
  /// Creates the composition.
  const Composition({
    required this.installOrder,
    required this.active,
    required this.missing,
    required this.rootVariables,
    required this.routing,
    required this.rules,
    required this.context,
    required this.skills,
    required this.skillCollisions,
  });

  /// The overlays to install, dependencies first.
  final List<String> installOrder;

  /// The active overlays in composition order.
  final List<String> active;

  /// `(required by, name)` of each required overlay that is not available.
  final List<(String, String)> missing;

  /// Identity to its `NAME_OVERLAY_ROOT` variable.
  final Map<String, String> rootVariables;

  /// The `routing` entries of the active overlays, in order.
  final List<String> routing;

  /// `(identity, path)` of each rule file.
  final List<(String, String)> rules;

  /// `(identity, path)` of each context file.
  final List<(String, String)> context;

  /// Skill name to the identity of the overlay that delivers it.
  final Map<String, String> skills;

  /// The folded skill names two different holders offered.
  final List<String> skillCollisions;
}

final OverlayTree _emptyTree = OverlayTree.empty();

/// Composes the overlays [requested] activates. Throws [Cycle].
Composition compose(
  Map<String, Document> overlays,
  List<String> requested,
  Map<String, OverlayTree> trees,
) {
  final resolution = resolve(overlays, requested);
  final active = compositionOrder(overlays, resolution.installOrder);
  final routing = <String>[];
  final rules = <(String, String)>[];
  final context = <(String, String)>[];
  final delivery = Delivery();
  for (final identity in active) {
    final manifest = overlays[identity]!;
    final tree = trees[identity] ?? _emptyTree;
    for (final route in declared(manifest, 'routing')) {
      if (route is String) routing.add(route);
    }
    for (final path in rulesOf(manifest, tree)) {
      rules.add((identity, path));
    }
    for (final path in contextOf(manifest, tree)) {
      context.add((identity, path));
    }
    for (final entry in skillEntries(manifest, tree)) {
      final bundled = entry.origin == 'bundled';
      final Held held = bundled
          ? Held.bundled(identity)
          : Held.elsewhere(entry.key, entry.pin);
      delivery.offer(entry.name, identity, bundled, held);
    }
  }
  return Composition(
    installOrder: resolution.installOrder,
    active: active,
    missing: resolution.missing,
    rootVariables: {
      for (final identity in active) identity: rootVariable(identity),
    },
    routing: routing,
    rules: rules,
    context: context,
    skills: delivery.delivered(),
    skillCollisions: delivery.collisions(),
  );
}

/// One package environment, shared by the overlays that name it, and the files that fill it.
class PackageEnvironment {
  /// Creates the environment.
  const PackageEnvironment({
    required this.runtime,
    required this.environment,
    required this.overlays,
    required this.files,
    required this.locked,
  });

  /// `python` or `node`.
  final String runtime;

  /// The environment's name.
  final String environment;

  /// The overlays that share it.
  final List<String> overlays;

  /// `(identity, file)` of each overlay's dependency file.
  final List<(String, String)> files;

  /// Whether every dependency file is locked.
  final bool locked;
}

/// A package two overlays of one environment pin to different versions.
class PackageConflict {
  /// Creates the conflict.
  const PackageConflict({
    required this.runtime,
    required this.environment,
    required this.package,
    required this.overlays,
  });

  /// `python` or `node`.
  final String runtime;

  /// The environment's name.
  final String environment;

  /// The package's normalized name.
  final String package;

  /// The overlays that pin it.
  final List<String> overlays;
}

/// An environment a removed overlay leaves behind, and what happens to it.
class LeftEnvironment {
  /// Creates the record.
  const LeftEnvironment({
    required this.overlay,
    required this.runtime,
    required this.environment,
    required this.then,
  });

  /// The removed overlay.
  final String overlay;

  /// `python` or `node`.
  final String runtime;

  /// The environment's name.
  final String environment;

  /// `rebuilt` or `deleted`.
  final String then;
}

/// The package environments of the active overlays.
class Environments {
  /// Creates the result.
  const Environments({
    required this.environments,
    required this.conflicts,
    required this.blocked,
    required this.left,
  });

  /// The environments to build.
  final List<PackageEnvironment> environments;

  /// Packages pinned to different versions.
  final List<PackageConflict> conflicts;

  /// Overlays left out because their pins conflict.
  final List<String> blocked;

  /// What becomes of the environments removed overlays used.
  final List<LeftEnvironment> left;
}

final RegExp _spaceOrBracketOrEquals = RegExp(r'^[\s[=]', unicode: true);
final RegExp _versionText = RegExp(r'^[^\s;\\]*', unicode: true);
final RegExp _firstAlnum = RegExp('^[A-Za-z0-9]');
final RegExp _nameChar = RegExp('[A-Za-z0-9._-]');
final RegExp _nameEndsInSeparator = RegExp('[._-]\$');

/// The pinned packages (name to version) a dependency file holds.
Map<String, String> _pins(String runtime, String? text) {
  final out = <String, String>{};
  if (text == null) return out;
  if (runtime == 'node') {
    Object? parsed;
    try {
      parsed = jsonDecode(text);
    } on FormatException {
      return out;
    }
    final dependencies = parsed is Map<String, Object?>
        ? parsed['dependencies']
        : null;
    if (dependencies is Map<String, Object?>) {
      for (final entry in dependencies.entries) {
        final spec = entry.value;
        out[entry.key] = spec is String ? spec : jsonEncode(spec);
      }
    }
    return out;
  }
  final joined = text.replaceAll('\\\r\n', ' ').replaceAll('\\\n', ' ');
  for (final line in joined.split('\n')) {
    final found = _pinnedRequirement(
      (line.endsWith('\r') ? line.substring(0, line.length - 1) : line).trim(),
    );
    if (found != null) out[found.$1] = found.$2;
  }
  return out;
}

/// `name==version` (extras allowed) as the normalized name and the version.
(String, String)? _pinnedRequirement(String line) {
  if (!_firstAlnum.hasMatch(line)) return null;
  var end = 0;
  while (end < line.length && _nameChar.hasMatch(line[end])) {
    end += 1;
  }
  var name = line.substring(0, end);
  while (_nameEndsInSeparator.hasMatch(name)) {
    name = name.substring(0, name.length - 1);
  }
  final after = line.substring(name.length);
  if (!_spaceOrBracketOrEquals.hasMatch(after)) return null;
  var rest = after.trimLeft();
  if (rest.startsWith('[')) {
    final close = rest.indexOf(']');
    if (close == -1) return null;
    rest = rest.substring(close + 1).trimLeft();
  }
  if (rest.startsWith('===')) {
    rest = rest.substring(3);
  } else if (rest.startsWith('==')) {
    rest = rest.substring(2);
  } else {
    return null;
  }
  rest = rest.trimLeft();
  final version = _versionText.firstMatch(rest)![0]!;
  if (version == '') return null;
  final normalized = StringBuffer();
  var last = '';
  for (final rune in name.runes) {
    final c = String.fromCharCode(rune);
    if (c == '-' || c == '_' || c == '.') {
      if (!last.endsWith('-')) {
        normalized.write('-');
        last = '-';
      }
    } else {
      final lower = asciiLower(c);
      normalized.write(lower);
      last = lower;
    }
  }
  return (normalized.toString(), version);
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

class _Group {
  _Group(this.runtime, this.environment, this.owner);

  final String runtime;
  final String environment;
  final String owner;
  final List<(String, Dependency)> members = [];
}

/// Groups the active overlays' dependencies into package environments: an `isolated` one per
/// overlay, any other shared by name. Overlays that pin one package differently are blocked.
/// [removed] overlays say what becomes of their environments. Throws `partial-lock`.
Environments environments(
  Map<String, Document> overlays,
  List<String> active,
  Map<String, OverlayTree> trees,
  String os,
  List<String> removed,
) {
  OverlayTree treeOf(String identity) => trees[identity] ?? _emptyTree;
  String keyOf(String runtime, String environment, String identity) =>
      '$runtime\u0000$environment\u0000${environment == 'isolated' ? identity : ''}';
  final groups = <String, _Group>{};
  for (final identity in active) {
    final manifest = overlays[identity];
    if (manifest == null) continue;
    for (final dependency in _dependenciesOrRefuse(
      manifest,
      treeOf(identity),
      os,
    )) {
      final key = keyOf(dependency.runtime, dependency.environment, identity);
      final group = groups.putIfAbsent(
        key,
        () => _Group(dependency.runtime, dependency.environment, identity),
      );
      group.members.add((identity, dependency));
    }
  }
  final conflicts = <PackageConflict>[];
  final blocked = <String>{};
  for (final group in groups.values) {
    final asked = <String, List<(String, List<String>)>>{};
    for (final (identity, dependency) in group.members) {
      final pins = _pins(group.runtime, treeOf(identity).text(dependency.file));
      for (final MapEntry(key: pkg, value: version) in pins.entries) {
        final versions = asked.putIfAbsent(pkg, () => []);
        final existing = versions.where((v) => v.$1 == version);
        if (existing.isEmpty) {
          versions.add((version, [identity]));
        } else {
          existing.first.$2.add(identity);
        }
      }
    }
    final packages = asked.keys.toList()..sort(compareText);
    for (final pkg in packages) {
      final versions = asked[pkg]!;
      if (versions.length > 1) {
        final involved = [
          for (final (identity, _) in group.members)
            if (versions.any((v) => v.$2.contains(identity))) identity,
        ];
        blocked.addAll(involved);
        conflicts.add(
          PackageConflict(
            runtime: group.runtime,
            environment: group.environment,
            package: pkg,
            overlays: involved,
          ),
        );
      }
    }
  }
  final remaining = <String>{};
  final built = <PackageEnvironment>[];
  for (final MapEntry(key: key, value: group) in groups.entries) {
    final kept = [
      for (final member in group.members)
        if (!blocked.contains(member.$1)) member,
    ];
    if (kept.isEmpty) continue;
    remaining.add(key);
    // npm installs one lock file; several overlays' locks cannot be combined.
    final locked = group.runtime == 'node'
        ? kept.length == 1 && kept.first.$2.lock != null
        : kept.every((member) => member.$2.lock != null);
    built.add(
      PackageEnvironment(
        runtime: group.runtime,
        environment: group.environment,
        overlays: [for (final member in kept) member.$1],
        files: [for (final member in kept) (member.$1, member.$2.file)],
        locked: locked,
      ),
    );
  }
  final left = <LeftEnvironment>[];
  for (final identity in removed) {
    final manifest = overlays[identity];
    if (manifest == null) continue;
    for (final dependency in _dependenciesOrRefuse(
      manifest,
      treeOf(identity),
      os,
    )) {
      final key = keyOf(dependency.runtime, dependency.environment, identity);
      left.add(
        LeftEnvironment(
          overlay: identity,
          runtime: dependency.runtime,
          environment: dependency.environment,
          then: remaining.contains(key) ? 'rebuilt' : 'deleted',
        ),
      );
    }
  }
  return Environments(
    environments: built,
    conflicts: conflicts,
    blocked: [
      for (final identity in active)
        if (blocked.contains(identity)) identity,
    ],
    left: left,
  );
}

/// The order the hooks of one event run in, as `(scope, source)` pairs.
///
/// Every active overlay that has a hook ([has]) in composition order, then the user's own, then
/// the project's, whichever order [scopes] (the scopes that have one, `user` or `project`) names
/// them in; a scope's own hook has the source `''`. Throws [Cycle] when `requires` loops.
List<(String, String)> hookOrder(
  Map<String, Document> overlays,
  List<String> requested,
  List<String> has,
  List<String> scopes,
) {
  final active = compositionOrder(
    overlays,
    resolve(overlays, requested).installOrder,
  );
  return [
    for (final identity in active)
      if (has.contains(identity)) ('overlay', identity),
    for (final scope in const ['user', 'project'])
      if (scopes.contains(scope)) (scope, ''),
  ];
}
