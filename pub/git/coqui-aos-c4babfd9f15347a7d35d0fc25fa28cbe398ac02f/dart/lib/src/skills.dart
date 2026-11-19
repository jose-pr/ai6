/// Skills from elsewhere: where a name resolves, what is fetched, and the check of what came
/// back.
library;

import 'dart:async';
import 'dart:typed_data';

import 'catalog.dart';
import 'compose.dart';
import 'defaults.dart';
import 'digest.dart';
import 'error.dart';
import 'front_matter.dart';
import 'kinds.dart';
import 'limits.dart';
import 'names.dart';
import 'source.dart';
import 'text.dart';
import 'tree.dart';
import 'value.dart';

/// What holds a skill's name: a bundled skill of an overlay, or a source and its pin.
class Held {
  /// A bundled skill of the overlay [identity].
  const Held.bundled(String this.identity)
    : key = null,
      pin = null,
      bundled = true;

  /// A skill from a source whose identifying text is [key], at [pin].
  const Held.elsewhere(this.key, this.pin) : identity = null, bundled = false;

  /// Whether this is a bundled skill.
  final bool bundled;

  /// The overlay a bundled skill belongs to.
  final String? identity;

  /// What identifies the source.
  final String? key;

  /// The digest it is pinned to.
  final String? pin;

  /// Whether both hold the same thing.
  bool sameAs(Held other) {
    if (bundled) return other.bundled && identity == other.identity;
    return !other.bundled && key == other.key && pin == other.pin;
  }
}

class _Winner {
  _Winner(this.name, this.identity, this.held);
  final String name;
  final String identity;
  final Held held;
}

/// Which overlay's skill is delivered under each name, folding case; two different holders of one
/// name collide and a bundled skill wins.
class Delivery {
  final Map<String, _Winner> _winners = {};
  final Set<String> _clashes = {};

  /// Offers a skill; false when an earlier holder keeps the name.
  bool offer(String name, String identity, bool bundled, Held held) {
    final folded = name.toLowerCase();
    final earlier = _winners[folded];
    if (earlier != null && !earlier.held.sameAs(held)) {
      _clashes.add(folded);
      if (!bundled) return false;
    }
    _winners[folded] = _Winner(name, identity, held);
    return true;
  }

  /// Name to the identity of the overlay that delivers it.
  Map<String, String> delivered() {
    final entries = [
      for (final item in _winners.values) (item.name, item.identity),
    ]..sort((a, b) => compareText(a.$1, b.$1));
    return {for (final (name, identity) in entries) name: identity};
  }

  /// The folded names two holders offered.
  List<String> collisions() => _clashes.toList()..sort(compareText);

  /// Whether [identity] delivers [name].
  bool holds(String name, String identity) =>
      _winners[name.toLowerCase()]?.identity == identity;
}

/// Where a planned skill comes from.
const plannedOrigin = Words([
  'bundled',
  'source',
  'catalog',
  'missing',
  'ambiguous',
]);

/// One skill of a plan.
class PlannedSkill {
  /// Creates the entry.
  const PlannedSkill({
    required this.name,
    required this.overlay,
    required this.origin,
    required this.source,
    required this.pin,
    required this.catalog,
    required this.catalogs,
  });

  /// The skill's name.
  final String name;

  /// The overlay that asks for it.
  final String overlay;

  /// A word of [plannedOrigin].
  final String origin;

  /// The source as shown: no user information or query.
  final String? source;

  /// The digest it must match.
  final String? pin;

  /// The catalog a `catalog` skill came from.
  final String? catalog;

  /// The catalogs an `ambiguous` name is offered by.
  final List<String>? catalogs;
}

/// A skill the caller must fetch.
class Fetch {
  /// Creates the request.
  const Fetch({
    required this.name,
    required this.overlay,
    required this.from,
    required this.pin,
  });

  /// The skill's name.
  final String name;

  /// The overlay that asks for it.
  final String overlay;

  /// Where to fetch it from.
  final Source from;

  /// The digest it must match.
  final String? pin;
}

/// What [resolveSkills] decided.
class SkillPlan {
  /// Creates an empty plan.
  SkillPlan();

  /// Every skill asked for.
  final List<PlannedSkill> skills = [];

  /// `(name, overlay)` of each catalog skill no catalog offers.
  final List<(String, String)> missing = [];

  /// `(name, overlay, catalogs)`.
  final List<(String, String, List<String>)> ambiguous = [];

  /// `(name, overlay)` of each fetched skill with no pin.
  final List<(String, String)> unpinned = [];

  /// `(id, name, overlay)` such as `insecure-transport` and `private-source`.
  final List<(String, String, String)> nonDefaults = [];

  /// `(name, overlay)`: asked of a consumer that does not fetch.
  final List<(String, String)> notFetched = [];

  /// `(name, overlay)` of each bundled skill whose `SKILL.md` names another skill.
  final List<(String, String)> nameMismatch = [];

  /// `(name, overlay)` of each skill an earlier holder of its name displaced.
  final List<(String, String)> shadowing = [];

  /// Name to the overlay that delivers it.
  Map<String, String> delivered = {};

  /// The folded names two holders offered.
  List<String> collisions = [];

  /// What the caller must fetch.
  final List<Fetch> toFetch = [];
}

String? _bundledName(Uint8List data, Settings settings) {
  final name = readFrontMatter(data, settings).data?['name'];
  if (name == null) return null;
  return name is String ? name : '\u0000${name.runtimeType}';
}

List<(String, T)> _ranked<T>(List<(String, T)> offers, List<String> rank) {
  for (final label in rank) {
    final at = offers.indexWhere((o) => normalizeName(o.$1) == label);
    if (at != -1) return [offers[at]];
  }
  return offers;
}

/// Plans the skills of the overlays [requested] (and what they require): where each comes from,
/// what the caller must fetch, and what could not be placed. [catalogs] are `(label, catalog)`;
/// [order] ranks labels where several offer a name; [fetches] is false for a consumer that does
/// not fetch. Nothing is fetched here.
SkillPlan resolveSkills(
  Map<String, Document> overlays,
  List<String> requested,
  Map<String, OverlayTree> trees,
  List<(String, Catalog)> catalogs,
  List<String> order,
  bool fetches,
  Settings settings,
) {
  final Resolution resolution;
  try {
    resolution = resolve(overlays, requested);
  } on Cycle catch (error) {
    throw AosError('cycle', error.chain.join(' -> '));
  }
  final active = compositionOrder(overlays, resolution.installOrder);
  listCatalogs(catalogs);
  final rank = order.map(normalizeName).toList();
  final empty = OverlayTree.fromPaths([]);
  final plan = SkillPlan();
  final delivery = Delivery();
  for (final identity in active) {
    final tree = trees[identity] ?? empty;
    for (final entry in skillEntries(overlays[identity]!, tree)) {
      var origin = entry.origin == 'bundled'
          ? 'bundled'
          : entry.origin == 'source'
          ? 'source'
          : 'catalog';
      var source = entry.source;
      var pin = entry.pin;
      String? catalog;
      List<String>? catalogLabels;
      var key = entry.key;
      var from = entry.from;
      if (entry.origin == 'catalog') {
        final wanted = entry.name.toLowerCase();
        final offers = <(String, CatalogEntry)>[];
        for (final (label, listed) in catalogs) {
          final skills = listed.skills;
          if (skills == null) continue;
          for (final MapEntry(key: name, value: offer)
              in skills.entries.entries) {
            if (name.toLowerCase() == wanted) offers.add((label, offer));
          }
        }
        final chosen = _ranked(offers, rank);
        if (chosen.isEmpty) {
          origin = 'missing';
          plan.missing.add((entry.name, identity));
        } else if (chosen.length == 1) {
          final (label, offer) = chosen.first;
          catalog = label;
          source = offer.source.shown();
          key = offer.source.keyWith(null);
          from = offer.source;
          pin = offer.pin;
        } else {
          final labels = [for (final (label, _) in chosen) label]
            ..sort(compareText);
          origin = 'ambiguous';
          catalogLabels = labels;
          plan.ambiguous.add((entry.name, identity, labels));
        }
      }
      plan.skills.add(
        PlannedSkill(
          name: entry.name,
          overlay: identity,
          origin: origin,
          source: source,
          pin: pin,
          catalog: catalog,
          catalogs: catalogLabels,
        ),
      );
      if (origin == 'missing' || origin == 'ambiguous') continue;
      final bundled = origin == 'bundled';
      if (bundled) {
        final text = tree.bytes('skills/${entry.name}/SKILL.md');
        if (text != null) {
          final named = _bundledName(text, settings);
          if (named != null && named != entry.name) {
            plan.nameMismatch.add((entry.name, identity));
            continue;
          }
        }
      } else {
        if (!fetches) {
          plan.notFetched.add((entry.name, identity));
          continue;
        }
        if (pin == null) plan.unpinned.add((entry.name, identity));
        final location = (key ?? '').split('#').first;
        var lower = asciiLower(location);
        if (lower.startsWith('git+')) lower = lower.substring(4);
        if (lower.startsWith('http://') || lower.startsWith('git://')) {
          plan.nonDefaults.add(('insecure-transport', entry.name, identity));
        }
        if (hostIsPrivate(sourceHost(location))) {
          plan.nonDefaults.add(('private-source', entry.name, identity));
        }
      }
      final held = bundled ? Held.bundled(identity) : Held.elsewhere(key, pin);
      if (!delivery.offer(entry.name, identity, bundled, held)) {
        plan.shadowing.add((entry.name, identity));
      } else if (!bundled && from != null) {
        plan.toFetch.add(
          Fetch(name: entry.name, overlay: identity, from: from, pin: pin),
        );
      }
    }
  }
  plan.delivered = delivery.delivered();
  plan.collisions = delivery.collisions();
  // A skill later displaced by a bundled one of the same name is not fetched.
  plan.toFetch.removeWhere((item) => !delivery.holds(item.name, item.overlay));
  return plan;
}

/// A fetched skill that passed: its tree digest, the front-matter keys beyond the inert ones, and
/// the files the digest covers.
class FetchedSkill {
  /// Creates the result.
  const FetchedSkill({
    required this.digest,
    required this.reported,
    required this.files,
  });

  /// The tree digest.
  final Digest digest;

  /// The front-matter keys beyond the inert ones, in byte order.
  final List<String> reported;

  /// The paths the digest covers: a caller writes these and nothing else, since the digest leaves
  /// out entries under an excluded name (`node_modules`, `__pycache__`, `*.pyc`, `.git`) that
  /// Python, Node.js and Git load code from.
  final List<String> files;
}

const List<String> _inertKeys = [
  'name',
  'description',
  'license',
  'compatibility',
  'metadata',
];

/// Checks a skill that was fetched, as `(path, bytes)` files: the tree limits first, then the
/// digest against [pin] (so nothing unverified is parsed), then `SKILL.md`'s YAML front matter,
/// whose `name` must be [name].
FetchedSkill readFetchedSkill(
  String name,
  List<(String, Uint8List)> files,
  String? pin,
  Settings settings,
) {
  final entries = <(List<Object>, TreeEntry)>[
    for (final (path, bytes) in files)
      (<Object>[...path.split('/')], FileEntry(bytes)),
  ];
  checkEntries(entries, settings);
  final tree = digestEntries(entries);
  final digest = tree.digest;
  if (pin != null) {
    Digest? pinned;
    try {
      pinned = Digest.parse(pin);
    } on AosError {
      pinned = null;
    }
    if (pinned == null || pinned.toString() != digest.toString()) {
      throw AosError('pin-mismatch', name);
    }
  }
  final skill = files.where((f) => f.$1 == 'SKILL.md').firstOrNull;
  if (skill == null) throw AosError('not-a-skill', name);
  final found = readFrontMatter(skill.$2, settings);
  final error = found.error;
  if (error != null) {
    final refused = AosError('skill-unreadable', error.errorClass);
    final line = error.line;
    if (line != null) refused.onLine(line);
    throw refused;
  }
  final split = splitFrontMatter(skill.$2, settings);
  final metadata = split.metadata;
  if (found.frontMatterCase != 'mapping' ||
      found.format != 'yaml' ||
      (metadata != null && startsWithBytes(metadata, [0x23, 0x21]))) {
    throw AosError('skill-unreadable', 'front matter in YAML is required');
  }
  final data = found.data ?? <String, Object>{};
  if (data['name'] != name) throw AosError('skill-name-mismatch', name);
  final reported = data.keys.where((k) => !_inertKeys.contains(k)).toList()
    ..sort(compareText);
  return FetchedSkill(
    digest: digest,
    reported: reported,
    files: tree.files.keys.toList(),
  );
}

/// The caller's way to get a skill's files, as `(path, bytes)`; fetching is the caller's alone.
/// May be synchronous or return a future.
abstract interface class SkillSource {
  /// The files of the skill at [source].
  FutureOr<List<(String, Uint8List)>> fetch(Source source);
}

/// A planned skill's outcome: the checked skill, or why it was not.
class FetchedPlanned {
  /// Creates the outcome.
  const FetchedPlanned({
    required this.name,
    required this.overlay,
    this.skill,
    this.error,
  });

  /// The skill's name.
  final String name;

  /// The overlay that asked for it.
  final String overlay;

  /// The checked skill, when it passed.
  final FetchedSkill? skill;

  /// Why it did not, otherwise.
  final AosError? error;

  /// Whether it passed.
  bool get ok => skill != null;
}

/// Fetches each skill in `plan.toFetch` once through [fetcher], one after another, and checks it
/// (pin first). A failure of the fetcher or the check is that skill's outcome alone.
Future<List<FetchedPlanned>> fetchPlanned(
  SkillPlan plan,
  SkillSource fetcher,
  Settings settings,
) async {
  final out = <FetchedPlanned>[];
  for (final item in plan.toFetch) {
    try {
      final files = await fetcher.fetch(item.from);
      out.add(
        FetchedPlanned(
          name: item.name,
          overlay: item.overlay,
          skill: readFetchedSkill(item.name, files, item.pin, settings),
        ),
      );
    } on Object catch (error) {
      out.add(
        FetchedPlanned(
          name: item.name,
          overlay: item.overlay,
          error: error is AosError ? error : AosError('io', 'the fetch failed'),
        ),
      );
    }
  }
  return out;
}
