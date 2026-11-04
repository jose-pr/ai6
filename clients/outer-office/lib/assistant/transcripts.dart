import 'dart:async';
import 'dart:convert';

import 'package:material_ui/material_ui.dart';

import 'export.dart' as exporting;

const _mono = TextStyle(fontFamily: 'monospace');

/// What the app reads of what was said and done in a Session: a Mission's Session context,
/// Moneypenny's conversation log and her harnesses' contexts. Universal Exports refuses what
/// the person may not read; another person's conversation needs `audit.read` and
/// `session.context`, and each such read is audited there.
abstract interface class TranscriptReads {
  /// A Mission's Session's context: `{session, format, source, filed_at, cut_from?, bytes,
  /// content}`.
  Future<Map<String, Object?>> sessionContext(String operation, String session);

  /// A conversation's log: `{conversation, person, entries}`.
  Future<Map<String, Object?>> conversationLog(String conversation);

  /// A conversation's contexts: `{conversation, person, contexts}`.
  Future<Map<String, Object?>> conversationContext(String conversation);

  /// The conversations in the log: the person's own, [person]'s, or [everyone]'s.
  Future<List<Map<String, Object?>>> conversations({
    String? person,
    bool everyone = false,
  });
}

/// One turn of a context as it is shown: who spoke or what happened, and its text. A tool's
/// result is [folded].
@immutable
class ContextTurn {
  const ContextTurn(this.who, this.text, {this.folded = false});

  /// `person`, `agent`, `tool`, `result` or `event`.
  final String who;
  final String text;
  final bool folded;
}

/// The turns of a context's [content], read by its [format]: Claude Code's transcript,
/// Codex's rollout, the Station's event stream, or any other JSON lines, each shown folded.
List<ContextTurn> contextTurns(String format, String content) {
  final turns = <ContextTurn>[];
  for (final line in const LineSplitter().convert(content)) {
    if (line.trim().isEmpty) continue;
    Object? parsed;
    try {
      parsed = jsonDecode(line);
    } on FormatException {
      turns.add(ContextTurn('event', line, folded: true));
      continue;
    }
    if (parsed is! Map<String, dynamic>) {
      turns.add(ContextTurn('event', line, folded: true));
      continue;
    }
    final read = switch (format) {
      'claude-code-jsonl' => _claudeTurns(parsed),
      'codex-rollout-jsonl' => _codexTurns(parsed),
      'ai6-events-jsonl' => _eventTurns(parsed),
      _ => null,
    };
    turns.addAll(read ?? [ContextTurn('event', line, folded: true)]);
  }
  return turns;
}

String _text(Object? value) => switch (value) {
  final String text => text,
  final List<dynamic> parts => parts.map(_text).join('\n'),
  final Map<String, dynamic> part => _text(
    part['text'] ?? part['content'] ?? part['output'],
  ),
  null => '',
  final other => jsonEncode(other),
};

/// A Claude Code transcript line: `{type: user|assistant, message: {content}}`.
List<ContextTurn>? _claudeTurns(Map<String, dynamic> line) {
  final type = line['type'];
  if (type != 'user' && type != 'assistant') return const [];
  final content = (line['message'] as Map<String, dynamic>?)?['content'];
  final who = type == 'user' ? 'person' : 'agent';
  if (content is String) return [ContextTurn(who, content)];
  if (content is! List<dynamic>) return const [];
  return [
    for (final block in content.whereType<Map<String, dynamic>>())
      switch (block['type']) {
        'text' => ContextTurn(who, _text(block['text'])),
        'tool_use' => ContextTurn(
          'tool',
          '${block['name']} ${jsonEncode(block['input'])}',
        ),
        'tool_result' => ContextTurn(
          'result',
          _text(block['content']),
          folded: true,
        ),
        'thinking' => ContextTurn(
          'event',
          _text(block['thinking']),
          folded: true,
        ),
        _ => ContextTurn('event', jsonEncode(block), folded: true),
      },
  ];
}

/// A Codex rollout line: `{type: response_item, payload: {type: message|function_call|...}}`.
List<ContextTurn>? _codexTurns(Map<String, dynamic> line) {
  if (line['type'] != 'response_item') return const [];
  final payload = line['payload'];
  if (payload is! Map<String, dynamic>) return null;
  return switch (payload['type']) {
    'message' => [
      ContextTurn(
        payload['role'] == 'user' ? 'person' : 'agent',
        _text(payload['content']),
      ),
    ],
    'function_call' || 'custom_tool_call' => [
      ContextTurn(
        'tool',
        '${payload['name']} ${payload['arguments'] ?? payload['input'] ?? ''}',
      ),
    ],
    'function_call_output' || 'custom_tool_call_output' => [
      ContextTurn('result', _text(payload['output']), folded: true),
    ],
    'reasoning' => const [],
    _ => [ContextTurn('event', jsonEncode(payload), folded: true)],
  };
}

/// An event the Station kept: `{kind, text, tools}`.
List<ContextTurn>? _eventTurns(Map<String, dynamic> line) {
  final kind = line['kind'] as String? ?? 'event';
  final text = line['text'] as String? ?? '';
  final tools = (line['tools'] as List<dynamic>? ?? const []).join(', ');
  return switch (kind) {
    'text' => [ContextTurn('agent', text)],
    'tool' => [ContextTurn('tool', tools.isEmpty ? text : '$tools $text')],
    _ => [ContextTurn('event', text.isEmpty ? kind : '$kind: $text')],
  };
}

/// One context: where it came from, its turns with each tool result folded, and Download for
/// the file as kept.
class ContextView extends StatelessWidget {
  const ContextView({
    required this.data,
    this.saveText = exporting.saveText,
    super.key,
  });

  /// The context as the gateway sends it.
  final Map<String, Object?> data;
  final Future<String?> Function(String name, String text) saveText;

  Future<void> _download(BuildContext context) async {
    final messenger = ScaffoldMessenger.maybeOf(context);
    final session = data['session'] as String? ?? 'session';
    try {
      final where = await saveText(
        'context-$session.jsonl',
        data['content'] as String? ?? '',
      );
      messenger?.showSnackBar(
        SnackBar(
          content: Text(where == null ? 'Context saved.' : 'Saved to $where'),
        ),
      );
    } on Object catch (error) {
      messenger?.showSnackBar(
        SnackBar(content: Text('The context was not saved: $error')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final format = data['format'] as String? ?? '';
    final turns = contextTurns(format, data['content'] as String? ?? '');
    final cut = data['cut_from'];
    final source = data['source'] == 'events'
        ? "The Station's event stream: this harness keeps no record it names."
        : "The harness's own record ($format).";
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'Session ${data['session'] ?? ''}',
                    style: theme.textTheme.titleSmall,
                  ),
                  Text(source, style: theme.textTheme.bodySmall),
                  if (cut != null)
                    Text(
                      'Only its newest part is kept; it was $cut bytes.',
                      style: theme.textTheme.bodySmall,
                    ),
                ],
              ),
            ),
            TextButton.icon(
              onPressed: () => unawaited(_download(context)),
              icon: const Icon(Icons.download_outlined),
              label: const Text('Download'),
            ),
          ],
        ),
        const SizedBox(height: 8),
        for (final turn in turns) _TurnView(turn: turn),
        if (turns.isEmpty) const Text('Nothing was kept.'),
      ],
    );
  }
}

class _TurnView extends StatelessWidget {
  const _TurnView({required this.turn});

  final ContextTurn turn;

  @override
  Widget build(BuildContext context) {
    final label = switch (turn.who) {
      'person' => 'Asked',
      'agent' => 'Agent',
      'tool' => 'Tool call',
      'result' => 'Tool result',
      _ => 'Event',
    };
    if (turn.folded) {
      return ExpansionTile(
        dense: true,
        tilePadding: EdgeInsets.zero,
        title: Text(
          '$label · ${turn.text.split('\n').first}',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: Text(turn.text, style: _mono),
          ),
        ],
      );
    }
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: Theme.of(context).textTheme.labelMedium),
          Text(turn.text, style: turn.who == 'tool' ? _mono : null),
        ],
      ),
    );
  }
}

/// Loads something to show, and shows it, a spinner, or why it could not be read.
class _Loaded<T> extends StatefulWidget {
  const _Loaded({required this.load, required this.builder, super.key});

  final Future<T> Function() load;
  final Widget Function(BuildContext context, T value) builder;

  @override
  State<_Loaded<T>> createState() => _LoadedState<T>();
}

class _LoadedState<T> extends State<_Loaded<T>> {
  late Future<T> _future = widget.load();

  @override
  void didUpdateWidget(covariant _Loaded<T> old) {
    super.didUpdateWidget(old);
    if (old.key != widget.key) _future = widget.load();
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<T>(
    future: _future,
    builder: (context, snapshot) {
      if (snapshot.hasError) {
        return Center(
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Text('It could not be read: ${snapshot.error}'),
          ),
        );
      }
      if (!snapshot.hasData) {
        return const Center(child: CircularProgressIndicator());
      }
      return widget.builder(context, snapshot.data as T);
    },
  );
}

/// A Mission's Session's context, loaded from [reads].
class SessionContextPanel extends StatelessWidget {
  const SessionContextPanel({
    required this.reads,
    required this.operation,
    required this.session,
    super.key,
  });

  final TranscriptReads reads;
  final String operation;
  final String session;

  @override
  Widget build(BuildContext context) => _Loaded<Map<String, Object?>>(
    key: ValueKey('$operation/$session'),
    load: () => reads.sessionContext(operation, session),
    builder: (context, loaded) => ListView(
      padding: const EdgeInsets.all(12),
      children: [ContextView(data: loaded)],
    ),
  );
}

/// One conversation with Moneypenny as the log keeps it, every entry with the turn that ran
/// it, and beside it the contexts of her harnesses' Sessions.
class ConversationLogPage extends StatelessWidget {
  const ConversationLogPage({
    required this.reads,
    required this.conversation,
    super.key,
  });

  final TranscriptReads reads;
  final String conversation;

  @override
  Widget build(BuildContext context) => DefaultTabController(
    length: 2,
    child: Scaffold(
      appBar: AppBar(
        title: const Text('Conversation log'),
        bottom: const TabBar(
          tabs: [
            Tab(text: 'Log'),
            Tab(text: 'Context'),
          ],
        ),
      ),
      body: TabBarView(
        children: [
          _Loaded<Map<String, Object?>>(
            load: () => reads.conversationLog(conversation),
            builder: (context, log) {
              final entries = (log['entries'] as List<dynamic>? ?? const [])
                  .whereType<Map<String, dynamic>>()
                  .toList();
              if (entries.isEmpty) {
                return const Center(child: Text('Nothing was logged.'));
              }
              return ListView(
                padding: const EdgeInsets.all(12),
                children: [for (final entry in entries) _LogEntry(entry)],
              );
            },
          ),
          _Loaded<Map<String, Object?>>(
            load: () => reads.conversationContext(conversation),
            builder: (context, loaded) {
              final contexts =
                  (loaded['contexts'] as List<dynamic>? ?? const [])
                      .whereType<Map<String, dynamic>>()
                      .toList();
              if (contexts.isEmpty) {
                return const Center(
                  child: Text('No Session context was filed for it.'),
                );
              }
              return ListView(
                padding: const EdgeInsets.all(12),
                children: [
                  for (final data in contexts)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 24),
                      child: ContextView(data: data),
                    ),
                ],
              );
            },
          ),
        ],
      ),
    ),
  );
}

class _LogEntry extends StatelessWidget {
  const _LogEntry(this.entry);

  final Map<String, dynamic> entry;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final kind = entry['kind'] as String? ?? '';
    final at = DateTime.fromMillisecondsSinceEpoch(
      ((entry['at'] as num? ?? 0) * 1000).round(),
      isUtc: true,
    ).toIso8601String();
    final turn = entry['turn'] as Map<String, dynamic>?;
    final route = turn?['route'] as Map<String, dynamic>?;
    final account = turn?['account'] as Map<String, dynamic>?;
    final ran = [
      if (turn?['station'] case final String station) 'Station $station',
      if (turn?['session'] case final String session) 'Session $session',
      if (route != null)
        'route ${route['handler']}'
            '${route['model'] == null ? '' : ' ${route['model']}'}'
            '${route['pool'] == null ? '' : ' (pool ${route['pool']})'}',
      if (account != null) 'account ${account['pool']}/${account['account']}',
      if (turn?['tokens'] case final num tokens) '$tokens tokens',
    ].join(' · ');
    final data = entry['data'];
    final title = '$kind · $at';
    final text = entry['text'] as String? ?? '';
    if (kind == 'tool_call' || kind == 'tool_result' || data != null) {
      return ExpansionTile(
        dense: true,
        tilePadding: EdgeInsets.zero,
        title: Text(
          text.isEmpty ? title : '$title · $text',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: ran.isEmpty ? null : Text(ran),
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: Text(
              const JsonEncoder.withIndent('  ').convert(data),
              style: _mono,
            ),
          ),
        ],
      );
    }
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title, style: theme.textTheme.labelMedium),
          if (ran.isNotEmpty) Text(ran, style: theme.textTheme.bodySmall),
          Text(text),
        ],
      ),
    );
  }
}

/// Conversations with Moneypenny in the log: the person's own, or, for one who holds
/// `audit.read` and `session.context`, another person's or everyone's.
class ConversationsPage extends StatefulWidget {
  const ConversationsPage({required this.reads, super.key});

  final TranscriptReads reads;

  @override
  State<ConversationsPage> createState() => _ConversationsPageState();
}

class _ConversationsPageState extends State<ConversationsPage> {
  final _whose = TextEditingController();
  bool _everyone = false;
  int _asked = 0;

  @override
  void dispose() {
    _whose.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final person = _whose.text.trim();
    return Scaffold(
      appBar: AppBar(title: const Text('Conversations with Moneypenny')),
      body: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.all(12),
            child: Wrap(
              spacing: 12,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                SizedBox(
                  width: 260,
                  child: TextField(
                    controller: _whose,
                    enabled: !_everyone,
                    decoration: const InputDecoration(
                      labelText: "Another person's ID",
                      helperText: 'Empty: your own. Their reads are audited.',
                    ),
                    onSubmitted: (_) => setState(() => _asked++),
                  ),
                ),
                FilterChip(
                  label: const Text('Everyone'),
                  selected: _everyone,
                  onSelected: (on) => setState(() {
                    _everyone = on;
                    _asked++;
                  }),
                ),
                FilledButton(
                  onPressed: () => setState(() => _asked++),
                  child: const Text('Show'),
                ),
              ],
            ),
          ),
          const Divider(height: 1),
          Expanded(
            child: _Loaded<List<Map<String, Object?>>>(
              key: ValueKey('$_asked/$_everyone/$person'),
              load: () => widget.reads.conversations(
                person: _everyone || person.isEmpty ? null : person,
                everyone: _everyone,
              ),
              builder: (context, listed) => listed.isEmpty
                  ? const Center(child: Text('No conversation is logged.'))
                  : ListView(
                      children: [
                        for (final conversation in listed)
                          ListTile(
                            title: Text('${conversation['conversation']}'),
                            subtitle: Text(
                              '${conversation['person']} · '
                              '${conversation['entries']} entries · last '
                              '${DateTime.fromMillisecondsSinceEpoch(((conversation['last_at'] as num? ?? 0) * 1000).round(), isUtc: true).toIso8601String()}',
                            ),
                            onTap: () => Navigator.of(context).push(
                              MaterialPageRoute<void>(
                                builder: (_) => ConversationLogPage(
                                  reads: widget.reads,
                                  conversation:
                                      conversation['conversation']! as String,
                                ),
                              ),
                            ),
                          ),
                      ],
                    ),
            ),
          ),
        ],
      ),
    );
  }
}
