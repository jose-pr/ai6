/// Bounds: every limit the library applies is a setting with a default and no floor.
library;

/// A bound: a whole number, 0 or more, or null for no bound.
typedef Limit = int?;

/// No bound.
const Limit unbounded = null;

/// Whether [n] (an `int` or a `BigInt`) is within [limit]. A negative number never is.
bool allows(Limit limit, Object n) {
  if (n is BigInt) {
    if (n < BigInt.zero) return false;
    return limit == null || n <= BigInt.from(limit);
  }
  final count = n as int;
  if (count < 0) return false;
  return limit == null || count <= limit;
}

/// A limit as text: the number, or `none`.
String limitText(Limit limit) => limit == null ? 'none' : '$limit';

const int _mib = 1024 * 1024;

/// Every setting's key (as a configuration file writes it), field and default.
const List<(String, String, int)> _definitions = [
  ('manifest_bytes', 'manifestBytes', _mib),
  ('manifest_depth', 'manifestDepth', 64),
  ('tree_files', 'treeFiles', 4096),
  ('tree_depth', 'treeDepth', 64),
  ('tree_bytes', 'treeBytes', 64 * _mib),
  ('file_bytes', 'fileBytes', 16 * _mib),
  ('context_bytes', 'contextBytes', _mib),
  ('catalog_bytes', 'catalogBytes', 4 * _mib),
  ('icon_bytes', 'iconBytes', 512 * 1024),
  ('icon_width', 'iconWidth', 1024),
  ('icon_height', 'iconHeight', 1024),
  ('hook_timeout', 'hookTimeout', 60),
  ('hook_event_timeout', 'hookEventTimeout', 120),
  ('hook_output_bytes', 'hookOutputBytes', 64 * 1024),
  ('hook_payload_bytes', 'hookPayloadBytes', _mib),
  ('hook_reason_chars', 'hookReasonChars', 1024),
  ('substitution_bytes', 'substitutionBytes', _mib),
  ('substitution_depth', 'substitutionDepth', 32),
  ('substitution_steps', 'substitutionSteps', 10000000),
];

/// A setting that differs from its default, as data: never a document's text.
class NonDefault {
  /// Creates the report.
  const NonDefault(this.setting, this.value, this.byDefault);

  /// The setting's key as a configuration file writes it, such as `manifest_bytes`.
  final String setting;

  /// The value in force, as text.
  final String value;

  /// The default, as text.
  final String byDefault;

  @override
  String toString() => '$setting = $value (default $byDefault)';
}

/// Every bound the library applies, each with a default and no floor. Immutable: [copyWith]
/// returns a changed copy. [nonDefaults] lists the bounds that differ, so a caller can report
/// them.
///
/// Changes are keyed by field name (`'manifestBytes'`); a null value lifts the bound.
class Settings {
  /// Settings holding the defaults, with [changes] applied. A limit must be a non-negative
  /// whole number or null, else an [ArgumentError]; an unknown field is one too.
  Settings([Map<String, Limit> changes = const {}]) : _values = _build(changes);

  /// Reads settings as a configuration writes them (a map such as `readDocument` returns):
  /// snake_case keys, a number or `BigInt`, null or `"none"`. An unknown key is refused
  /// without naming it.
  factory Settings.fromData(Map<String, Object?> data) {
    final changes = <String, Limit>{};
    for (final entry in data.entries) {
      final definition = _definitions.where((d) => d.$1 == entry.key);
      if (definition.isEmpty) throw RangeError('unknown setting');
      final value = entry.value;
      Limit limit;
      if (value == null || value == 'none') {
        limit = null;
      } else if (value is BigInt) {
        if (value > BigInt.from(0x1fffffffffffff)) {
          throw RangeError('a limit too large to hold');
        }
        limit = value.toInt();
      } else if (value is int) {
        limit = value;
      } else {
        throw RangeError('a whole number, 0 or more, or null for no bound');
      }
      changes[definition.first.$2] = limit;
    }
    return Settings(changes);
  }

  final Map<String, Limit> _values;

  static Map<String, Limit> _build(Map<String, Limit> changes) {
    for (final field in changes.keys) {
      if (!_definitions.any((d) => d.$2 == field)) {
        throw ArgumentError.value(field, 'changes', 'not a setting');
      }
    }
    final out = <String, Limit>{};
    for (final (_, field, byDefault) in _definitions) {
      final Limit limit = changes.containsKey(field)
          ? changes[field]
          : byDefault;
      if (limit != null && limit < 0) {
        throw ArgumentError.value(
          limit,
          field,
          'a whole number, 0 or more, or null for no bound',
        );
      }
      out[field] = limit;
    }
    return Map.unmodifiable(out);
  }

  /// A copy with these bounds changed.
  Settings copyWith(Map<String, Limit> changes) =>
      Settings({..._values, ...changes});

  /// The bounds as a plain map, keyed by field name.
  Map<String, Limit> values() => Map.of(_values);

  /// Every setting that differs from its default.
  List<NonDefault> nonDefaults() => [
    for (final (setting, field, byDefault) in _definitions)
      if (_values[field] != byDefault)
        NonDefault(setting, limitText(_values[field]), limitText(byDefault)),
  ];

  /// Bytes of a manifest, an env file and an env script's output.
  Limit get manifestBytes => _values['manifestBytes'];

  /// Nesting of a manifest.
  Limit get manifestDepth => _values['manifestDepth'];

  /// Files and directories in a tree.
  Limit get treeFiles => _values['treeFiles'];

  /// Directory levels in a tree.
  Limit get treeDepth => _values['treeDepth'];

  /// Bytes in a tree.
  Limit get treeBytes => _values['treeBytes'];

  /// Bytes in one file of a tree.
  Limit get fileBytes => _values['fileBytes'];

  /// Bytes of an overlay's context.
  Limit get contextBytes => _values['contextBytes'];

  /// Bytes of a catalog.
  Limit get catalogBytes => _values['catalogBytes'];

  /// Bytes of an icon.
  Limit get iconBytes => _values['iconBytes'];

  /// Width of an icon, in pixels.
  Limit get iconWidth => _values['iconWidth'];

  /// Height of an icon, in pixels.
  Limit get iconHeight => _values['iconHeight'];

  /// Seconds a hook may run.
  Limit get hookTimeout => _values['hookTimeout'];

  /// Seconds the hooks of one event may run.
  Limit get hookEventTimeout => _values['hookEventTimeout'];

  /// Bytes a hook may print.
  Limit get hookOutputBytes => _values['hookOutputBytes'];

  /// Bytes of a hook's payload.
  Limit get hookPayloadBytes => _values['hookPayloadBytes'];

  /// Characters of a hook's reason.
  Limit get hookReasonChars => _values['hookReasonChars'];

  /// Bytes substitution may produce.
  Limit get substitutionBytes => _values['substitutionBytes'];

  /// Nesting of substitution.
  Limit get substitutionDepth => _values['substitutionDepth'];

  /// Steps substitution may take.
  Limit get substitutionSteps => _values['substitutionSteps'];
}

/// The default settings.
final Settings defaultSettings = Settings();
