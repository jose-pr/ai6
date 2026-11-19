/// What an overlay's files contribute by default, merged with what its manifest declares.
library;

import 'error.dart';
import 'front_matter.dart';
import 'kinds.dart' as kinds;
import 'limits.dart';
import 'names.dart';
import 'source.dart';
import 'text.dart';
import 'tree.dart';
import 'value.dart';

/// The `defaults` keys a manifest may turn off.
const List<String> defaultKeys = [
  'bin',
  'lib',
  'env',
  'env_scripts',
  'setup',
  'deps',
  'context',
  'skills',
  'rules',
  'hooks',
];

const List<(String, String, String)> _defaultPaths = [
  ('bin', 'PATH', 'bin'),
  ('lib', 'PYTHONPATH', 'lib/python'),
  ('lib', 'NODE_PATH', 'lib/node'),
];

/// A script's file extension, the interpreter that runs it, and the operating systems it is for.
const List<(String, String, String?)> defaultScripts = [
  ('.py', 'python', null),
  ('.js', 'node', null),
  ('.mjs', 'node', null),
  ('.cjs', 'node', null),
  ('.ps1', 'powershell', 'windows'),
  ('.bash', 'bash', 'unix'),
  ('.sh', 'sh', 'unix'),
];

/// The interpreters a manifest may name.
const List<String> interpreters = [
  'python',
  'node',
  'powershell',
  'bash',
  'sh',
];

/// The mapping inside a value, if it is one.
Document? asMap(Object? value) => value is Map<String, Object> ? value : null;

/// The text of a value, if it is text.
String? asText(Object? value) => value is String ? value : null;

/// The list inside a value, if it is one.
List<Object>? asList(Object? value) => value is List<Object> ? value : null;

/// A mapping from pairs.
Document mapOf(Iterable<(String, Object)> entries) => {
  for (final (key, value) in entries) key: value,
};

/// Whether a default is on: unless `defaults.<key>` is `false`.
bool defaultOn(Document manifest, String key) =>
    asMap(manifest['defaults'])?[key] != false;

/// The entries a manifest declares under [field], or none.
List<Object> declared(Document manifest, String field) =>
    asList(manifest[field]) ?? const [];

/// The path an entry names: the text itself, or a mapping's `path`.
String? named(Object entry) {
  if (entry is String) return entry;
  return entry is Map<String, Object> ? asText(entry['path']) : null;
}

List<Object> _merge(List<Object> defaults, List<Object> given) {
  final names = {for (final entry in given) ?named(entry)};
  return [
    ...defaults.where((entry) {
      final name = named(entry);
      return name == null || !names.contains(name);
    }),
    ...given,
  ];
}

/// An entry's `os` list, if it has one.
List<String>? osList(Object entry) {
  final names = entry is Map<String, Object> ? asList(entry['os']) : null;
  return names?.whereType<String>().toList();
}

/// Whether an entry applies on [os].
bool entryOn(Object entry, String os) => osMatches(osList(entry), os);

/// The first variant that applies on [os].
Object? firstVariant(List<Object> variants, String os) {
  for (final variant in variants) {
    if (entryOn(variant, os)) return variant;
  }
  return null;
}

/// The paths added to a variable by default: `bin`, `lib/python` and `lib/node` that the tree
/// holds and the manifest does not name.
List<Document> defaultPaths(Document manifest, OverlayTree tree) {
  final names = {
    for (final entry in declared(manifest, 'paths')) ?named(entry),
  };
  final members = tree.members();
  return [
    for (final (key, variable, directory) in _defaultPaths)
      if (defaultOn(manifest, key) &&
          !names.contains(directory) &&
          members.any((path) => path.startsWith('$directory/')))
        mapOf([('variable', variable), ('path', directory)]),
  ];
}

/// The default paths, then the declared.
List<Object> pathsOf(Document manifest, OverlayTree tree) => [
  ...defaultPaths(manifest, tree),
  ...declared(manifest, 'paths'),
];

/// The rule files: `rules/*.md` by default, then the declared, a declared path replacing a
/// default of its name.
List<String> rulesOf(Document manifest, OverlayTree tree) {
  List<Object> found = [];
  if (defaultOn(manifest, 'rules')) {
    found = tree.members().where((path) {
      if (!path.startsWith('rules/')) return false;
      final name = path.substring('rules/'.length);
      return !name.contains('/') && name.endsWith('.md');
    }).toList();
  }
  return _strings(_merge(found, declared(manifest, 'rules')));
}

bool _hasBody(OverlayTree tree) {
  final data = tree.bytes('OVERLAY.md');
  if (data == null) return false;
  final split = splitFrontMatter(data, Settings({'manifestBytes': null}));
  return split
      .body(data)
      .any(
        (byte) => byte != 0x20 && byte != 0x09 && byte != 0x0d && byte != 0x0a,
      );
}

bool _bodyIsContext(Document manifest, OverlayTree tree) =>
    _hasBody(tree) &&
    (defaultOn(manifest, 'context') ||
        declared(manifest, 'context').any((entry) => entry == 'OVERLAY.md'));

/// The context files: `OVERLAY.md`'s body, else `CONTEXT.md`, then the declared.
List<String> contextOf(Document manifest, OverlayTree tree) {
  List<Object> found = [];
  if (defaultOn(manifest, 'context')) {
    if (_hasBody(tree)) {
      found = ['OVERLAY.md'];
    } else if (tree.hasMember('CONTEXT.md')) {
      found = ['CONTEXT.md'];
    }
  }
  final merged = _strings(_merge(found, declared(manifest, 'context')));
  return _bodyIsContext(manifest, tree)
      ? merged.where((entry) => entry != 'CONTEXT.md').toList()
      : merged;
}

/// Context files the overlay carries and does not use: `CONTEXT.md` beside a body that is the
/// context.
List<String> contextIgnored(Document manifest, OverlayTree tree) =>
    _bodyIsContext(manifest, tree) && tree.hasMember('CONTEXT.md')
    ? ['CONTEXT.md']
    : [];

/// The context files a harness's hints list.
List<String> harnessContextOf(
  Document manifest,
  String harness,
  OverlayTree tree,
) {
  final listed = _strings(
    asList(asMap(asMap(manifest['harness'])?[harness])?['context']) ?? const [],
  );
  return _bodyIsContext(manifest, tree)
      ? listed.where((entry) => entry != 'CONTEXT.md').toList()
      : listed;
}

List<String> _strings(List<Object> entries) =>
    entries.whereType<String>().toList();

/// A skill a manifest asks for. [origin] is `bundled`, `source` or `catalog`.
class SkillEntry {
  /// Creates the entry.
  const SkillEntry({
    required this.name,
    required this.origin,
    this.source,
    this.key,
    this.pin,
    this.from,
  });

  /// The skill's name.
  final String name;

  /// Where it comes from: `bundled`, `source` or `catalog`.
  final String origin;

  /// The source as shown: no user information.
  final String? source;

  /// The source with everything that identifies it; never shown.
  final String? key;

  /// The digest the skill must match.
  final String? pin;

  /// The parsed source.
  final Source? from;
}

/// The skills the overlay bundles: `skills/<name>/SKILL.md`.
Set<String> bundledSkills(OverlayTree tree) {
  final found = <String>{};
  for (final path in tree.members()) {
    final parts = path.split('/');
    if (parts.length == 3 &&
        parts[0] == 'skills' &&
        parts[2] == 'SKILL.md' &&
        validToolName(parts[1])) {
      found.add(parts[1]);
    }
  }
  return found;
}

/// The skills an overlay asks for, by name in byte order: bundled, from a source, or from a
/// catalog.
List<SkillEntry> skillEntries(Document manifest, OverlayTree tree) {
  final bundled = bundledSkills(tree);
  final found = <String, SkillEntry>{};
  SkillEntry plain(String name, String origin) =>
      SkillEntry(name: name, origin: origin);
  if (defaultOn(manifest, 'skills')) {
    for (final name in bundled) {
      found[name] = plain(name, 'bundled');
    }
  }
  for (final entry in declared(manifest, 'skills')) {
    if (entry is Map<String, Object>) {
      final name = asText(entry['name']);
      final source = asText(entry['source']);
      if (name == null || source == null) continue;
      final Source parsed;
      try {
        parsed = Source.parse(source);
      } on AosError {
        continue;
      }
      final reference = asText(entry['ref']);
      found[name] = SkillEntry(
        name: name,
        origin: 'source',
        source: parsed.shownWith(reference ?? parsed.reference),
        key: parsed.keyWith(reference),
        pin: asText(entry['pin']),
        from: reference == null ? parsed : parsed.withReference(reference),
      );
    } else if (entry is String) {
      found[entry] = plain(
        entry,
        bundled.contains(entry) ? 'bundled' : 'catalog',
      );
    }
  }
  final names = found.keys.toList()..sort(compareText);
  return [for (final name in names) found[name]!];
}

/// The skill names [skillEntries] lists.
List<String> skillsOf(Document manifest, OverlayTree tree) =>
    skillEntries(manifest, tree).map((entry) => entry.name).toList();

/// The environment files: `env` by default, then the declared.
List<Object> envFilesOf(Document manifest, OverlayTree tree) {
  final found = <Object>[];
  if (defaultOn(manifest, 'env') && tree.hasMember('env')) {
    found.add(mapOf([('path', 'env'), ('format', 'dotenv')]));
  }
  return _merge(found, declared(manifest, 'env_files'));
}

/// The default script of a field, which interpreter runs it, and what was declared besides.
class DefaultScript {
  /// Creates the choice.
  const DefaultScript({
    this.chosen,
    required this.configured,
    required this.present,
    required this.others,
  });

  /// The entry that runs, if any.
  final Document? chosen;

  /// Whether the manifest configured the chosen script.
  final bool configured;

  /// The default script paths the tree holds.
  final List<String> present;

  /// The declared entries that configure no default script.
  final List<Object> others;
}

/// Chooses `<stem>.<ext>`: the first script present that applies on [os] and whose interpreter
/// the consumer has.
DefaultScript defaultScript(
  Document manifest,
  String field,
  String stem,
  OverlayTree tree,
  String os,
  List<String> available,
) {
  final given = declared(manifest, field);
  if (!defaultOn(manifest, field)) {
    return DefaultScript(configured: false, present: const [], others: given);
  }
  final present = [
    for (final (extension, _, _) in defaultScripts)
      if (tree.hasMember('$stem$extension')) '$stem$extension',
  ];
  final configured = <String, Document>{};
  for (final entry in given) {
    if (entry is! Map<String, Object>) continue;
    final path = named(entry);
    if (path != null && present.contains(path)) configured[path] = entry;
  }
  final others = given.where((entry) {
    final path = named(entry);
    return path == null || !configured.containsKey(path);
  }).toList();
  for (final (extension, interpreter, on) in defaultScripts) {
    final path = '$stem$extension';
    if (!present.contains(path)) continue;
    var entry = configured[path];
    if (entry == null) {
      entry = mapOf([('interpreter', interpreter), ('path', path)]);
      if (on != null) entry['os'] = <Object>[on];
    }
    final name = asText(entry['interpreter']);
    final runs = name != null && available.contains(name);
    if (entryOn(entry, os) && runs) {
      return DefaultScript(
        configured: configured.containsKey(path),
        chosen: entry,
        present: present,
        others: others,
      );
    }
  }
  return DefaultScript(configured: false, present: present, others: others);
}

/// A variant's `environment` name, normalized, or `shared`.
String environmentName(Object? variant) {
  final name = variant is Map<String, Object>
      ? asText(variant['environment'])
      : null;
  return normalizeName(name ?? 'shared');
}

/// A dependency file an overlay brings, and the lock beside it.
class Dependency {
  /// Creates the dependency.
  const Dependency({
    required this.runtime,
    required this.file,
    this.lock,
    required this.environment,
  });

  /// `python` or `node`.
  final String runtime;

  /// The dependency file's path.
  final String file;

  /// The lock beside it, if any.
  final String? lock;

  /// The environment's normalized name.
  final String environment;
}

/// A requirements file that pins some requirements by hash and not others. Its text is the
/// file's path.
class PartialLock extends AosError {
  /// Refuses [subject], the file's path.
  PartialLock(String subject) : super('partial-lock', subject);
}

final RegExp _endsInSpace = RegExp(r'\s$', unicode: true);
final RegExp _space = RegExp(r'\s', unicode: true);

/// Whether every requirement carries a hash; a file with some and not all is a [PartialLock].
bool requirementsLocked(String text, String path) {
  final joined = text.replaceAll('\\\r\n', ' ').replaceAll('\\\n', ' ');
  final hashed = splitLines(joined)
      .map(trimWhitespace)
      .where((line) => line != '' && !line.startsWith('#'))
      .map((line) {
        for (
          var at = line.indexOf('--hash');
          at != -1;
          at = line.indexOf('--hash', at + 1)
        ) {
          final before =
              at == 0 || _endsInSpace.hasMatch(line.substring(0, at));
          final nextAt = at + '--hash'.length;
          final after =
              nextAt < line.length &&
              (line[nextAt] == '=' || _space.hasMatch(line[nextAt]));
          if (before && after) return true;
        }
        return false;
      })
      .toList();
  if (hashed.isNotEmpty && hashed.every((h) => h)) return true;
  if (hashed.any((h) => h)) throw PartialLock(path);
  return false;
}

/// Splits text into lines as Rust's `str::lines` does: on `\n`, dropping a `\r` before it.
List<String> splitLines(String text) {
  if (text == '') return [];
  final lines = text.split('\n');
  if (lines.last == '') lines.removeLast();
  return [
    for (final line in lines)
      line.endsWith('\r') ? line.substring(0, line.length - 1) : line,
  ];
}

final RegExp _leadingDashes = RegExp(r'^-+');

/// Checks a requirements file reads as requirements only: no option but `--hash`, no direct URL,
/// and every one pinned with `==` when [locked].
void checkRequirements(String text, bool locked) {
  final joined = text.replaceAll('\\\r\n', ' ').replaceAll('\\\n', ' ');
  final lines = splitLines(joined);
  for (var number = 0; number < lines.length; number += 1) {
    AosError refused(String why) =>
        AosError('invalid-requirements', why).onLine(number + 1);
    final line = trimWhitespace(lines[number].split(' #').first);
    if (line == '' || line.startsWith('#')) continue;
    final cut = line.indexOf(' --');
    final requirement = cut == -1 ? line : line.substring(0, cut);
    final options = cut == -1 ? '' : line.substring(cut + 3);
    if (requirement.startsWith('-')) {
      throw refused('an option, and only requirements are read');
    }
    if (requirement.contains('://') || requirement.contains(' @ ')) {
      throw refused('a direct URL');
    }
    if (options != '' &&
        options
            .split(' --')
            .any(
              (o) => !o.replaceFirst(_leadingDashes, '').startsWith('hash'),
            )) {
      throw refused('an option other than --hash');
    }
    if (locked && !requirement.contains('==')) {
      throw refused('a requirement not pinned with ==');
    }
  }
}

/// The dependency files an overlay brings on [os]: Python's requirements and Node's package, each
/// with its lock. Throws [PartialLock].
List<Dependency> dependenciesOf(
  Document manifest,
  OverlayTree tree,
  String os,
) {
  final runtimeTable = asMap(manifest['runtime']);
  List<Object> variants(String name) => asList(runtimeTable?[name]) ?? const [];
  final found = <Dependency>[];
  for (final (name, byDefault, beside) in const [
    ('python', 'lib/python/requirements.txt', null),
    ('node', 'lib/node/package.json', 'lib/node/package-lock.json'),
  ]) {
    final variant = firstVariant(variants(name), os);
    String? field(String key) =>
        variant is Map<String, Object> ? asText(variant[key]) : null;
    final String file;
    String? lock;
    if (name == 'python' &&
        (field('lock') != null || field('requirements') != null)) {
      file = field('lock') ?? field('requirements') ?? '';
      lock = field('lock');
    } else if (name == 'node' && field('package') != null) {
      file = field('package') ?? '';
      final candidate = file.endsWith('package.json')
          ? '${file.substring(0, file.length - 'package.json'.length)}package-lock.json'
          : null;
      lock = candidate != null && tree.hasMember(candidate) ? candidate : null;
    } else if (defaultOn(manifest, 'deps') && tree.hasMember(byDefault)) {
      if (beside == null) {
        final text = tree.text(byDefault);
        lock = text != null && requirementsLocked(text, byDefault)
            ? byDefault
            : null;
      } else {
        lock = tree.hasMember(beside) ? beside : null;
      }
      file = byDefault;
    } else {
      continue;
    }
    found.add(
      Dependency(
        runtime: kinds.runtime.named(name) ?? 'python',
        file: file,
        lock: lock,
        environment: environmentName(variant),
      ),
    );
  }
  return found;
}
