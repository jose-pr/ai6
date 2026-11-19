/// What preparing an overlay does when it is added, synced, activated or removed.
library;

import 'apply.dart';
import 'defaults.dart';
import 'kinds.dart';
import 'names.dart';

/// What the user is doing to an overlay: `add`, `sync`, `activate` or `remove`.
const changes = Words(['add', 'sync', 'activate', 'remove']);

/// A setup script's phase, its first argument: `install`, `sync` or `remove`.
const phaseWords = Words(['install', 'sync', 'remove']);

/// What was last prepared: the overlay's digest, and the phase that did it.
class SetupRecord {
  /// Creates the record.
  const SetupRecord({required this.digest, required this.phase});

  /// The digest prepared.
  final String digest;

  /// The phase that prepared it.
  final String phase;

  /// The record as a consumer keeps it: `{"digest": ..., "phase": ...}`.
  Map<String, String> toJson() => {'digest': digest, 'phase': phase};

  /// What [toJson] wrote, or a [FormatException]: [json] is not an object with a `digest`
  /// string and a `phase` that is exactly `install`, `sync` or `remove`.
  factory SetupRecord.fromJson(Object? json) {
    if (json is! Map) {
      throw const FormatException('a setup record is an object');
    }
    final digest = json['digest'];
    final name = json['phase'];
    if (digest is! String || name is! String) {
      throw const FormatException(
        'a setup record has a digest and a phase, both text',
      );
    }
    final phase = phaseWords.named(name);
    if (phase == null) throw FormatException('not a setup phase', name);
    return SetupRecord(digest: digest, phase: phase);
  }
}

/// Which phase, if any, a [change] calls for, given the digest last [recorded] and the current
/// one.
String? setupPhase(String change, String? recorded, String? digest) {
  if (change == 'remove') return recorded == null ? null : 'remove';
  if (recorded == null) return 'install';
  if (change == 'activate' &&
      digest != null &&
      asciiLower(digest) == asciiLower(recorded)) {
    return null;
  }
  return 'sync';
}

/// The steps a change runs and what is recorded after them.
class Preparation {
  /// Creates the plan.
  const Preparation({
    required this.phase,
    required this.arguments,
    required this.steps,
    required this.record,
    required this.activates,
  });

  /// The phase, if the change calls for one.
  final String? phase;

  /// The setup script's arguments: the phase, then its own.
  final List<String>? arguments;

  /// The steps: `empty-state`, `deps:python`, `deps:node`, `setup`, `record`, `delete`.
  final List<String> steps;

  /// What is recorded afterwards.
  final SetupRecord? record;

  /// Whether the overlay may be activated afterwards.
  final bool activates;
}

/// Plans a change; [fails] names the step (a word of `failStep`) that fails, ending the run
/// there.
Preparation preparation(
  String change,
  SetupRecord? record,
  String? digest,
  SetupRun? setup,
  List<Dependency> dependencies,
  String? fails,
) {
  final recorded = record?.digest;
  final phase = setup != null || dependencies.isNotEmpty
      ? setupPhase(change, recorded, digest)
      : null;
  var steps = <String>[];
  if (phase == 'install') steps.add('empty-state');
  if (phase == 'install' || phase == 'sync') {
    for (final dependency in dependencies) {
      steps.add('deps:${dependency.runtime}');
    }
  }
  if (phase != null && setup != null) steps.add('setup');
  var failed = false;
  if (fails != null) {
    final index = steps.indexWhere(
      (step) => fails == 'deps' ? step.startsWith('deps:') : step == 'setup',
    );
    if (index != -1) {
      steps = steps.sublist(0, index + 1);
      failed = true;
    }
  }
  SetupRecord? after;
  bool activates;
  if (change == 'remove') {
    steps.add('delete');
    after = null;
    activates = false;
  } else if (phase == null) {
    after = record;
    activates = true;
  } else if (failed) {
    // The record still names the last digest prepared; only that one stays usable.
    after = record;
    activates = recorded != null && recorded == digest;
  } else {
    steps.add('record');
    after = SetupRecord(digest: digest ?? '', phase: phase);
    activates = true;
  }
  return Preparation(
    phase: phase,
    arguments: phase != null && setup != null ? [phase, ...setup.args] : null,
    steps: steps,
    record: after,
    activates: activates,
  );
}
