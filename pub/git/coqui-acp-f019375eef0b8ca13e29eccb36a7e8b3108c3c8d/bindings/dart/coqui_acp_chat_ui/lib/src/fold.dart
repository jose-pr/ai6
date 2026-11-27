import 'package:coqui_acp_chat/coqui_acp_chat.dart';

import 'diff.dart';
import 'entry.dart';
import 'look.dart';

/// Whether [call] is done with and went as it should, so that a line can stand for it. A call that failed, or waits or runs, is
/// shown whole: that is what a person looks for.
bool _foldable(ToolCall call) => switch (call.status) {
  ToolStatus.done || ToolStatus.cancelled || ToolStatus.stopped => true,
  ToolStatus.failed || ToolStatus.pending || ToolStatus.running => false,
};

bool _isWork(ChatEntryData entry) => entry.role == MessageRole.tool || entry.role == MessageRole.thought;

/// [entries] as a transcript lays them out at [density]. [working] says whether the last turn is still going. [turns] are the
/// conversation's turns by id, which give a folded turn its true duration; without one the duration is the entries' own span.
///
/// - [ChatDensity.verbose]: every entry as it is.
/// - [ChatDensity.thinking]: a run of finished tool calls becomes a [BurstItem]; a failed one, or one still going, stays an entry.
/// - [ChatDensity.normal]: also, thoughts are left out, and a finished turn's work, from its first tool call or thought to its
///   last, becomes a [WorkItem] unless one of its tool calls failed or is still waiting. What the agent said after its last piece
///   of work, its answer, stays outside. A turn still going folds its work so far into a [WorkItem] with [WorkItem.running] set,
///   thoughts kept, up to the first call that waits, runs or failed, which stays whole after it.
List<TranscriptItem> foldTranscript(
  List<ChatEntryData> entries, {
  required ChatDensity density,
  bool working = false,
  bool showThinking = true,
  List<TurnSpan> turns = const [],
}) {
  if (density == ChatDensity.verbose) {
    return [
      for (var i = 0; i < entries.length; i++)
        EntryItem(entries[i], streaming: working && showThinking && i == entries.length - 1 && entries[i].role == MessageRole.thought),
    ];
  }
  final items = <TranscriptItem>[];
  var start = 0;
  while (start < entries.length) {
    // A turn is a message of the person and what follows it, up to the next one.
    var end = start + 1;
    while (end < entries.length && entries[end].role != MessageRole.user) {
      end++;
    }
    final turn = entries.sublist(start, end);
    final last = end >= entries.length;
    items.addAll(_foldTurn(turn, density, finished: !(last && working), showThinking: showThinking, turns: turns));
    start = end;
  }
  return items;
}

/// The id of the turn [turn]'s entries belong to, when one of them says.
int? _turnIdOf(List<ChatEntryData> turn) {
  for (final entry in turn) {
    final id = switch (entry) {
      ModelEntry(:final message) => message.turn,
      ChatEntry(:final turn) => turn,
      _ => null,
    };
    if (id != null) return id;
  }
  return null;
}

List<TranscriptItem> _foldTurn(
  List<ChatEntryData> turn,
  ChatDensity density, {
  required bool finished,
  required bool showThinking,
  required List<TurnSpan> turns,
}) {
  final normal = density == ChatDensity.normal;
  final hasHead = turn.first.role == MessageRole.user;
  final head = hasHead ? turn.first : null;
  final rest = hasHead ? turn.sublist(1) : turn;
  final out = <TranscriptItem>[?head == null ? null : EntryItem(head)];
  final streamingThought = !finished && showThinking && rest.isNotEmpty && rest.last.role == MessageRole.thought ? rest.last : null;
  final activeRest = streamingThought == null ? rest : rest.sublist(0, rest.length - 1);
  final first = activeRest.indexWhere(_isWork);
  final lastAt = activeRest.lastIndexWhere(_isWork);
  final id = _turnIdOf(turn);
  final span = id == null ? null : turns.where((t) => t.id == id).firstOrNull;
  if (normal && finished && first >= 0) {
    final range = activeRest.sublist(first, lastAt + 1);
    final calls = [for (final entry in range) ?entry.tool];
    if (calls.every(_foldable)) {
      final ended = span?.endedAt;
      final duration = ended == null ? _span(head?.at ?? range.first.at, range.last.at) : _span(span!.startedAt, ended);
      out
        ..addAll(_foldRun(activeRest.sublist(0, first), hideThoughts: true))
        ..add(WorkItem(_foldRun(range, hideThoughts: false), duration, turn: id))
        ..addAll(_foldRun(activeRest.sublist(lastAt + 1), hideThoughts: true));
      return out;
    }
  }
  if (normal && !finished && first >= 0) {
    // Up to the first call that is not done with; what comes after it is drawn as it is.
    var stop = first;
    while (stop < activeRest.length) {
      final tool = activeRest[stop].tool;
      if (activeRest[stop].role == MessageRole.tool && tool != null && !_foldable(tool)) break;
      stop++;
    }
    final range = activeRest.sublist(first, stop).takeWhileEnd(_isWork);
    final end = first + range.length;
    if (range.isNotEmpty) {
      out
        ..addAll(_foldRun(activeRest.sublist(0, first), hideThoughts: true))
        ..add(WorkItem(_foldRun(range, hideThoughts: false), Duration.zero, turn: id, running: true))
        ..addAll(_foldRun(activeRest.sublist(end), hideThoughts: true));
      if (streamingThought != null) out.add(EntryItem(streamingThought, streaming: true));
      return out;
    }
  }
  out.addAll(_foldRun(activeRest, hideThoughts: normal));
  if (streamingThought != null) out.add(EntryItem(streamingThought, streaming: true));
  return out;
}

extension on List<ChatEntryData> {
  /// This list without the trailing entries that are not [keep]: what the agent said after its last piece of work is not work.
  List<ChatEntryData> takeWhileEnd(bool Function(ChatEntryData) keep) {
    var end = length;
    while (end > 0 && !keep(this[end - 1])) {
      end--;
    }
    return sublist(0, end);
  }
}

Duration _span(DateTime from, DateTime to) {
  final span = to.difference(from);
  return span.isNegative ? Duration.zero : span;
}

/// A run of entries with consecutive foldable tool calls made into bursts.
List<TranscriptItem> _foldRun(List<ChatEntryData> entries, {required bool hideThoughts}) {
  final out = <TranscriptItem>[];
  final burst = <ChatEntryData>[];
  void flush() {
    if (burst.isEmpty) return;
    out.add(BurstItem(List.of(burst)));
    burst.clear();
  }

  for (final entry in entries) {
    final tool = entry.tool;
    if (entry.role == MessageRole.tool && tool != null && _foldable(tool)) {
      burst.add(entry);
      continue;
    }
    if (entry.role == MessageRole.thought && hideThoughts) continue;
    flush();
    out.add(EntryItem(entry));
  }
  flush();
  return out;
}

/// What a line says of a burst of tool calls, and what they added and removed.
typedef BurstSummary = ({String text, int added, int removed});

final Expando<({int added, int removed})> _counted = Expando('lines changed');

({int added, int removed}) _countsOf(ToolBlock block) {
  if (_counted[block] case final known?) return known;
  var added = 0, removed = 0;
  void count(Iterable<DiffLine?> lines) {
    for (final line in lines) {
      if (line?.kind == DiffKind.added) added++;
      if (line?.kind == DiffKind.removed) removed++;
    }
  }

  switch (block) {
    case ToolDiff(:final oldText, :final newText):
      // A final line break ends a line, it does not start an empty one.
      String whole(String? text) => text != null && text.endsWith('\n') ? text.substring(0, text.length - 1) : text ?? '';
      count(diffLines(oldText == null ? null : whole(oldText), whole(newText)));
    case ToolPatch(:final text):
      count(patchLines(text));
    case ToolText() || ToolTerminal():
      break;
  }
  return _counted[block] = (added: added, removed: removed);
}

String _base(String path) {
  final cut = path.split(RegExp(r'[\\/]')).where((part) => part.isNotEmpty).lastOrNull ?? path;
  // A location may carry a line: `calc.js:2`.
  return cut.replaceFirst(RegExp(r':\d+$'), '');
}

String _count(int n, String one, String many) => n == 1 ? one : '$n $many';

/// "Edited calc.js, ran a command" and its line counts, for [calls]: each kind of work once, in the order it first came, with the
/// files named when there are few. Calls a stopped turn never ran are counted at the end, "(1 stopped)".
BurstSummary summarizeBurst(List<ToolCall> calls) {
  final order = <String>[];
  final paths = <String, Set<String>>{};
  final counts = <String, int>{};
  var added = 0, removed = 0, stopped = 0;
  for (final call in calls) {
    final kind = switch (call.kind) {
      'edit' || 'delete' || 'move' || 'read' || 'search' || 'execute' || 'fetch' => call.kind!,
      _ => 'other',
    };
    if (!counts.containsKey(kind)) order.add(kind);
    counts[kind] = (counts[kind] ?? 0) + 1;
    final named = kind == 'edit' || kind == 'delete' || kind == 'move' ? call.changedPaths : (kind == 'read' ? call.locations : const <String>[]);
    (paths[kind] ??= {}).addAll([for (final path in named) _base(path)]);
    if (call.status != ToolStatus.done) stopped++;
    for (final block in call.blocks) {
      final seen = _countsOf(block);
      added += seen.added;
      removed += seen.removed;
    }
  }
  String files(String verb, String kind) {
    final names = paths[kind] ?? const {};
    if (names.isEmpty) return '$verb ${_count(counts[kind]!, 'a file', 'files')}';
    if (names.length <= 2) return '$verb ${names.join(', ')}';
    return '$verb ${names.length} files';
  }

  final phrases = [
    for (final kind in order)
      switch (kind) {
        'edit' => files('edited', kind),
        'delete' => files('deleted', kind),
        'move' => files('moved', kind),
        'read' => files('read', kind),
        'search' => 'searched${counts[kind]! > 1 ? ' ${counts[kind]} times' : ''}',
        'execute' => 'ran ${_count(counts[kind]!, 'a command', 'commands')}',
        'fetch' => 'fetched ${_count(counts[kind]!, 'a page', 'pages')}',
        _ => 'used ${_count(counts[kind]!, 'a tool', 'tools')}',
      },
  ];
  var text = phrases.join(', ');
  text = text.isEmpty ? text : '${text[0].toUpperCase()}${text.substring(1)}';
  if (stopped > 0) text = '$text ($stopped stopped)';
  return (text: text, added: added, removed: removed);
}

/// A span as "1m 29s", "42s" or "1h 5m".
String spanLabel(Duration span) {
  final seconds = span.inSeconds;
  if (seconds < 1) return 'under a second';
  if (seconds < 60) return '${seconds}s';
  if (seconds < 3600) return '${seconds ~/ 60}m ${seconds % 60}s';
  return '${seconds ~/ 3600}h ${(seconds % 3600) ~/ 60}m';
}
