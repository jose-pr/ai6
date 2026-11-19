import 'dart:collection';
import 'dart:convert';

import 'limits.dart';
import 'text.dart';

// What an agent reports beyond its messages. All of it comes from the agent, so every field is checked for its type and bounded by
// the chat's ChatLimits: a wrong one is ignored rather than trusted, and an agent cannot grow this without limit.

/// How far a tool call has got. [cancelled] is ACP v2's: a tool a stopped turn never ran. [stopped] is this model's: a tool that
/// was waiting for an answer or running when the connection to its agent ended, so nothing can answer it now.
enum ToolStatus { pending, running, done, failed, cancelled, stopped }

ToolStatus? _toolStatus(Object? value) => switch (value) {
  'pending' => ToolStatus.pending,
  'in_progress' => ToolStatus.running,
  'completed' => ToolStatus.done,
  'failed' => ToolStatus.failed,
  'cancelled' => ToolStatus.cancelled,
  _ => null,
};

/// The wire word of [status], for replaying a tool call. [ToolStatus.stopped] goes as `cancelled`.
String toolStatusWord(ToolStatus status) => switch (status) {
  ToolStatus.pending => 'pending',
  ToolStatus.running => 'in_progress',
  ToolStatus.done => 'completed',
  ToolStatus.failed => 'failed',
  ToolStatus.cancelled || ToolStatus.stopped => 'cancelled',
};

/// One piece of a tool call's content.
sealed class ToolBlock {
  const ToolBlock();
}

/// Plain text the tool produced.
final class ToolText extends ToolBlock {
  const ToolText(this.text);
  final String text;
}

/// Whole-file text before and after. No [oldText] is a new file.
final class ToolDiff extends ToolBlock {
  const ToolDiff({required this.path, required this.newText, this.oldText});
  final String path;
  final String? oldText;
  final String newText;
}

/// A change an agent already wrote as a unified patch.
final class ToolPatch extends ToolBlock {
  const ToolPatch(this.text, {this.paths = const []});
  final String text;

  /// The files it changes: those the agent listed with it, or else those its `+++` lines name.
  final List<String> paths;
}

/// A command's output, which the session keeps under [terminalId] in [ChatSession.terminals].
final class ToolTerminal extends ToolBlock {
  const ToolTerminal(this.terminalId);
  final String terminalId;
}

/// The files a v2 diff lists in its `changes`, or else those a patch's `+++` lines name (`b/` taken off, `/dev/null` left out).
List<String> _patchPaths(Object? changes, String text, ChatLimits limits) {
  if (changes is List) {
    final listed = [
      for (final change in changes.take(limits.locations))
        if (change is Map) ?shortText(change['path'], limits),
    ];
    if (listed.isNotEmpty) return listed;
  }
  return [
    for (final line
        in text
            .split('\n')
            .where((line) => line.startsWith('+++ '))
            .take(limits.locations))
      if (line.substring(4).trim() case final named when named != '/dev/null')
        ?shortText(named.startsWith('b/') ? named.substring(2) : named, limits),
  ];
}

ToolBlock? _toolBlock(Object? raw, ChatLimits limits) {
  if (raw is! Map) return null;
  final type = raw['type'];
  if (type == 'content') {
    final content = raw['content'];
    if (content is Map &&
        content['type'] == 'text' &&
        content['text'] is String) {
      return ToolText(boundedHead(content['text'] as String, limits.toolText));
    }
    return null;
  }
  if (type == 'diff' && raw['newText'] is String) {
    return ToolDiff(
      path: shortText(raw['path'], limits) ?? 'a file',
      oldText: raw['oldText'] is String
          ? boundedHead(raw['oldText'] as String, limits.toolText)
          : null,
      newText: boundedHead(raw['newText'] as String, limits.toolText),
    );
  }
  if (type == 'terminal' && idText(raw['terminalId'], limits) != null) {
    return ToolTerminal(raw['terminalId'] as String);
  }
  final patch = raw['patch'];
  if (patch is Map && patch['text'] is String) {
    final text = boundedHead(patch['text'] as String, limits.toolText);
    return ToolPatch(text, paths: _patchPaths(raw['changes'], text, limits));
  }
  return null;
}

/// One tool the agent is using, built up from however many updates it sends: a field an update leaves out keeps its value.
final class ToolCall {
  ToolCall(this.id);
  final String id;
  String _title = 'A tool';
  String? _kind;
  ToolStatus _status = ToolStatus.pending;
  List<ToolBlock> _blocks = const [];
  List<String> _locations = const [];

  String get title => _title;
  String? get kind => _kind;
  ToolStatus get status => _status;
  List<ToolBlock> get blocks => UnmodifiableListView(_blocks);

  /// Where it works, as `path` or `path:line`.
  List<String> get locations => UnmodifiableListView(_locations);

  /// Marks a call that was waiting or running as stopped: nothing can finish it now. A call that has finished stays as it was.
  void stop() {
    if (isOpen) _status = ToolStatus.stopped;
  }

  /// Whether it is waiting or running, which a connection that ended leaves for [ToolStatus.stopped].
  bool get isOpen =>
      status == ToolStatus.pending || status == ToolStatus.running;

  /// The files its diffs and patches change, each once, in the order they came.
  List<String> get changedPaths => {
    for (final block in blocks)
      ...switch (block) {
        ToolDiff(:final path) => [path],
        ToolPatch(:final paths) => paths,
        _ => const <String>[],
      },
  }.toList();

  void apply(Map<String, dynamic> raw, ChatLimits limits) {
    _title =
        shortText(raw['title'], limits) ??
        (_title == 'A tool'
            ? shortText(raw['name'], limits) ?? _title
            : _title);
    _kind = shortText(raw['kind'], limits) ?? _kind;
    _status = _toolStatus(raw['status']) ?? _status;
    final content = raw['content'];
    if (raw['sessionUpdate'] == 'tool_call_content_chunk') {
      // One more item after what came before; past the limit the oldest goes.
      final block = _toolBlock(content, limits);
      if (block != null) {
        final all = [..._blocks, block];
        _blocks = all.length > limits.toolBlocks
            ? all.sublist(all.length - limits.toolBlocks)
            : all;
      }
    } else if (content is List) {
      final parsed = content
          .take(limits.toolBlocks)
          .map((block) => _toolBlock(block, limits))
          .whereType<ToolBlock>()
          .toList();
      // An update with an empty list is how one starts, not a request to forget what came before.
      if (parsed.isNotEmpty) _blocks = parsed;
    }
    final places = raw['locations'];
    if (places is List && places.isNotEmpty) {
      _locations = places
          .take(limits.locations)
          .whereType<Map>()
          .map((place) {
            final path = shortText(place['path'], limits);
            if (path == null) return null;
            return place['line'] is int ? '$path:${place['line']}' : path;
          })
          .whereType<String>()
          .toList();
    }
  }

  /// This call as the update that makes it, for a client that attaches late.
  Map<String, Object?> toUpdate() => {
    'sessionUpdate': 'tool_call',
    'toolCallId': id,
    'title': title,
    'kind': ?kind,
    'status': toolStatusWord(status),
    'content': [
      for (final block in blocks)
        switch (block) {
          ToolText(:final text) => {
            'type': 'content',
            'content': {'type': 'text', 'text': text},
          },
          ToolDiff(:final path, :final oldText, :final newText) => {
            'type': 'diff',
            'path': path,
            'oldText': ?oldText,
            'newText': newText,
          },
          ToolPatch(:final text, :final paths) => {
            'patch': {'text': text},
            'changes': [
              for (final path in paths) {'path': path},
            ],
          },
          ToolTerminal(:final terminalId) => {
            'type': 'terminal',
            'terminalId': terminalId,
          },
        },
    ],
    'locations': [for (final place in locations) _locationJson(place)],
  };
}

Map<String, Object?> _locationJson(String place) {
  final at = place.lastIndexOf(':');
  final line = at < 0 ? null : int.tryParse(place.substring(at + 1));
  return line == null
      ? {'path': place}
      : {'path': place.substring(0, at), 'line': line};
}

/// Terminal output as ACP v2 sends it, base64-encoded bytes, as text. What is not base64 is shown as it came.
String terminalText(String data) {
  try {
    return utf8.decode(base64.decode(data), allowMalformed: true);
  } on FormatException {
    return data;
  }
}

/// A command the agent ran, as it reports it. The output keeps its end, which is where a failure says why.
final class TerminalView {
  TerminalView(ChatLimits limits)
    : _output = BoundedText.tail(limits.terminalOutput);

  final BoundedText _output;
  String? _command;
  int? _exitCode;

  String? get command => _command;
  String get output => _output.value;
  int? get exitCode => _exitCode;

  /// Whether the command has not reported that it ended.
  bool get isRunning => _exitCode == null;

  /// A `terminal_update`, whose output replaces what came before, or a `terminal_output_chunk`, which adds to it.
  void apply(Map<String, dynamic> raw, ChatLimits limits) {
    _command = shortText(raw['command'], limits) ?? _command;
    final said = raw['output'];
    if (said is Map && said['data'] is String) {
      _output.set(terminalText(said['data'] as String));
    }
    if (raw['sessionUpdate'] == 'terminal_output_chunk' &&
        raw['data'] is String) {
      _output.append(terminalText(raw['data'] as String));
    }
    final exit = raw['exitStatus'];
    if (exit is Map && exit['exitCode'] is int) {
      _exitCode = exit['exitCode'] as int;
    }
  }

  Map<String, Object?> toUpdate(String id) => {
    'sessionUpdate': 'terminal_update',
    'terminalId': id,
    'command': ?_command,
    'output': {'data': base64.encode(utf8.encode(output))},
    if (_exitCode != null) 'exitStatus': {'exitCode': _exitCode},
  };
}

/// How far a plan entry has got.
enum PlanStatus { pending, running, done, cancelled }

/// One step of the agent's plan, which a chat shows as a checklist.
final class PlanEntry {
  const PlanEntry(this.content, this.status);
  final String content;
  final PlanStatus status;
}

List<PlanEntry>? parsePlan(Object? raw, ChatLimits limits) {
  if (raw is! List) return null;
  return raw
      .take(limits.planEntries)
      .whereType<Map>()
      .map((entry) {
        final content = shortText(entry['content'], limits);
        if (content == null) return null;
        final status = switch (entry['status']) {
          'in_progress' => PlanStatus.running,
          'completed' => PlanStatus.done,
          'cancelled' => PlanStatus.cancelled,
          _ => PlanStatus.pending,
        };
        return PlanEntry(content, status);
      })
      .whereType<PlanEntry>()
      .toList();
}

/// The wire word of a plan entry's status.
String planStatusWord(PlanStatus status) => switch (status) {
  PlanStatus.pending => 'pending',
  PlanStatus.running => 'in_progress',
  PlanStatus.done => 'completed',
  PlanStatus.cancelled => 'cancelled',
};

/// The entries of a v2 `plan_update`'s plan, when it is a list of items; null for a plan content type this reader does not know.
Object? planItems(Object? plan) =>
    plan is Map && plan['type'] == 'items' ? plan['entries'] : null;

/// How far a compaction of the agent's context has got.
enum CompactionStatus { running, done, failed, cancelled }

String _texts(Iterable<Object?> blocks) => blocks
    .whereType<Map>()
    .where((block) => block['type'] == 'text' && block['text'] is String)
    .map((block) => block['text'])
    .join();

/// One compaction of the agent's context, built up from its `compaction_update`s and `compaction_summary_chunk`s.
final class Compaction {
  Compaction(this.id, ChatLimits limits)
    : _summary = BoundedText.head(limits.toolText);
  final String id;
  CompactionStatus _status = CompactionStatus.running;
  final BoundedText _summary;
  String? _error;

  CompactionStatus get status => _status;
  String get summary => _summary.value;
  String? get error => _error;

  void apply(Map<String, dynamic> raw, ChatLimits limits) {
    if (raw['sessionUpdate'] == 'compaction_summary_chunk') {
      _summary.append(_texts([raw['content']]));
      return;
    }
    final now = switch (raw['status']) {
      'in_progress' => CompactionStatus.running,
      'completed' => CompactionStatus.done,
      'failed' => CompactionStatus.failed,
      'cancelled' => CompactionStatus.cancelled,
      _ => null,
    };
    _status = now ?? _status;
    // Both are patches: a field left out keeps its value, and null clears it.
    if (raw.containsKey('summary')) {
      final said = raw['summary'];
      _summary.set(said is List ? _texts(said) : '');
    }
    if (raw.containsKey('error')) _error = shortText(raw['error'], limits);
  }

  Map<String, Object?> toUpdate() => {
    'sessionUpdate': 'compaction_update',
    'compactionId': id,
    'status': switch (status) {
      CompactionStatus.running => 'in_progress',
      CompactionStatus.done => 'completed',
      CompactionStatus.failed => 'failed',
      CompactionStatus.cancelled => 'cancelled',
    },
    'summary': [
      if (summary.isNotEmpty) {'type': 'text', 'text': summary},
    ],
    'error': _error,
  };
}

/// The names of what went with a person's message, from its content: one block, as a chunk has, or a list of them.
List<String> attachedNames(Object? content, ChatLimits limits) {
  final blocks = content is List ? content.take(limits.toolBlocks) : [content];
  return [
    for (final block in blocks.whereType<Map>())
      ?switch (block['type']) {
        'resource' =>
          _nameOf(
                block['resource'] is Map
                    ? (block['resource'] as Map)['uri']
                    : null,
                limits,
              ) ??
              'a file',
        'resource_link' =>
          shortText(block['name'], limits) ??
              _nameOf(block['uri'], limits) ??
              'a file',
        'image' => _nameOf(block['uri'], limits) ?? 'an image',
        'audio' => 'a recording',
        _ => null,
      },
  ];
}

/// The last part of [uri]'s path, which is the file's name for `attachment:notes.txt` and `file:///work/notes.txt` alike.
String? _nameOf(Object? uri, ChatLimits limits) {
  if (uri is! String) return null;
  final parsed = Uri.tryParse(uri);
  final segments =
      parsed?.pathSegments.where((segment) => segment.isNotEmpty) ??
      const <String>[];
  return shortText(segments.isEmpty ? uri : segments.last, limits);
}

/// A slash command the agent offers.
final class AgentCommandInfo {
  const AgentCommandInfo(this.name, this.description, {this.hint});
  final String name;
  final String description;

  /// What follows the name, such as `[period]`, when the agent says (ACP's `input.hint`).
  final String? hint;
}

List<AgentCommandInfo>? parseCommands(Object? raw, ChatLimits limits) {
  if (raw is! List) return null;
  return raw
      .take(limits.commands)
      .whereType<Map>()
      .map((entry) {
        final name = shortText(entry['name'], limits);
        return name == null
            ? null
            : AgentCommandInfo(
                name,
                shortText(entry['description'], limits) ?? '',
                hint: switch (entry['input']) {
                  final Map input => shortText(input['hint'], limits),
                  _ => null,
                },
              );
      })
      .whereType<AgentCommandInfo>()
      .toList();
}

/// How much of the agent's context is used, and what the session has cost, as the agent reports them.
final class SessionUsage {
  const SessionUsage({this.used, this.size, this.cost, this.currency});
  final int? used;
  final int? size;
  final double? cost;
  final String? currency;

  SessionUsage applied(Map<String, dynamic> raw, ChatLimits limits) {
    final said = raw['cost'];
    final amount =
        said is Map && said['amount'] is num && (said['amount'] as num).isFinite
        ? (said['amount'] as num).toDouble()
        : null;
    return SessionUsage(
      used: _whole(raw['used']) ?? used,
      size: _whole(raw['size']) ?? size,
      cost: amount ?? cost,
      currency: said is Map
          ? shortText(said['currency'], limits) ?? currency
          : currency,
    );
  }

  /// Whether the agent has said anything of it.
  bool get isEmpty => used == null && size == null && cost == null;

  /// How much of the context is used, from 0 to 100; null when the agent gave no size.
  int? get percent => used != null && size != null && size! > 0
      ? (100 * used! / size!).clamp(0, 100).round()
      : null;

  Map<String, Object?> toUpdate() => {
    'sessionUpdate': 'usage_update',
    'used': ?used,
    'size': ?size,
    if (cost != null) 'cost': {'amount': cost, 'currency': ?currency},
  };
}

final _hidden = RegExp(
  r'[\u{0}-\u{8}\u{b}-\u{1f}\u{7f}-\u{9f}\u{200e}\u{200f}\u{2028}-\u{202e}\u{2066}-\u{2069}]',
  unicode: true,
);

/// Text a person is about to approve: nothing in it can reorder or hide the rest, and each line break can be seen.
String _plainly(String text) => text
    .replaceAll('\r\n', '\n')
    .replaceAll(_hidden, ' ')
    .replaceAll('\n', '⏎\n');

/// What the person sees of what a tool is about to do. Without this, a card can only repeat the title an agent chose. All of what
/// it was given shows, the part that decides it first: a value left out is a value they did not approve.
String? describeToolInput(Object? raw, ChatLimits limits) {
  if (raw is! Map) return null;
  const first = ['command', 'file_path', 'path', 'url', 'pattern', 'query'];
  final keys = [
    ...first.where(raw.containsKey),
    ...raw.keys.whereType<String>().where((key) => !first.contains(key)),
  ];
  final lines = <String>[];
  for (final key in keys) {
    final value = raw[key];
    if (value is String && value.isNotEmpty) {
      lines.add(
        first.contains(key) && lines.isEmpty
            ? _plainly(value)
            : '$key: ${_plainly(value)}',
      );
    } else if (value is num || value is bool) {
      lines.add('$key: $value');
    } else if (value is List || value is Map) {
      lines.add('$key: ${_plainly(jsonEncode(value))}');
    }
  }
  return lines.isEmpty
      ? null
      : boundedHead(lines.join('\n'), limits.describedInput);
}

/// The change an edit would make, for a card that shows it: the diff in [toolCall]'s content, else the old and new strings of an
/// edit's input. Null when the request carries neither.
ToolDiff? describeToolDiff(
  Map<String, dynamic>? toolCall,
  Object? rawInput,
  ChatLimits limits,
) {
  final content = toolCall?['content'];
  if (content is List) {
    for (final item in content) {
      if (item is Map && item['type'] == 'diff') {
        if (_toolBlock(item, limits) case final ToolDiff diff) return diff;
      }
    }
  }
  if (rawInput is Map &&
      rawInput['old_string'] is String &&
      rawInput['new_string'] is String) {
    return ToolDiff(
      path: shortText(rawInput['file_path'] ?? rawInput['path'], limits) ?? '',
      oldText: boundedHead(rawInput['old_string'] as String, limits.toolText),
      newText: boundedHead(rawInput['new_string'] as String, limits.toolText),
    );
  }
  return null;
}

int? _whole(Object? value) =>
    value is num &&
        value.isFinite &&
        value.abs() < 9007199254740992 &&
        value == value.truncate()
    ? value.toInt()
    : null;
