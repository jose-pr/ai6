/// The one error: a refusal by class.
library;

import 'text.dart';

/// The classes of refusal the specification's conformance vectors name, plus this package's own.
const List<String> errorClasses = [
  'ambiguous',
  'case-collision',
  'cycle',
  'datetime-value',
  'dotenv-syntax',
  'duplicate-key',
  'duplicate-member',
  'env-path-conflict',
  'env-script-output',
  'hook-refused',
  'icon-dimensions',
  'icon-format',
  'icon-mismatch',
  'icon-too-large',
  'icon-unreadable',
  'integer-range',
  'invalid-digest',
  'invalid-env-file',
  'invalid-label',
  'invalid-name',
  'invalid-option',
  'invalid-pattern',
  'invalid-reference',
  'invalid-requirements',
  'invalid-source',
  'invalid-unicode',
  'invalid-utf8',
  'io',
  'malformed-fence',
  'missing-file',
  'no-format-named',
  'non-finite-float',
  'non-string-key',
  'non-utf8-name',
  'not-a-catalog',
  'not-a-mapping',
  'not-a-skill',
  'not-found',
  'null-value',
  'parse-error',
  'partial-lock',
  'pin-mismatch',
  'reserved-variable',
  'secret-placeholder',
  'skill-name-mismatch',
  'skill-unreadable',
  'special-file',
  'symlink',
  'too-complex',
  'too-deep',
  'too-large',
  'two-manifests',
  'undefined-variable',
  'unresolved-reference',
  'unsupported-format',
  'unterminated-front-matter',
  'yaml-anchor',
  'yaml-directive',
  'yaml-merge-key',
  'yaml-tag',
];

/// The class a word names, or null.
String? classNamed(String word) => errorClasses.contains(word) ? word : null;

/// The one error: a refusal by class. [errorClass] is the contract, a word from [errorClasses];
/// match on it, never on [subject], which names what the refusal is about (a variable, a path, a
/// pointer) for a person and is never text from a document. [line] and [column] are one-based;
/// [column] counts Unicode scalar values.
class AosError implements Exception {
  /// Refuses with [errorClass], naming [subject]. The subject is escaped and cut to 256
  /// characters whoever builds the error.
  AosError(this.errorClass, [String subject = ''])
    : subject = safeSubject(subject) {
    _message = _describe(errorClass, this.subject, null, null);
  }

  /// The refusal's class.
  final String errorClass;

  /// What the refusal is about, escaped; never text from a document.
  final String subject;

  int? _line;
  int? _column;
  String? _setting;
  late String _message;

  /// For a refusal of a limit (`too-large`, `too-deep`, `too-complex`, `icon-too-large`,
  /// `icon-dimensions`): the key of the setting that was passed, as a configuration writes it,
  /// such as `tree_files`. Stable, unlike [subject]'s wording; null for any other refusal.
  String? get setting => _setting;

  /// This error naming the [setting] whose limit was passed; returns itself.
  AosError over(String setting) {
    _setting = setting;
    return this;
  }

  /// `class: subject at line N column M`.
  String get message => _message;

  /// The one-based line, if the error has a position.
  int? get line => _line;

  /// The one-based column, if the error has one.
  int? get column => _column;

  static String _describe(String cls, String subject, int? line, int? column) {
    final text = StringBuffer(cls);
    if (subject != '') text.write(': $subject');
    if (line != null) {
      text.write(' at line $line');
      if (column != null) text.write(' column $column');
    }
    return text.toString();
  }

  /// This error with a position; returns itself.
  AosError at(int line, int column) {
    _line = line;
    _column = column;
    _message = _describe(errorClass, subject, line, column);
    return this;
  }

  /// This error with a line, keeping any column; returns itself.
  AosError onLine(int line) {
    _line = line;
    _message = _describe(errorClass, subject, line, _column);
    return this;
  }

  @override
  String toString() => _message;
}
