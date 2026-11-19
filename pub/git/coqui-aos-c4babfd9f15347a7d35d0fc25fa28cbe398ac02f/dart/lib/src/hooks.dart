/// Hooks: which an overlay declares, whether they are on, what each event tells them, and the
/// checking of what a run left. This library never runs a hook: the caller does, and builds a
/// [HookRun] from what the process did.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'defaults.dart' as defaults;
import 'defaults.dart' show defaultOn, defaultScripts;
import 'digest.dart';
import 'error.dart';
import 'json.dart';
import 'kinds.dart';
import 'limits.dart';
import 'names.dart';
import 'read.dart';
import 'same_digest.dart';
import 'text.dart';
import 'tree.dart';
import 'value.dart';

/// The moments a hook may run at.
const hookEvent = Words([
  'pre-env',
  'session-start',
  'session-end',
  'prompt',
  'turn-end',
  'cd',
  'pre-compact',
  'pre-tool',
  'post-tool',
]);

/// What a hook's output decides for its event.
const decision = Words(['block', 'deny', 'ask', 'allow', 'refuse']);

/// What a failed hook does to its event.
const onFailure = Words(['report', 'refuse']);

/// Whose hooks they are: an overlay's, the user's own (always on) or a project's.
const hookScope = Words(['overlay', 'user', 'project']);

/// Every way a run fails.
const hookFailure = Words([
  'not-run',
  'payload-too-large',
  'timeout',
  'too-large',
  'exit',
  'output',
  'event-timeout',
]);

/// A hook's failure as a thrown value: the `hookFailure` word.
class HookFailed extends AosError {
  /// Fails with [failure], a word of [hookFailure].
  HookFailed(this.failure) : super('hook-refused', failure);

  /// The failure word.
  final String failure;
}

const Map<String, bool> _eventDefaultOn = {
  'pre-env': true,
  'session-start': true,
  'session-end': true,
  'prompt': false,
  'turn-end': true,
  'cd': true,
  'pre-compact': true,
  'pre-tool': false,
  'post-tool': false,
};

/// The outputs an event's hook may give.
const Map<String, List<String>> _outputs = {
  'pre-env': ['env'],
  'cd': ['env'],
  'session-start': ['context', 'env'],
  'session-end': [],
  'turn-end': [],
  'prompt': ['context', 'block', 'env'],
  'pre-compact': ['context', 'block'],
  'pre-tool': ['context', 'deny', 'ask', 'allow', 'input', 'env'],
  'post-tool': ['context'],
};

const Map<String, String> _refusals = {
  'pre-env': 'refuse',
  'session-start': 'refuse',
  'prompt': 'block',
  'pre-compact': 'block',
  'pre-tool': 'deny',
};

const Map<String, String> _selects = {
  'session-start': 'reason',
  'pre-tool': 'kind',
  'post-tool': 'kind',
};

const Map<String, List<String>> _payloadKeys = {
  'pre-env': ['event', 'harness', 'cwd'],
  'session-start': ['event', 'harness', 'cwd', 'session', 'reason'],
  'session-end': ['event', 'harness', 'cwd', 'session'],
  'turn-end': ['event', 'harness', 'cwd', 'session'],
  'prompt': ['event', 'harness', 'cwd', 'session', 'prompt'],
  'cd': ['event', 'harness', 'cwd', 'session', 'previous'],
  'pre-compact': ['event', 'harness', 'cwd', 'session', 'trigger'],
  'pre-tool': ['event', 'harness', 'cwd', 'session', 'tool'],
  'post-tool': ['event', 'harness', 'cwd', 'session', 'tool', 'status'],
};

/// The payload keys an event's hook is told, `event` first.
List<String> payloadKeys(String event) => _payloadKeys[event]!;

int _rank(String? decided) {
  if (decided == null) return 0;
  if (decided == 'allow') return 1;
  return decided == 'ask' ? 2 : 3;
}

/// A hook as an overlay declares it.
class Hook {
  /// Creates a hook.
  const Hook({
    required this.event,
    required this.path,
    required this.interpreter,
    required this.runs,
    required this.select,
    required this.sets,
    required this.timeout,
    required this.onFailure,
    required this.payload,
  });

  /// The moment it runs at.
  final String event;

  /// The script's path in the overlay; null when none is chosen.
  final String? path;

  /// The interpreter that runs it.
  final String? interpreter;

  /// Whether the consumer has the interpreter.
  final bool runs;

  /// The `match` list, when one is written.
  final List<String>? select;

  /// The variables the hook may set.
  final List<String> sets;

  /// Seconds.
  final int timeout;

  /// `report` or `refuse`.
  final String onFailure;

  /// The payload keys it is told.
  final List<String> payload;

  /// A copy with another [timeout].
  Hook withTimeout(int timeout) => Hook(
    event: event,
    path: path,
    interpreter: interpreter,
    runs: runs,
    select: select,
    sets: sets,
    timeout: timeout,
    onFailure: onFailure,
    payload: payload,
  );
}

String? _extensionInterpreter(String path) {
  final at = path.lastIndexOf('.');
  if (at == -1) return null;
  switch (path.substring(at)) {
    case '.py':
      return 'python';
    case '.sh':
      return 'sh';
    case '.bash':
      return 'bash';
    case '.ps1':
      return 'powershell';
    case '.js':
    case '.mjs':
    case '.cjs':
      return 'node';
    default:
      return null;
  }
}

({({String path, String interpreter})? chosen, List<String> present})
_defaultHook(OverlayTree tree, String os, List<String> interpreters) {
  final members = tree.members();
  final present = [
    for (final (extension, _, _) in defaultScripts)
      if (members.contains('hook$extension')) 'hook$extension',
  ];
  for (final (extension, interpreter, on) in defaultScripts) {
    final path = 'hook$extension';
    final names = on == null ? null : [on];
    if (present.contains(path) &&
        osMatches(names, os) &&
        interpreters.contains(interpreter)) {
      return (chosen: (path: path, interpreter: interpreter), present: present);
    }
  }
  return (chosen: null, present: present);
}

int? _asNumber(Object? value) =>
    value is BigInt && value >= BigInt.zero ? value.toInt() : null;

List<String>? _strings(Object? value) {
  if (value is! List<Object?>) return null;
  return [
    for (final item in value)
      if (item is String) item,
  ];
}

/// The hooks a manifest declares or defaults to, and the `hook.<ext>` files reported
/// `script-not-run` because no consumer interpreter runs them. Call `validate` first.
({List<Hook> hooks, List<String> notRun}) hooksOf(
  Document manifest,
  OverlayTree tree,
  String os,
  List<String> interpreters,
) {
  final declaredValue = manifest['hooks'];
  final Map<String, Object?>? declared = declaredValue is Map<String, Object?>
      ? declaredValue
      : null;
  final (:chosen, :present) = _defaultHook(tree, os, interpreters);
  final entries = <Hook>[];
  var needsDefault = false;
  for (final event in hookEvent.all) {
    final given = declared?[event];
    final Map<String, Object?> table;
    if (given == false) continue;
    if (given is Map<String, Object?>) {
      table = given;
    } else if (given != null) {
      table = {'run': given};
    } else if (_eventDefaultOn[event]! &&
        defaultOn(manifest, 'hooks') &&
        present.isNotEmpty) {
      table = {};
    } else {
      continue;
    }
    String? path;
    String? interpreter;
    final run = table['run'];
    if (run is String) {
      path = run;
      interpreter = _extensionInterpreter(run);
    } else if (chosen != null) {
      path = chosen.path;
      interpreter = chosen.interpreter;
    } else {
      needsDefault = true;
      if (declared == null || !declared.containsKey(event)) continue;
    }
    entries.add(
      Hook(
        event: event,
        path: path,
        interpreter: interpreter,
        runs: interpreter != null && interpreters.contains(interpreter),
        select: _strings(table['match']),
        sets: _strings(table['sets']) ?? [],
        timeout: _asNumber(table['timeout']) ?? 30,
        onFailure: table['on_failure'] == 'refuse' ? 'refuse' : 'report',
        payload: [..._payloadKeys[event]!],
      ),
    );
  }
  return (hooks: entries, notRun: needsDefault ? present : <String>[]);
}

/// Whether the overlay declares a hook on any OS with any interpreter: [hooksOf] finds an entry
/// or a `hook.<ext>` that cannot run, for some OS, were every interpreter AOS names present.
/// Entries an overlay switches off (`false`, or `defaults.hooks = false`) are not declared. For a
/// consumer that runs no hooks and refuses an overlay that asks for any, whatever it runs on.
bool hooksDeclared(Document manifest, OverlayTree tree) {
  for (final os in const ['windows', 'linux']) {
    final found = hooksOf(manifest, tree, os, defaults.interpreters);
    if (found.hooks.isNotEmpty || found.notRun.isNotEmpty) return true;
  }
  return false;
}

/// What a consumer knows when it asks whether an overlay's hooks are on.
class HookFacts {
  /// Creates the facts.
  const HookFacts({
    required this.os,
    required this.identity,
    this.interpreters,
    this.scope = 'overlay',
    this.digest,
    this.tree,
    this.allowed,
    this.fires,
    this.partial = const [],
    this.timeout,
  });

  /// The operating system's name.
  final String os;

  /// The overlay's identity.
  final String identity;

  /// The interpreters the consumer has; null means every one AOS names.
  final List<String>? interpreters;

  /// A word of [hookScope].
  final String scope;

  /// The overlay's tree digest, as text.
  final String? digest;

  /// A project's files by path and bytes: its digest replaces [digest].
  final List<(String, Uint8List)>? tree;

  /// The digest the user allowed.
  final String? allowed;

  /// The events the consumer fires; null means every one.
  final List<String>? fires;

  /// Events it fires in part.
  final List<String> partial;

  /// A bound on a hook's seconds.
  final int? timeout;
}

/// Facts for an [os] and an overlay [identity]; the rest by default, set by the named arguments.
HookFacts hookFacts(
  String os,
  String identity, {
  List<String>? interpreters,
  String scope = 'overlay',
  String? digest,
  List<(String, Uint8List)>? tree,
  String? allowed,
  List<String>? fires,
  List<String> partial = const [],
  int? timeout,
}) => HookFacts(
  os: os,
  identity: identity,
  interpreters: interpreters,
  scope: scope,
  digest: digest,
  tree: tree,
  allowed: allowed,
  fires: fires,
  partial: partial,
  timeout: timeout,
);

/// The result of [hookPlan].
class HookPlan {
  /// Creates the plan.
  const HookPlan({
    required this.hooks,
    required this.on,
    required this.activates,
    required this.needs,
    required this.reports,
    required this.nonDefaults,
  });

  /// The hooks, their timeouts bounded.
  final List<Hook> hooks;

  /// Whether the hooks are allowed and there are some.
  final bool on;

  /// False for a guard (a hook that refuses on failure) that is off: its overlay stays inactive.
  final bool activates;

  /// The interpreters the hooks need.
  final List<String> needs;

  /// `(id, subject)`: `hook-not-allowed`, `hook-unsupported`, `hook-partial`.
  final List<(String, String)> reports;

  /// `(id, subject)`: `hook` and `script-not-run`.
  final List<(String, String)> nonDefaults;
}

int _comparePairs((String, String) a, (String, String) b) {
  final first = compareText(a.$1, b.$1);
  return first != 0 ? first : compareText(a.$2, b.$2);
}

/// Plans an overlay's hooks against what the consumer knows. A hook is off until the user allows
/// it at the overlay's digest.
HookPlan hookPlan(
  Document manifest,
  OverlayTree files,
  HookFacts facts,
  Settings settings,
) {
  final interpreters = facts.interpreters ?? defaults.interpreters;
  final found = hooksOf(manifest, files, facts.os, interpreters);
  final bound = facts.timeout ?? settings.hookTimeout ?? 9007199254740991;
  final hooks = [
    for (final hook in found.hooks)
      hook.withTimeout(hook.timeout < bound ? hook.timeout : bound),
  ];
  var digest = facts.digest;
  final tree = facts.tree;
  if (tree != null) {
    // Every file of the directory, not only those the hooks name: a module beside a script is
    // loaded by it.
    final entries = <(List<Object>, TreeEntry)>[
      for (final (path, bytes) in tree)
        (<Object>[...path.split('/')], FileEntry(bytes)),
    ];
    checkEntries(entries, settings);
    digest = digestEntries(entries).digest.toString();
  }
  final allowed = facts.allowed;
  final on =
      facts.scope == 'user' ||
      (digest != null && allowed != null && sameDigest(digest, allowed));
  final fires = facts.fires ?? hookEvent.all;
  final nonDefaults = <(String, String)>[
    for (final path in found.notRun) ('script-not-run', path),
  ];
  final reports = <(String, String)>[];
  if (hooks.isNotEmpty && !on) {
    reports.add(('hook-not-allowed', facts.identity));
  } else if (on) {
    for (final hook in hooks) {
      if (!fires.contains(hook.event)) {
        reports.add(('hook-unsupported', hook.event));
        continue;
      }
      nonDefaults.add(('hook', hook.event));
      if (facts.partial.contains(hook.event)) {
        reports.add(('hook-partial', hook.event));
      }
    }
  }
  final guarded = hooks.any((hook) => hook.onFailure == 'refuse');
  final needs = <String>{
    for (final hook in hooks)
      if (hook.interpreter != null) hook.interpreter!,
  }.toList()..sort(compareText);
  reports.sort(_comparePairs);
  nonDefaults.sort(_comparePairs);
  return HookPlan(
    hooks: hooks,
    on: on && hooks.isNotEmpty,
    activates: on || !guarded,
    needs: needs,
    reports: reports,
    nonDefaults: nonDefaults,
  );
}

/// Whether a hook's `match` selects [value]; no `match` selects everything, and a tool's `other`
/// kind is always selected.
bool hookMatches(String event, List<String>? select, String value) {
  if (select == null) return true;
  if (_selects[event] == 'kind' && value == 'other') return true;
  return select.contains(value);
}

/// The payload a hook is told: `event`, then the event's keys that [known] holds; a tool keeps
/// only its `kind`, `name` and `input`.
Document hookPayload(String event, Map<String, Object> known) {
  final payload = <String, Object>{'event': event};
  for (final key in _payloadKeys[event]!.skip(1)) {
    final value = known[key];
    if (value == null) continue;
    if (key == 'tool' && value is Map<String, Object?>) {
      final tool = <String, Object>{};
      for (final field in const ['kind', 'name', 'input']) {
        final item = value[field];
        if (item != null) tool[field] = item;
      }
      payload[key] = tool;
    } else {
      payload[key] = value;
    }
  }
  return payload;
}

/// What a hook's output decided and asked for. Its text is never shown by [toString] or
/// [toJson].
class HookOutput {
  /// Creates the output.
  HookOutput({this.decision, this.reason, this.context, this.input, this.env});

  /// A word of [decision], if one was given.
  String? decision;

  /// The reason given with a decision.
  String? reason;

  /// Context to add.
  String? context;

  /// With an allow, the whole object that replaces the call's input.
  Map<String, Object?>? input;

  /// Variables to set; a null value unsets one.
  List<(String, String?)>? env;

  /// The output without its text: lengths and variable names.
  Map<String, Object?> toJson() => {
    'decision': decision,
    'reason': reason?.length,
    'context': context?.length,
    'input': input?.length,
    'env': env?.map((e) => e.$1).toList(),
  };

  @override
  String toString() => 'HookOutput ${jsonEncode(toJson())}';
}

bool _hookText(String text) {
  for (var at = 0; at < text.length; at += 1) {
    final c = text.codeUnitAt(at);
    if ((c <= 0x08) ||
        c == 0x0b ||
        c == 0x0c ||
        (c >= 0x0e && c <= 0x1f) ||
        (c >= 0x7f && c <= 0x9f) ||
        (c >= 0x202a && c <= 0x202e) ||
        (c >= 0x2066 && c <= 0x2069)) {
      return false;
    }
  }
  return true;
}

/// Reads what a hook printed for [event]; [sets] are the variables it declared. Throws
/// [HookFailed].
HookOutput readHookOutput(
  String event,
  Uint8List data,
  List<String> sets,
  Settings settings,
) {
  final outputs = _outputs[event]!;
  if (event == 'pre-env') {
    final List<(String, String?)> set;
    try {
      set = readEnvScriptOutput(data, settings);
    } on Object {
      throw HookFailed('output');
    }
    for (final (_, value) in set) {
      if (value != null && !_hookText(value)) throw HookFailed('output');
    }
    return HookOutput(env: set);
  }
  final String text;
  try {
    text = decodeUtf8(data);
  } on AosError {
    throw HookFailed('output');
  }
  final whole = trimAscii(text);
  if (whole == '') return HookOutput();
  final Object? value;
  try {
    value = readJson(whole, settings.manifestDepth, true);
  } on Object {
    throw HookFailed('output');
  }
  if (value is! Map<String, Object?>) throw HookFailed('output');
  final out = HookOutput();
  final decided = value['decision'];
  if (decided != null) {
    if (decided == 'block' && outputs.contains('block')) {
      out.decision = 'block';
    } else if (decided == 'deny' && outputs.contains('deny')) {
      out.decision = 'deny';
    } else if (decided == 'ask' && outputs.contains('ask')) {
      out.decision = 'ask';
    } else if (decided == 'allow' && outputs.contains('allow')) {
      out.decision = 'allow';
    } else {
      throw HookFailed('output');
    }
  }
  for (final key in value.keys) {
    if (key != 'context' &&
        key != 'decision' &&
        key != 'reason' &&
        key != 'input' &&
        key != 'env') {
      throw HookFailed('output');
    }
  }
  final rewritten = value['input'];
  if (rewritten != null) {
    if (rewritten is! Map<String, Object?> ||
        !outputs.contains('input') ||
        out.decision != 'allow') {
      throw HookFailed('output');
    }
    out.input = rewritten;
  }
  final reason = value['reason'];
  if (reason != null) {
    final cap = settings.hookReasonChars;
    if (reason is! String ||
        out.decision == null ||
        !_hookText(reason) ||
        (cap != null && reason.runes.length > cap)) {
      throw HookFailed('output');
    }
    out.reason = reason;
  }
  final context = value['context'];
  if (context != null) {
    if (context is String &&
        outputs.contains('context') &&
        _hookText(context)) {
      out.context = context;
    } else {
      throw HookFailed('output');
    }
  }
  final env = value['env'];
  if (env != null) {
    if (env is! Map<String, Object?> || !outputs.contains('env')) {
      throw HookFailed('output');
    }
    final folded = <String>[];
    final set = <(String, String?)>[];
    for (final entry in env.entries) {
      final name = entry.key;
      final item = entry.value;
      if (!isVariableName(name) ||
          isReservedVariable(name) ||
          !sets.contains(name) ||
          folded.contains(asciiUpper(name))) {
        throw HookFailed('output');
      }
      folded.add(asciiUpper(name));
      if (item == null) {
        set.add((name, null));
      } else if (item is String && !item.contains('\u0000')) {
        set.add((name, item));
      } else {
        throw HookFailed('output');
      }
    }
    out.env = set;
  }
  return out;
}

/// What a caller saw of one hook process. Its output text is never shown by [toString] or
/// [toJson].
class HookRun {
  /// A run for the hook [source] with its [stdout]; set the other fields on it.
  HookRun(this.source, [Uint8List? stdout]) : stdout = stdout ?? Uint8List(0);

  /// The hook's source, as the caller names it.
  String source;

  /// What failing does to the event: a word of [onFailure].
  String? onFailure;

  /// How long it ran.
  num seconds = 0;

  /// Whether it never started.
  bool notRun = false;

  /// The size of the payload it was given.
  int payloadSize = 0;

  /// Whether it was stopped at its time bound.
  bool timedOut = false;

  /// What it printed.
  Uint8List stdout;

  /// Its exit status.
  int exit = 0;

  /// The variables it declared it may set.
  List<String> sets = [];

  /// Its entry's `match`, or null for every call: on a call of kind `other` only a hook with none,
  /// or one naming `other`, may allow.
  List<String>? select;

  /// The run without its output: the source, status and output size.
  Map<String, Object?> toJson() => {
    'source': source,
    'exit': exit,
    'timedOut': timedOut,
    'stdoutBytes': stdout.length,
  };

  @override
  String toString() => 'HookRun ${jsonEncode(toJson())}';
}

/// A run for the hook [source] with its [stdout]; set the other fields on it.
HookRun hookRun(String source, [Uint8List? stdout]) => HookRun(source, stdout);

/// Checks a run in a fixed order (started, payload, time, size, exit, output). Throws
/// [HookFailed].
HookOutput hookResult(String event, HookRun run, Settings settings) {
  if (run.notRun) throw HookFailed('not-run');
  final payload = settings.hookPayloadBytes;
  if (payload != null && run.payloadSize > payload) {
    throw HookFailed('payload-too-large');
  }
  if (run.timedOut) throw HookFailed('timeout');
  final output = settings.hookOutputBytes;
  if (output != null && run.stdout.length > output) {
    throw HookFailed('too-large');
  }
  if (run.exit != 0) throw HookFailed('exit');
  return readHookOutput(event, run.stdout, run.sets, settings);
}

/// One thing worth telling the user about an event.
class HookReport {
  /// Creates the report.
  const HookReport(this.id, this.about);

  /// `hook-failed`, `hook-ask-denied` or `hook-not-applied`.
  final String id;

  /// The hook's source, the event, and for some the cause or the output refused.
  final List<String> about;

  /// The report as data.
  Map<String, Object> toJson() => {'id': id, 'about': about};
}

/// The combined result of one event's runs. Context and variable values are never shown by
/// [toString] or [toJson].
class FiredEvent {
  /// The sources of the runs that ran.
  List<String> ran = [];

  /// The tightest decision.
  String? decision;

  /// The source whose decision won.
  String? by;

  /// The winning decision's reason.
  String? reason;

  /// `(source, context)` of each run that gave some.
  List<(String, String)> context = [];

  /// The variables set, in order; a null value unsets one.
  List<(String, String?)> env = [];

  /// What went wrong or was not applied.
  List<HookReport> reports = [];

  /// `(id, subject)`: `env-sensitive`.
  List<(String, String)> nonDefaults = [];

  /// With a `toolInput`: `(source, input)` for what each hook that ran was given.
  List<(String, Object?)>? seen;

  /// With a `toolInput`: the input the call runs with, set only for a combined allow; null is the
  /// one it was given.
  Map<String, Object?>? input;

  /// The event without context or values.
  Map<String, Object?> toJson() => {
    'ran': ran,
    'decision': decision,
    'by': by,
    'contextFrom': context.map((c) => c.$1).toList(),
    'env': env.map((e) => e.$1).toList(),
    'reports': reports.map((r) => r.toJson()).toList(),
  };

  @override
  String toString() => 'FiredEvent ${jsonEncode(toJson())}';
}

/// Combines the runs of one event: the tightest decision wins, each run is checked and a failure
/// that refuses (`on_failure`) becomes the event's refusal. [applies] lists what the consumer
/// can apply (`context`, `env`, `ask`, `block`, `deny`, ...); null means everything the event
/// allows. [windows] makes variable names compare without regard to ASCII case.
///
/// [contextUsed] is what the session's context already holds, in bytes: a part of a hook's
/// context that would take the total past `contextBytes` is not added, whole and never cut, and is
/// reported as `hook-not-applied`; a later part that fits still is added.
///
/// [toolInput] is the call's input as the harness gave it, for a `pre-tool` event whose allow may
/// rewrite it: [FiredEvent.seen] then says what each hook was given (a hook's `input` replaces it
/// for the hooks after it) and [FiredEvent.input] is what the call runs with, set only by a
/// combined allow, since an ask or a deny beats a rewrite. An allow whose `input` the consumer
/// cannot apply (`input` is not in [applies]) is not applied at all; a rewrite that an ask beat is
/// reported `hook-not-applied`.
///
/// [toolKind] is the call's kind: on `other`, a run whose [HookRun.select] is given and does not
/// name `other` still runs and its deny and ask count, but its allow and input do not, and are
/// reported `hook-not-applied` `allow`.
FiredEvent fireEvent(
  String event,
  List<HookRun> runs,
  List<String>? applies,
  Settings settings,
  bool windows, [
  int contextUsed = 0,
  Object? toolInput,
  String? toolKind,
]) {
  bool can(String output) => applies == null
      ? output == 'refuse' || _outputs[event]!.contains(output)
      : applies.contains(output);
  num spent = 0;
  final fired = FiredEvent();
  var current = toolInput;
  String? rewriter;
  if (toolInput != null) fired.seen = [];
  for (final run in runs) {
    final source = run.source;
    HookOutput? out;
    String? failure;
    final eventTimeout = settings.hookEventTimeout;
    if (eventTimeout != null && spent >= eventTimeout) {
      failure = 'event-timeout';
    } else {
      fired.ran.add(source);
      fired.seen?.add((source, current));
      spent += run.seconds;
      try {
        out = hookResult(event, run, settings);
      } on HookFailed catch (error) {
        failure = error.failure;
      }
    }
    if (failure != null) {
      fired.reports.add(HookReport('hook-failed', [source, event, failure]));
      final refusal = _refusals[event];
      if (run.onFailure == 'refuse' && refusal != null) {
        out = HookOutput(decision: refusal);
      } else {
        continue;
      }
    }
    final output = out!;
    var decided = output.decision;
    if (decided == 'ask' && !can('ask')) {
      fired.reports.add(HookReport('hook-ask-denied', [source, event]));
      decided = 'deny';
    }
    if (decided != null && !can(decided)) {
      fired.reports.add(
        HookReport('hook-not-applied', [source, event, decided]),
      );
      decided = null;
    }
    var rewrite = output.input;
    final select = run.select;
    if (decided == 'allow' &&
        toolKind == 'other' &&
        select != null &&
        !select.contains('other')) {
      fired.reports.add(
        HookReport('hook-not-applied', [source, event, 'allow']),
      );
      decided = null;
      rewrite = null;
    }
    if (decided == 'allow' && rewrite != null) {
      if (can('input')) {
        if (toolInput != null) current = rewrite;
        rewriter = source;
      } else {
        fired.reports.add(
          HookReport('hook-not-applied', [source, event, 'input']),
        );
        decided = null;
      }
    }
    if (_rank(decided) > _rank(fired.decision)) {
      fired.decision = decided;
      fired.by = source;
      fired.reason = output.reason;
    }
    if (_rank(decided) == 3) {
      // The prompt, compaction, call or session does not happen: nothing else applies.
      fired.context = [];
      fired.env = [];
      break;
    }
    final context = output.context;
    if (context != null && context != '') {
      final total = contextUsed + encodeUtf8(context).length;
      if (can('context') && allows(settings.contextBytes, total)) {
        contextUsed = total;
        fired.context.add((source, context));
      } else {
        fired.reports.add(
          HookReport('hook-not-applied', [source, event, 'context']),
        );
      }
    }
    final env = output.env;
    if (env != null && env.isNotEmpty) {
      if (!can('env')) {
        fired.reports.add(
          HookReport('hook-not-applied', [source, event, 'env']),
        );
        continue;
      }
      for (final (name, value) in env) {
        // On Windows a name set under another case keeps the spelling it has.
        final at = fired.env.indexWhere(
          (held) =>
              held.$1 == name ||
              (windows && asciiUpper(held.$1) == asciiUpper(name)),
        );
        if (at != -1) {
          fired.env[at] = (fired.env[at].$1, value);
        } else {
          fired.env.add((name, value));
        }
      }
    }
  }
  fired.nonDefaults = [
    for (final (name, _) in fired.env)
      if (isSensitiveVariable(name)) ('env-sensitive', name),
  ]..sort(_comparePairs);
  if (rewriter != null && fired.seen != null) {
    if (fired.decision == 'allow') {
      fired.input = current as Map<String, Object?>;
    } else if (fired.decision == 'ask') {
      fired.reports.add(
        HookReport('hook-not-applied', [rewriter, event, 'input']),
      );
    }
  }
  return fired;
}
