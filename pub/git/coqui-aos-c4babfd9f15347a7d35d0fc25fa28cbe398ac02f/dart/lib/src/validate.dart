/// Validating a manifest's data model: the rules `schemas/overlay.schema.json` states, then those
/// it cannot (an integer refusing a float, cross references, reserved names, files that must
/// exist), each reported with a JSON pointer to the offending value.
library;

import 'text.dart';
import 'tree.dart';
import 'validate_check.dart';
import 'validate_files.dart';
import 'validate_types.dart';
import 'value.dart';

export 'validate_check.dart'
    show
        aboutFieldValid,
        catalogFieldValid,
        envValueOk,
        isDigest,
        isGitRef,
        validLicense;
export 'validate_files.dart' show namedFiles, skillFlags;
export 'validate_types.dart' show Issue, issueCode, issueCodes;

/// Whether a manifest with these problems is still applied: a problem under `/about`, which is
/// informational only, never stops it.
bool applies(List<Issue> issues) => issues.every(
  (issue) => issue.at == '/about' || issue.at.startsWith('/about/'),
);

/// Every problem with [manifest], sorted and without repeats; empty when it is valid. With
/// [tree], every file a field names must be one of the overlay's files, and `env` may set no
/// variable a default path addition adds to.
List<Issue> validate(Document manifest, [OverlayTree? tree]) {
  final check = Check();
  check.document(manifest);
  semantic(manifest, check);
  if (tree != null) validateFiles(manifest, tree, check);
  final unique = <String, Issue>{};
  for (final issue in check.issues) {
    unique['${issue.at}\u0000${issue.code}'] = issue;
  }
  return unique.values.toList()..sort((a, b) {
    final byAt = compareText(a.at, b.at);
    return byAt != 0 ? byAt : compareText(a.code, b.code);
  });
}
