/// The closed sets of words the specification and its vectors use.
///
/// Each set is a [Words] value over plain strings, so a word read from a document and a word the
/// library writes are the same thing.
library;

/// A closed set of words: [all], [contains] and [named].
class Words {
  /// Creates the set.
  const Words(this.all);

  /// Every word, in the specification's order.
  final List<String> all;

  /// Whether [word] is one of them.
  bool contains(Object? word) => word is String && all.contains(word);

  /// [word] itself when it is one of them, else null.
  String? named(String word) => all.contains(word) ? word : null;
}

/// The runtimes a dependency or an env script may need.
const runtime = Words(['python', 'node']);

/// What an overlay's state becomes when its preparation fails.
const leftState = Words(['rebuilt', 'deleted']);

/// The two manifest file names.
const manifestFile = Words(['OVERLAY.md', 'overlay.cfg']);

/// The three document formats.
const format = Words(['toml', 'json', 'yaml']);

/// The steps of a preparation that can fail.
const failStep = Words(['deps', 'setup']);

/// The ids of the non-default settings an overlay reports.
const nonDefaultId = Words([
  'deps-network',
  'env-passthrough',
  'env-script',
  'env-sensitive',
  'network-not-enforced',
  'no-timeout',
  'script-not-run',
  'setup-network',
  'unpinned',
  'secret-read',
  'insecure-transport',
  'private-source',
  'hook',
  'unlocked-dependencies',
]);
