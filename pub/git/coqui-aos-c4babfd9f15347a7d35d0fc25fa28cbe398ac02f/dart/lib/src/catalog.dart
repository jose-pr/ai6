/// Catalogs of overlays and skills, listing several, and icons judged from their headers.
library;

import 'dart:typed_data';

import 'error.dart';
import 'kinds.dart';
import 'limits.dart';
import 'names.dart';
import 'read.dart';
import 'source.dart';
import 'text.dart';
import 'validate.dart';
import 'value.dart';

/// Why an entry of a catalog is refused.
const entryRefusal = Words([
  'invalid-name',
  'duplicate-name',
  'invalid-entry',
  'invalid-source',
  'invalid-pin',
  'ref-not-allowed',
  'leaves-repository',
  'local-source',
  'source-credentials',
]);

/// `minimal` when every entry is a source string, else `full`.
const catalogForm = Words(['minimal', 'full']);

/// The formats an icon may have.
const iconFormat = Words(['png', 'webp']);

/// One entry of a catalog, its source resolved beside the catalog. [description] is a stranger's
/// text: never render it as markup.
class CatalogEntry {
  /// Creates an entry.
  const CatalogEntry({
    required this.source,
    required this.pin,
    required this.description,
    required this.about,
  });

  /// Where the entry comes from.
  final Source source;

  /// The digest the entry is pinned to.
  final String? pin;

  /// A stranger's description.
  final String? description;

  /// The `about` fields that are valid.
  final Document? about;
}

/// The entries of one table by name, and the names refused with the reason.
class CatalogSection {
  /// Creates an empty section.
  CatalogSection();

  /// The entries read.
  final Map<String, CatalogEntry> entries = {};

  /// `(name, reason)`, a reason being a word of [entryRefusal].
  final List<(String, String)> refused = [];
}

/// A catalog read.
class Catalog {
  /// Creates a catalog.
  const Catalog({
    required this.form,
    required this.name,
    required this.description,
    required this.homepage,
    required this.overlays,
    required this.skills,
    required this.ignored,
  });

  /// A word of [catalogForm].
  final String form;

  /// The catalog's name.
  final String? name;

  /// The catalog's description.
  final String? description;

  /// The catalog's homepage.
  final String? homepage;

  /// The overlays table.
  final CatalogSection overlays;

  /// The skills table, if the catalog has one.
  final CatalogSection? skills;

  /// JSON pointers of fields read and dropped.
  final List<String> ignored;
}

/// A catalog file's format from its name's extension; null for any other.
String? catalogFormat(String name) {
  final last = name.replaceAll(r'\', '/').split('/').last;
  final dot = last.lastIndexOf('.');
  if (dot == -1) return null;
  switch (asciiLower(last.substring(dot + 1))) {
    case 'toml':
      return 'toml';
    case 'json':
      return 'json';
    case 'yaml':
    case 'yml':
      return 'yaml';
    default:
      return null;
  }
}

/// Reads a catalog file's document: its name's extension picks the format; any other name reads
/// as an AOS document.
Document readCatalogDocument(Uint8List data, String name, Settings settings) {
  final bounded = settings.copyWith({'manifestBytes': settings.catalogBytes});
  final format = catalogFormat(name);
  if (format == null) {
    try {
      return readDocument(data, bounded);
    } on AosError catch (error) {
      if (error.setting == 'manifest_bytes') error.over('catalog_bytes');
      rethrow;
    }
  }
  if (!allows(bounded.manifestBytes, data.length)) {
    throw AosError('too-large', 'a catalog').over('catalog_bytes');
  }
  final bytes = startsWithBytes(data, [0xef, 0xbb, 0xbf])
      ? Uint8List.sublistView(data, 3)
      : data;
  return parsePayload(bytes, format, bounded, false);
}

String _escapeToken(String token) =>
    token.replaceAll('~', '~0').replaceAll('/', '~1');

final RegExp _drive = RegExp(r'^[A-Za-z]:');
final RegExp _scheme = RegExp(r'^[A-Za-z][A-Za-z0-9+.-]+:');

bool _hasDrive(String location) => _drive.hasMatch(location);
bool _hasScheme(String location) => _scheme.hasMatch(location);
bool _absoluteDirectory(String location) =>
    location.startsWith('/') ||
    location.startsWith(r'\') ||
    _hasDrive(location);

String _normpath(String path) {
  final rooted = path.startsWith('/');
  final parts = <String>[];
  for (final part in path.split('/')) {
    if (part == '' || part == '.') continue;
    if (part == '..') {
      if (parts.isNotEmpty && parts.last != '..') {
        parts.removeLast();
      } else if (!rooted) {
        parts.add('..');
      }
    } else {
      parts.add(part);
    }
  }
  final joined = parts.join('/');
  if (rooted) return '/$joined';
  return joined == '' ? '.' : joined;
}

String _join(List<String> parts) {
  var out = '';
  for (final part in parts.where((item) => item != '')) {
    if (part.startsWith('/')) {
      out = part;
    } else if (out == '' || out.endsWith('/')) {
      out += part;
    } else {
      out += '/$part';
    }
  }
  return _normpath(out == '' ? '.' : out);
}

final RegExp _notSlash = RegExp('[^/]');
final RegExp _trailingSlashes = RegExp(r'/+$');

String _dirname(String path) {
  final head = path.substring(0, path.lastIndexOf('/') + 1);
  return head != '' && _notSlash.hasMatch(head)
      ? head.replaceFirst(_trailingSlashes, '')
      : head;
}

String _removeDots(String path) {
  final out = <String>[];
  final segments = path.split('/');
  final last = segments.length - 1;
  for (var index = 0; index < segments.length; index += 1) {
    final segment = segments[index];
    if (segment == '.') {
      if (index == last) out.add('');
    } else if (segment == '..') {
      if (out.length > 1) out.removeLast();
      if (index == last) out.add('');
    } else {
      out.add(segment);
    }
  }
  final joined = out.join('/');
  return joined.startsWith('/') ? joined : '/$joined';
}

int _indexOfAny(String text, String chars) {
  for (var at = 0; at < text.length; at += 1) {
    if (chars.contains(text[at])) return at;
  }
  return -1;
}

String _urlJoin(String base, String reference) {
  if (reference == '') return base;
  if (_hasScheme(reference)) return reference;
  final split = base.indexOf('://');
  if (split == -1) return reference;
  final scheme = base.substring(0, split);
  final rest = base.substring(split + 3);
  final cut = _indexOfAny(rest, '/?#');
  final authority = cut == -1 ? rest : rest.substring(0, cut);
  final tail = cut == -1 ? '' : rest.substring(cut);
  if (reference.startsWith('//')) return '$scheme:$reference';
  final withoutFragment = tail.split('#').first;
  final question = withoutFragment.indexOf('?');
  final basePath = question == -1
      ? withoutFragment
      : withoutFragment.substring(0, question);
  final baseQuery = question == -1
      ? ''
      : withoutFragment.substring(question + 1);
  final suffixAt = _indexOfAny(reference, '?#');
  final refPath = suffixAt == -1 ? reference : reference.substring(0, suffixAt);
  final refSuffix = suffixAt == -1 ? '' : reference.substring(suffixAt);
  if (refPath == '') {
    var path = basePath;
    if (refSuffix.startsWith('#') && baseQuery != '') path += '?$baseQuery';
    return '$scheme://$authority$path$refSuffix';
  }
  String path;
  if (refPath.startsWith('/')) {
    path = _removeDots(refPath);
  } else {
    var head = basePath.substring(0, basePath.lastIndexOf('/') + 1);
    if (head == '' && authority != '') head = '/';
    path = _removeDots('$head$refPath');
  }
  return '$scheme://$authority$path$refSuffix';
}

/// A copy of [source] with fields changed; [setPath] and [setReference] say a null is meant.
Source _changed(
  Source source, {
  String? location,
  String? kind,
  bool setPath = false,
  String? path,
  bool setReference = false,
  String? reference,
}) => Source(
  kind ?? source.kind,
  location ?? source.location,
  setReference ? reference : source.reference,
  setPath ? path : source.path,
);

/// An entry's refusal, thrown inside this library and turned into the name's reason.
class _Refused implements Exception {
  const _Refused(this.reason);
  final String reason;
}

Source _resolveEntrySource(Source entry, Source? origin) {
  final location = entry.location;
  final relative =
      entry.kind == 'directory' &&
      !_absoluteDirectory(location) &&
      !_hasScheme(location);
  final local = !relative && entry.isLocal();
  if (origin == null || origin.kind == 'directory') {
    if (relative && origin != null) {
      final folder = _dirname(origin.location.replaceAll(r'\', '/'));
      return _changed(entry, location: _join([folder, location]));
    }
    return entry;
  }
  if (local) throw const _Refused('local-source');
  if (!relative) {
    if (entry.kind != 'git' && !entry.isWebUrl()) {
      throw const _Refused('invalid-source');
    }
    if (entry.holdsCredentials()) throw const _Refused('source-credentials');
    return entry;
  }
  if (origin.kind == 'git') {
    final folder = _dirname(origin.path ?? '');
    final path = _join([folder, location, entry.path ?? '']);
    if (path == '..' || path.startsWith('../')) {
      throw const _Refused('leaves-repository');
    }
    if (path != '.' && !validRelativePath(path)) {
      throw const _Refused('invalid-source');
    }
    return _changed(origin, setPath: true, path: path == '.' ? null : path);
  }
  return _changed(
    entry,
    kind: 'url',
    location: _urlJoin(origin.location, location),
    setReference: true,
  );
}

Document? _storeAbout(Object about, String at, List<String> ignored) {
  if (about is! Map<String, Object>) {
    ignored.add(at);
    return null;
  }
  final shown = <String, Object>{};
  for (final entry in about.entries) {
    if (aboutFieldValid(entry.key, entry.value)) {
      shown[entry.key] = entry.value;
    } else {
      ignored.add('$at/${_escapeToken(entry.key)}');
    }
  }
  return shown.isNotEmpty ? shown : null;
}

const List<String> _entryKeys = [
  'source',
  'ref',
  'pin',
  'description',
  'about',
];

/// An entry, or the reason it is refused (thrown as a [_Refused]).
CatalogEntry _catalogEntry(
  Object value,
  String at,
  Source? origin,
  List<String> ignored,
) {
  final Map<String, Object> fields;
  if (value is String) {
    fields = {'source': value};
  } else if (value is Map<String, Object>) {
    for (final key in value.keys) {
      if (!_entryKeys.contains(key)) throw const _Refused('invalid-entry');
    }
    fields = value;
  } else {
    throw const _Refused('invalid-entry');
  }
  final text = fields['source'];
  if (text is! String) throw const _Refused('invalid-entry');
  final Source parsed;
  try {
    parsed = Source.parse(text);
  } on AosError {
    throw const _Refused('invalid-source');
  }
  var source = _resolveEntrySource(parsed, origin);
  final reference = fields['ref'];
  if (reference != null) {
    if (reference is! String || !isGitRef(reference)) {
      throw const _Refused('invalid-entry');
    }
    if (parsed.kind != 'git' || parsed.reference != null) {
      throw const _Refused('ref-not-allowed');
    }
    source = _changed(source, setReference: true, reference: reference);
  }
  String? pin;
  final given = fields['pin'];
  if (given != null) {
    if (given is String && isDigest(given)) {
      pin = given;
    } else {
      throw const _Refused('invalid-pin');
    }
  }
  String? description;
  final words = fields['description'];
  if (words != null) {
    if (words is String) {
      description = words;
    } else {
      ignored.add('$at/description');
    }
  }
  final about = fields['about'];
  return CatalogEntry(
    source: source,
    pin: pin,
    description: description,
    about: about == null ? null : _storeAbout(about, '$at/about', ignored),
  );
}

CatalogSection _section(
  Document table,
  String prefix,
  bool Function(String name) nameOk,
  String Function(String name) fold,
  Source? origin,
  List<String> ignored,
) {
  final holders = <String, int>{};
  for (final key in table.keys) {
    if (nameOk(key)) holders[fold(key)] = (holders[fold(key)] ?? 0) + 1;
  }
  final out = CatalogSection();
  for (final key in table.keys.toList()..sort(compareText)) {
    if (!nameOk(key)) {
      out.refused.add((key, 'invalid-name'));
    } else if ((holders[fold(key)] ?? 0) > 1) {
      out.refused.add((key, 'duplicate-name'));
    } else {
      try {
        out.entries[key] = _catalogEntry(
          table[key]!,
          '$prefix/${_escapeToken(key)}',
          origin,
          ignored,
        );
      } on _Refused catch (refusal) {
        out.refused.add((key, refusal.reason));
      }
    }
  }
  return out;
}

/// Reads a catalog. [name] is the file's name, whose extension picks the format; [base] is the
/// source string the file came from, against which relative sources resolve. Nothing is fetched.
Catalog readCatalog(
  Uint8List data,
  String name,
  String? base,
  Settings settings,
) {
  final document = readCatalogDocument(data, name, settings);
  final overlaysValue = document['overlays'];
  final skillsValue = document['skills'];
  final Document? overlays = overlaysValue is Map<String, Object>
      ? overlaysValue
      : null;
  final Document? skills = skillsValue is Map<String, Object>
      ? skillsValue
      : null;
  if (overlays == null && skills == null) throw AosError('not-a-catalog', '');
  Source? origin;
  if (base != null) {
    try {
      origin = Source.parse(base);
    } on AosError {
      throw AosError('invalid-source', "the catalog's base");
    }
  }
  final ignored = <String>[];
  if (overlays == null && document.containsKey('overlays')) {
    ignored.add('/overlays');
  }
  final listed = [...?overlays?.values, ...?skills?.values];
  final form = listed.every((item) => item is String) ? 'minimal' : 'full';
  final fields = <String, String?>{};
  for (final field in const ['name', 'description', 'homepage']) {
    final value = document[field];
    if (value != null) {
      if (catalogFieldValid(field, value)) {
        fields[field] = value is String ? value : null;
      } else {
        ignored.add('/$field');
      }
    }
  }
  final overlaySection = _section(
    overlays ?? <String, Object>{},
    '/overlays',
    validOverlayName,
    normalizeName,
    origin,
    ignored,
  );
  CatalogSection? skillSection;
  if (skills != null) {
    skillSection = _section(
      skills,
      '/skills',
      (item) => validToolName(item) && validRelativePath(item),
      (item) => item.toLowerCase(),
      origin,
      ignored,
    );
  } else if (document.containsKey('skills')) {
    ignored.add('/skills');
  }
  ignored.sort(compareText);
  return Catalog(
    form: form,
    name: fields['name'],
    description: fields['description'],
    homepage: fields['homepage'],
    overlays: overlaySection,
    skills: skillSection,
    ignored: ignored,
  );
}

/// One overlay a listing offers.
class ListedEntry {
  /// Creates an entry.
  const ListedEntry({
    required this.catalog,
    required this.name,
    required this.identity,
    required this.shown,
  });

  /// The label of the catalog it comes from.
  final String catalog;

  /// The overlay's name there.
  final String name;

  /// The normalized name two catalogs' entries share.
  final String identity;

  /// [name], or `catalog/name` when another catalog offers the same identity.
  final String shown;
}

/// The overlays of several catalogs.
class Listing {
  /// Creates an empty listing.
  Listing();

  /// Every entry, by identity then catalog.
  final List<ListedEntry> entries = [];

  /// Identity to the labels of the catalogs that offer it, where more than one does.
  final Map<String, List<String>> collisions = {};
}

/// Lists the overlays of several catalogs, given as `(label, catalog)`, qualifying the names they
/// share. Refuses a label that is no valid name or repeats (`invalid-label`).
Listing listCatalogs(List<(String, Catalog)> catalogs) {
  final seen = <String>{};
  for (final (label, _) in catalogs) {
    if (!validOverlayName(label) || seen.contains(normalizeName(label))) {
      throw AosError('invalid-label', '');
    }
    seen.add(normalizeName(label));
  }
  final offered = <String, List<(String, String)>>{};
  for (final (label, catalog) in catalogs) {
    for (final name in catalog.overlays.entries.keys) {
      final identity = normalizeName(name);
      (offered[identity] ??= []).add((label, name));
    }
  }
  final listing = Listing();
  for (final identity in offered.keys.toList()..sort(compareText)) {
    final holders = offered[identity]!
      ..sort((a, b) {
        final first = compareText(a.$1, b.$1);
        return first != 0 ? first : compareText(a.$2, b.$2);
      });
    final qualified = holders.length > 1;
    if (qualified) {
      listing.collisions[identity] = [for (final (label, _) in holders) label];
    }
    for (final (label, name) in holders) {
      listing.entries.add(
        ListedEntry(
          catalog: label,
          name: name,
          identity: identity,
          shown: qualified ? '$label/$name' : name,
        ),
      );
    }
  }
  return listing;
}

/// The entry a [requested] `name` or `catalog/name` names: `not-found` for none, `ambiguous` for
/// several.
ListedEntry chooseListed(Listing listing, String requested) {
  final slash = requested.lastIndexOf('/');
  final label = slash == -1 ? null : requested.substring(0, slash);
  final name = slash == -1 ? requested : requested.substring(slash + 1);
  if (!validOverlayName(name) || (label != null && !validOverlayName(label))) {
    throw AosError('not-found', '');
  }
  final found = listing.entries
      .where(
        (entry) =>
            entry.identity == normalizeName(name) &&
            (label == null ||
                normalizeName(entry.catalog) == normalizeName(label)),
      )
      .toList();
  if (found.isEmpty) throw AosError('not-found', '');
  if (found.length > 1) throw AosError('ambiguous', '');
  return found.first;
}

/// An icon judged from its header alone.
class Icon {
  /// Creates an icon.
  const Icon({required this.format, required this.width, required this.height});

  /// A word of [iconFormat].
  final String format;

  /// Width in pixels.
  final int width;

  /// Height in pixels.
  final int height;
}

int _le(Uint8List data) {
  var value = 0;
  for (var at = data.length - 1; at >= 0; at -= 1) {
    value = value * 256 + data[at];
  }
  return value;
}

int _be(Uint8List data) {
  var value = 0;
  for (final byte in data) {
    value = value * 256 + byte;
  }
  return value;
}

bool _same(Uint8List data, int at, List<int> expected) =>
    startsWithBytes(data, expected, at);

List<int> _ascii(String text) => text.codeUnits;

(int, int) _pngSize(Uint8List data) {
  if (data.length < 24 || !_same(data, 8, [0, 0, 0, 0x0d, ..._ascii('IHDR')])) {
    throw AosError('icon-unreadable', '');
  }
  return (_be(data.sublist(16, 20)), _be(data.sublist(20, 24)));
}

(int, int) _webpSize(Uint8List data) {
  AosError unreadable() => AosError('icon-unreadable', '');
  if (data.length < 20 || _le(data.sublist(4, 8)) + 8 > data.length) {
    throw unreadable();
  }
  if (_same(data, 12, _ascii('VP8 ')) &&
      data.length >= 30 &&
      _same(data, 23, [0x9d, 0x01, 0x2a])) {
    return (
      _le(data.sublist(26, 28)) & 0x3fff,
      _le(data.sublist(28, 30)) & 0x3fff,
    );
  }
  if (_same(data, 12, _ascii('VP8L')) &&
      data.length >= 25 &&
      data[20] == 0x2f) {
    final bits = _le(data.sublist(21, 25));
    return ((bits & 0x3fff) + 1, ((bits ~/ 16384) & 0x3fff) + 1);
  }
  if (_same(data, 12, _ascii('VP8X')) && data.length >= 30) {
    return (_le(data.sublist(24, 27)) + 1, _le(data.sublist(27, 30)) + 1);
  }
  throw unreadable();
}

/// Reads an icon's format and size from its header: extension, size, signature and dimensions
/// are checked against [settings].
Icon readIcon(Uint8List data, String name, Settings settings) {
  final last = name.split('/').last;
  final dot = last.lastIndexOf('.');
  final extension = dot == -1 ? '' : asciiLower(last.substring(dot + 1));
  if (extension != 'png' && extension != 'webp') {
    throw AosError('icon-format', '');
  }
  if (!allows(settings.iconBytes, data.length)) {
    throw AosError('icon-too-large', '').over('icon_bytes');
  }
  final String format;
  if (startsWithBytes(data, [0x89, ..._ascii('PNG'), 0x0d, 0x0a, 0x1a, 0x0a])) {
    format = 'png';
  } else if (_same(data, 0, _ascii('RIFF')) && _same(data, 8, _ascii('WEBP'))) {
    format = 'webp';
  } else {
    throw AosError('icon-format', '');
  }
  if (format != extension) throw AosError('icon-mismatch', '');
  final (width, height) = format == 'png' ? _pngSize(data) : _webpSize(data);
  if (width == 0 || height == 0) throw AosError('icon-unreadable', '');
  if (!allows(settings.iconWidth, width)) {
    throw AosError('icon-dimensions', '').over('icon_width');
  }
  if (!allows(settings.iconHeight, height)) {
    throw AosError('icon-dimensions', '').over('icon_height');
  }
  return Icon(format: format, width: width, height: height);
}
