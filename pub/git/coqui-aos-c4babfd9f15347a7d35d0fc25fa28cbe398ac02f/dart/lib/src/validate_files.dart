/// The validation problems that need the overlay's files.
library;

import 'defaults.dart' show bundledSkills, defaultPaths;
import 'names.dart';
import 'tree.dart';
import 'validate_check.dart';
import 'validate_types.dart';
import 'value.dart';

/// Extensions of the scripts a default `hook.<ext>` may have.
const List<String> _hookExtensions = [
  '.py',
  '.js',
  '.mjs',
  '.cjs',
  '.ps1',
  '.bash',
  '.sh',
];

/// Every `(pointer, relative path)` of a file a field requires to exist.
List<(String, String)> namedFiles(Document doc) => namedFilesOf(doc);

/// What a validator flags without refusing: each skill name neither bundled nor given a source,
/// as pointers.
List<String> skillFlags(Document doc, OverlayTree tree) {
  final folded = bundledSkills(tree).map((name) => name.toLowerCase()).toSet();
  final out = <String>[];
  final entries = declared(doc, 'skills');
  for (var index = 0; index < entries.length; index += 1) {
    final entry = entries[index];
    if (entry is String && !folded.contains(entry.toLowerCase())) {
      out.add('/skills/$index');
    }
  }
  return out;
}

/// The problems that need the overlay's files.
void validateFiles(Document doc, OverlayTree tree, Check check) {
  for (final (at, path) in namedFilesOf(doc)) {
    if (!tree.contains(path)) check.fail('missing-file', at);
  }
  final env = doc['env'];
  if (env is Map<String, Object>) {
    final added = <String>{};
    for (final entry in defaultPaths(doc, tree)) {
      final variable = memberText(entry, 'variable');
      if (variable != null) added.add(asciiUpper(variable));
    }
    for (final name in env.keys) {
      if (added.contains(asciiUpper(name))) {
        check.fail('env-path-conflict', '/env/${token(name)}');
      }
    }
  }
  final hooks = doc['hooks'];
  if (hooks is Map<String, Object>) {
    final members = tree.paths().where(validRelativePath).toSet();
    if (!_hookExtensions.any(
      (extension) => members.contains('hook$extension'),
    )) {
      // A table with no run runs hook.<ext>, which has to be there.
      for (final MapEntry(key: event, value: entry) in hooks.entries) {
        if (entry is Map<String, Object> && !entry.containsKey('run')) {
          check.fail('missing-file', '/hooks/${token(event)}');
        }
      }
    }
  }
  final bundled = bundledSkills(tree);
  final folded = bundled.map((name) => name.toLowerCase()).toSet();
  final entries = declared(doc, 'skills');
  for (var index = 0; index < entries.length; index += 1) {
    final entry = entries[index];
    final name = skillName(entry);
    if (name == null || !folded.contains(name.toLowerCase())) continue;
    if (entry is Map<String, Object>) {
      check.fail('skill-ambiguous', skillAt(entry, index));
    } else if (!bundled.contains(name)) {
      check.fail('skill-case', skillAt(entry, index));
    }
  }
  if (tree.contains('OVERLAY.md') && tree.contains('overlay.cfg')) {
    check.fail('two-manifests', '');
  }
  final links = declared(doc['mcp'], 'skills');
  for (var index = 0; index < links.length; index += 1) {
    final name = memberText(links[index], 'name');
    if (name != null && !tree.contains('skills/$name/SKILL.md')) {
      check.fail('missing-skill', '/mcp/skills/$index/name');
    }
  }
}
