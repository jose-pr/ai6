import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';

/// One entry of a memory, without its text.
@immutable
class MemorySummary {
  const MemorySummary({
    required this.id,
    required this.summary,
    required this.revision,
    required this.bytes,
    required this.updatedBy,
    required this.updatedAt,
    required this.writtenBy,
    this.name,
    this.exposures = const [],
    this.compartments = const [],
  });

  /// A generated UUID; names and summaries may repeat.
  final String id;
  final String? name;

  /// One line saying what it is for.
  final String summary;

  /// The revision a change to it names.
  final int revision;
  final int bytes;
  final String updatedBy;

  /// Unix seconds.
  final double updatedAt;

  /// Who wrote what it holds now.
  final String writtenBy;

  /// How what wrote it had read outside text; none means it had not.
  final List<String> exposures;

  /// It is visible only to those read into one of these.
  final List<String> compartments;

  bool get exposed => exposures.isNotEmpty;

  factory MemorySummary.fromJson(Map<String, dynamic> json) {
    final source = json['source'] as Map<String, dynamic>? ?? const {};
    return MemorySummary(
      id: json['id'] as String,
      name: json['name'] as String?,
      summary: json['summary'] as String? ?? '',
      revision: (json['revision'] as num?)?.toInt() ?? 1,
      bytes: (json['bytes'] as num?)?.toInt() ?? 0,
      updatedBy: json['updated_by'] as String? ?? '',
      updatedAt: (json['updated_at'] as num?)?.toDouble() ?? 0,
      writtenBy: source['by'] as String? ?? '',
      exposures: [
        for (final exposure
            in source['exposures'] as List<dynamic>? ?? const [])
          exposure as String,
      ],
      compartments: [
        for (final compartment
            in json['compartments'] as List<dynamic>? ?? const [])
          compartment as String,
      ],
    );
  }
}

/// One entry with its text.
@immutable
class MemoryRecord {
  const MemoryRecord({required this.summary, required this.content});

  final MemorySummary summary;
  final String content;

  factory MemoryRecord.fromJson(Map<String, dynamic> json) => MemoryRecord(
    summary: MemorySummary.fromJson(json),
    content: json['content'] as String? ?? '',
  );
}

/// What a listing or a search of a memory answers.
@immutable
class MemoryListing {
  const MemoryListing({required this.entries, required this.total});

  /// Newest change first, or best match first.
  final List<MemorySummary> entries;

  /// How many there are, of which [entries] may be the first.
  final int total;

  factory MemoryListing.fromJson(Map<String, dynamic> json) {
    final entries = [
      for (final entry in json['entries'] as List<dynamic>? ?? const [])
        MemorySummary.fromJson(entry as Map<String, dynamic>),
    ];
    return MemoryListing(
      entries: entries,
      total: (json['total'] as num?)?.toInt() ?? entries.length,
    );
  }

  /// Sample memory for the preview build.
  factory MemoryListing.demo() => const MemoryListing(
    total: 2,
    entries: [
      MemorySummary(
        id: '6f1e4a6a-9c55-4a0c-9d5c-2f8a0b6c1e11',
        name: 'prefs',
        summary: 'How the operator likes answers',
        revision: 2,
        bytes: 58,
        updatedBy: 'moneypenny:operator',
        updatedAt: 1760000000,
        writtenBy: 'moneypenny:operator',
      ),
      MemorySummary(
        id: '0c8b7a35-5a4b-4d7a-8f50-9d2c6e1f7a22',
        summary: 'Who is on call this quarter',
        revision: 1,
        bytes: 41,
        updatedBy: 'person:operator',
        updatedAt: 1759000000,
        writtenBy: 'person:operator',
      ),
    ],
  );
}

/// Universal Exports refused a memory request; [message] is its own words.
class MemoryRefused implements Exception {
  const MemoryRefused(this.message);

  final String message;

  @override
  String toString() => message;
}

/// One line saying who wrote [entry] and what it was marked with.
String memorySource(MemorySummary entry) {
  final written = 'Written by ${entry.writtenBy}';
  return entry.exposed
      ? '$written, after reading outside text (${entry.exposures.join(', ')})'
      : written;
}

/// What Moneypenny remembers about the signed-in person: rows she and the person keep, searched,
/// opened, edited and forgotten through Universal Exports. No repository or Station is involved.
class MemoryPage extends StatefulWidget {
  const MemoryPage({required this.board, super.key});

  final MissionBoard board;

  @override
  State<MemoryPage> createState() => _MemoryPageState();
}

class _MemoryPageState extends State<MemoryPage> {
  final _search = TextEditingController();
  MemoryListing? _listing;
  String? _error;
  bool _loading = false;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    setState(() => _loading = true);
    try {
      final query = _search.text.trim();
      final listing = await widget.board.loadMemory(
        query: query.isEmpty ? null : query,
      );
      if (!mounted) return;
      setState(() {
        _listing = listing;
        _error = null;
      });
    } on Object catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  void _say(String text) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _open(MemorySummary? entry) async {
    final changed = await Navigator.of(context).push<String>(
      MaterialPageRoute(
        builder: (_) => MemoryEditorPage(board: widget.board, entry: entry),
      ),
    );
    if (changed == null || !mounted) return;
    _say(changed);
    await _load();
  }

  Future<void> _forget(MemorySummary entry) async {
    if (!await confirmForget(context, entry.summary) || !mounted) return;
    try {
      await widget.board.forgetMemory(entry.id);
      if (!mounted) return;
      _say('Forgotten: ${entry.summary}.');
      await _load();
    } on Object catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  @override
  Widget build(BuildContext context) {
    final listing = _listing;
    final small = Theme.of(context).textTheme.bodySmall;
    final searching = _search.text.trim().isNotEmpty;
    return Scaffold(
      appBar: AppBar(
        title: const Text("Moneypenny's memory"),
        actions: [
          IconButton(
            tooltip: 'Refresh',
            onPressed: _loading ? null : () => unawaited(_load()),
            icon: const Icon(Icons.refresh),
          ),
        ],
      ),
      floatingActionButton: widget.board.demo
          ? null
          : FloatingActionButton.extended(
              onPressed: () => unawaited(_open(null)),
              icon: const Icon(Icons.add),
              label: const Text('New entry'),
            ),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 16, 16, 96),
        children: [
          const Text(
            'What Moneypenny remembers about you. She starts each conversation with these '
            'summaries, reads an entry when she needs it, and shows you each change she makes. '
            'You can correct or forget anything here.',
          ),
          const SizedBox(height: 12),
          TextField(
            key: const Key('memory-search'),
            controller: _search,
            decoration: const InputDecoration(
              labelText: 'Search',
              prefixIcon: Icon(Icons.search),
            ),
            onSubmitted: (_) => unawaited(_load()),
          ),
          if (_error case final error?)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: Text(
                revealInvisible(error),
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            ),
          const SizedBox(height: 12),
          if (listing == null)
            Center(
              child: _loading
                  ? const CircularProgressIndicator()
                  : const Text('Her memory is unavailable.'),
            )
          else if (listing.entries.isEmpty)
            Text(
              searching
                  ? 'Nothing she keeps matches that.'
                  : 'She remembers nothing about you yet.',
            )
          else ...[
            for (final entry in listing.entries)
              Card(
                child: ListTile(
                  leading: Icon(
                    entry.exposed
                        ? Icons.warning_amber_outlined
                        : Icons.description_outlined,
                  ),
                  title: Text(revealInvisible(entry.summary)),
                  subtitle: Text(
                    [
                      if (entry.name case final name?) revealInvisible(name),
                      memorySource(entry),
                      '${entry.bytes} bytes',
                    ].join(' · '),
                  ),
                  onTap: () => unawaited(_open(entry)),
                  trailing: widget.board.demo
                      ? null
                      : IconButton(
                          tooltip: 'Forget ${entry.summary}',
                          onPressed: () => unawaited(_forget(entry)),
                          icon: const Icon(Icons.delete_outline),
                        ),
                ),
              ),
            if (listing.total > listing.entries.length)
              Text(
                'The best ${listing.entries.length} of ${listing.total}; search for more.',
                style: small,
              ),
          ],
        ],
      ),
    );
  }
}

/// Asks whether to forget the entry [what]; true when the person confirms.
Future<bool> confirmForget(BuildContext context, String what) async =>
    await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Forget "$what"?'),
        content: const Text(
          'It goes for good, with its earlier versions. A conversation she is in now keeps '
          'what it already read.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Forget'),
          ),
        ],
      ),
    ) ??
    false;

/// One entry to read and edit by a form, or a new one with no [entry]. A save names the revision
/// that was opened, so an entry changed meanwhile is refused rather than replaced. Pops with what
/// was done.
class MemoryEditorPage extends StatefulWidget {
  const MemoryEditorPage({required this.board, this.entry, super.key});

  final MissionBoard board;

  /// The entry opened; `null` for a new one.
  final MemorySummary? entry;

  @override
  State<MemoryEditorPage> createState() => _MemoryEditorPageState();
}

class _MemoryEditorPageState extends State<MemoryEditorPage> {
  final _name = TextEditingController();
  final _summary = TextEditingController();
  final _content = TextEditingController();
  MemorySummary? _opened;
  String? _problem;
  late bool _loading = widget.entry != null;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    if (widget.entry != null) unawaited(_read());
  }

  @override
  void dispose() {
    _name.dispose();
    _summary.dispose();
    _content.dispose();
    super.dispose();
  }

  Future<void> _read() async {
    try {
      final record = await widget.board.readMemory(widget.entry!.id);
      if (!mounted) return;
      setState(() {
        _opened = record.summary;
        _name.text = record.summary.name ?? '';
        _summary.text = record.summary.summary;
        _content.text = record.content;
      });
    } on Object catch (error) {
      if (mounted) setState(() => _problem = '$error');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _save() async {
    setState(() {
      _saving = true;
      _problem = null;
    });
    try {
      final name = _name.text.trim();
      await widget.board.saveMemory(
        id: widget.entry?.id,
        revision: _opened?.revision,
        name: name.isEmpty ? null : name,
        summary: _summary.text.trim(),
        content: _content.text,
      );
      if (mounted) Navigator.of(context).pop('Saved ${_summary.text.trim()}.');
    } on Object catch (error) {
      if (mounted) setState(() => _problem = '$error');
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  Future<void> _forget() async {
    final what = _opened?.summary ?? widget.entry!.summary;
    if (!await confirmForget(context, what) || !mounted) return;
    setState(() {
      _saving = true;
      _problem = null;
    });
    try {
      await widget.board.forgetMemory(widget.entry!.id);
      if (mounted) Navigator.of(context).pop('Forgotten: $what.');
    } on Object catch (error) {
      if (mounted) setState(() => _problem = '$error');
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final busy = _loading || _saving;
    final readOnly = widget.board.demo;
    final opened = _opened;
    final error = Theme.of(context).colorScheme.error;
    return Scaffold(
      appBar: AppBar(
        title: Text(widget.entry == null ? 'New entry' : 'Memory entry'),
        actions: [
          if (widget.entry != null && !readOnly)
            IconButton(
              tooltip: 'Forget this entry',
              onPressed: busy ? null : () => unawaited(_forget()),
              icon: const Icon(Icons.delete_outline),
            ),
        ],
      ),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 12,
          children: [
            if (_loading) const LinearProgressIndicator(),
            if (_problem case final problem?)
              Text(revealInvisible(problem), style: TextStyle(color: error)),
            if (opened != null)
              Text(
                '${memorySource(opened)}. Last changed by ${opened.updatedBy}, revision '
                '${opened.revision}.',
                key: const Key('memory-source'),
                style: opened.exposed
                    ? TextStyle(color: error)
                    : Theme.of(context).textTheme.bodySmall,
              ),
            TextField(
              key: const Key('memory-summary'),
              controller: _summary,
              enabled: !busy && !readOnly,
              decoration: const InputDecoration(
                labelText: 'Summary',
                helperText:
                    'One line saying what it is for; she sees it first.',
                border: OutlineInputBorder(),
              ),
            ),
            TextField(
              key: const Key('memory-name'),
              controller: _name,
              enabled: !busy && !readOnly,
              decoration: const InputDecoration(
                labelText: 'Name (optional)',
                border: OutlineInputBorder(),
              ),
            ),
            Expanded(
              child: TextField(
                key: const Key('memory-text'),
                controller: _content,
                enabled: !busy && !readOnly,
                expands: true,
                maxLines: null,
                textAlignVertical: TextAlignVertical.top,
                decoration: const InputDecoration(
                  labelText: 'Content',
                  alignLabelWithHint: true,
                  border: OutlineInputBorder(),
                  hintText: 'What Moneypenny should remember',
                ),
              ),
            ),
            if (!readOnly)
              Align(
                alignment: Alignment.centerRight,
                child: FilledButton.icon(
                  onPressed: busy ? null : () => unawaited(_save()),
                  icon: const Icon(Icons.save_outlined),
                  label: Text(_saving ? 'Saving…' : 'Save'),
                ),
              ),
          ],
        ),
      ),
    );
  }
}
