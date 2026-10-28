import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';

/// One file of Moneypenny's memory of the signed-in person.
@immutable
class MemoryFileSummary {
  const MemoryFileSummary({required this.path, required this.bytes});

  /// Relative to `memory/` in the person's personal repository.
  final String path;
  final int bytes;

  factory MemoryFileSummary.fromJson(Map<String, dynamic> json) =>
      MemoryFileSummary(
        path: json['path'] as String,
        bytes: (json['bytes'] as num).toInt(),
      );
}

/// The files of `memory/` as one commit of the repository holds them.
@immutable
class MemoryListing {
  const MemoryListing({required this.files, this.commit, this.more = false});

  final List<MemoryFileSummary> files;

  /// The commit they were read at; `null` before the first.
  final String? commit;

  /// There are more files than the listing names.
  final bool more;

  factory MemoryListing.fromJson(Map<String, dynamic> json) => MemoryListing(
    files: [
      for (final file in (json['files'] as List<dynamic>? ?? const []))
        MemoryFileSummary.fromJson(file as Map<String, dynamic>),
    ],
    commit: json['commit'] as String?,
    more: json['more'] as bool? ?? false,
  );

  /// Sample memory for the preview build.
  factory MemoryListing.demo() => const MemoryListing(
    commit: 'c0ffee1',
    files: [
      MemoryFileSummary(path: 'MEMORY.md', bytes: 96),
      MemoryFileSummary(path: 'preferences.md', bytes: 41),
    ],
  );
}

/// One memory file's text, and the digest a save of it is made from.
@immutable
class MemoryText {
  const MemoryText({required this.path, required this.text, this.digest});

  final String path;
  final String text;
  final String? digest;

  factory MemoryText.fromJson(Map<String, dynamic> json) => MemoryText(
    path: json['path'] as String,
    text: json['text'] as String? ?? '',
    digest: json['digest'] as String?,
  );
}

/// Universal Exports refused a memory request; [message] is its own words.
class MemoryRefused implements Exception {
  const MemoryRefused(this.message);

  final String message;

  @override
  String toString() => message;
}

/// What Moneypenny remembers about the signed-in person: the files of `memory/` in their
/// personal repository, each opened, edited and saved, or forgotten, through Universal
/// Exports. Every save and every "forget" is a commit by the person.
class MemoryPage extends StatefulWidget {
  const MemoryPage({required this.board, super.key});

  final MissionBoard board;

  @override
  State<MemoryPage> createState() => _MemoryPageState();
}

class _MemoryPageState extends State<MemoryPage> {
  MemoryListing? _listing;
  String? _error;
  bool _loading = false;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    setState(() => _loading = true);
    try {
      final listing = await widget.board.loadMemory();
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

  Future<void> _open(String path, {required bool existing}) async {
    final changed = await Navigator.of(context).push<String>(
      MaterialPageRoute(
        builder: (_) => MemoryEditorPage(
          board: widget.board,
          path: path,
          existing: existing,
        ),
      ),
    );
    if (changed == null || !mounted) return;
    _say(changed);
    await _load();
  }

  Future<void> _new() async {
    final path = await showDialog<String>(
      context: context,
      builder: (context) => const _NewFileDialog(),
    );
    if (path == null || !mounted) return;
    await _open(path, existing: false);
  }

  Future<void> _forget(String path) async {
    if (!await confirmForget(context, path) || !mounted) return;
    try {
      await widget.board.forgetMemory(path);
      if (!mounted) return;
      _say('Forgotten: $path.');
      await _load();
    } on Object catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  @override
  Widget build(BuildContext context) {
    final listing = _listing;
    final small = Theme.of(context).textTheme.bodySmall;
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
              onPressed: () => unawaited(_new()),
              icon: const Icon(Icons.add),
              label: const Text('New file'),
            ),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 16, 16, 96),
        children: [
          const Text(
            'What Moneypenny remembers about you, kept in memory/ of your personal '
            'repository and read at the start of every conversation. Each save and each '
            '"forget" is a commit by you.',
          ),
          if (listing?.commit case final commit?)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                'As of commit ${commit.length > 12 ? commit.substring(0, 12) : commit}',
                style: small,
              ),
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
          else if (listing.files.isEmpty)
            const Text('She remembers nothing about you yet.')
          else ...[
            for (final file in listing.files)
              Card(
                child: ListTile(
                  leading: const Icon(Icons.description_outlined),
                  title: Text(revealInvisible(file.path)),
                  subtitle: Text('${file.bytes} bytes'),
                  onTap: () => unawaited(_open(file.path, existing: true)),
                  trailing: widget.board.demo
                      ? null
                      : IconButton(
                          tooltip: 'Forget ${file.path}',
                          onPressed: () => unawaited(_forget(file.path)),
                          icon: const Icon(Icons.delete_outline),
                        ),
                ),
              ),
            if (listing.more)
              Text(
                'More files than these are kept; see them in the repository.',
                style: small,
              ),
          ],
        ],
      ),
    );
  }
}

/// Asks whether to forget [path]; true when the person confirms.
Future<bool> confirmForget(BuildContext context, String path) async =>
    await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Forget $path?'),
        content: const Text(
          'Moneypenny stops remembering it: a commit by you removes it from your '
          'personal repository, whose history still holds it. A conversation she is in '
          'now keeps what it already read.',
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

/// Asks for a new file's path, relative to `memory/`.
class _NewFileDialog extends StatefulWidget {
  const _NewFileDialog();

  @override
  State<_NewFileDialog> createState() => _NewFileDialogState();
}

class _NewFileDialogState extends State<_NewFileDialog> {
  final _path = TextEditingController();

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  void _create() {
    final path = _path.text.trim();
    if (path.isNotEmpty) Navigator.of(context).pop(path);
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('New memory file'),
    content: TextField(
      key: const Key('memory-new-path'),
      controller: _path,
      autofocus: true,
      decoration: const InputDecoration(
        labelText: 'Path under memory/',
        hintText: 'projects.md',
      ),
      onSubmitted: (_) => _create(),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(onPressed: _create, child: const Text('Create')),
    ],
  );
}

/// One memory file: its text to read and edit, saved as a commit by the person, or forgotten.
/// A save is made from the text that was opened, so a file Moneypenny changed meanwhile is
/// refused rather than replaced. Pops with what was done.
class MemoryEditorPage extends StatefulWidget {
  const MemoryEditorPage({
    required this.board,
    required this.path,
    required this.existing,
    super.key,
  });

  final MissionBoard board;

  /// Relative to `memory/`.
  final String path;

  /// Whether the file exists: it is read first, and can be forgotten.
  final bool existing;

  @override
  State<MemoryEditorPage> createState() => _MemoryEditorPageState();
}

class _MemoryEditorPageState extends State<MemoryEditorPage> {
  final _text = TextEditingController();
  String? _digest;
  String? _problem;
  late bool _loading = widget.existing;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    if (widget.existing) unawaited(_read());
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _read() async {
    try {
      final file = await widget.board.readMemory(widget.path);
      if (!mounted) return;
      setState(() {
        _text.text = file.text;
        _digest = file.digest;
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
      await widget.board.saveMemory(widget.path, _text.text, base: _digest);
      if (mounted) Navigator.of(context).pop('Saved ${widget.path}.');
    } on Object catch (error) {
      if (mounted) setState(() => _problem = '$error');
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  Future<void> _forget() async {
    if (!await confirmForget(context, widget.path) || !mounted) return;
    setState(() {
      _saving = true;
      _problem = null;
    });
    try {
      await widget.board.forgetMemory(widget.path);
      if (mounted) Navigator.of(context).pop('Forgotten: ${widget.path}.');
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
    return Scaffold(
      appBar: AppBar(
        title: Text('memory/${widget.path}'),
        actions: [
          if (widget.existing && !readOnly)
            IconButton(
              tooltip: 'Forget ${widget.path}',
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
              Text(
                revealInvisible(problem),
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            Expanded(
              child: TextField(
                key: const Key('memory-text'),
                controller: _text,
                enabled: !busy && !readOnly,
                expands: true,
                maxLines: null,
                textAlignVertical: TextAlignVertical.top,
                style: const TextStyle(fontFamily: 'monospace'),
                decoration: const InputDecoration(
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
