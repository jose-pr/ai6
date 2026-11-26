import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';
import 'report_detail.dart' show reportScopeLabel;
import 'reports_model.dart';

/// One line saying who wrote [writtenBy] and whether it was marked.
String knowledgeSource(String writtenBy, bool exposed) {
  final written = writtenBy.isEmpty ? 'Written' : 'Written by $writtenBy';
  return exposed ? '$written, after reading outside text' : written;
}

/// Who filed [report], as one short name: `person:ID`, `agent:CODENAME`, a Source or a charge.
String knowledgeWriter(ReportSummary report) {
  final filer = report.filer;
  return switch (report.origin) {
    'person' => 'person:${filer?['person'] ?? ''}',
    'agent' || 'charge' => 'agent:${filer?['agent'] ?? ''}',
    'assistant' => 'moneypenny:${filer?['person'] ?? ''}',
    'built_in' => 'this build',
    _ => report.source ?? '',
  };
}

/// Asks whether to forget [what]; true when the person confirms.
Future<bool> confirmForget(BuildContext context, String what) async =>
    await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Forget "$what"?'),
        content: const Text(
          'A memory goes for good, with its earlier versions; tradecraft, a rule or a contact '
          'is retired and kept. A conversation or a Mission now running keeps what it already '
          'read.',
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

/// A known Report (a memory, tradecraft, a rule or a contact) to read and edit by a form, or a
/// new one with no [entry]. A save names the revision that was opened, so one changed meanwhile
/// is refused rather than replaced. [template] starts a new one from another Report's words (an
/// override of a shipped guide). Pops with what was done.
class KnowledgeEditorPage extends StatefulWidget {
  const KnowledgeEditorPage({
    required this.board,
    this.entry,
    this.template,
    this.scope,
    super.key,
  });

  final MissionBoard board;

  /// The Report opened, with its text; `null` for a new one.
  final ReportSummary? entry;

  /// A Report whose kind, title, summary, topics and text a new one starts from.
  final ReportSummary? template;

  /// Where a new one starts out, when it is not the person's own.
  final String? scope;

  @override
  State<KnowledgeEditorPage> createState() => _KnowledgeEditorPageState();
}

class _KnowledgeEditorPageState extends State<KnowledgeEditorPage> {
  final _title = TextEditingController();
  final _summary = TextEditingController();
  final _content = TextEditingController();
  final _compartments = TextEditingController();
  late String _scope;
  late String _kind;
  String? _problem;
  bool _saving = false;

  ReportSummary? get _opened => widget.entry;

  @override
  void initState() {
    super.initState();
    final from = widget.entry ?? widget.template;
    _title.text = from?.title ?? '';
    _summary.text = from?.summary ?? '';
    _content.text = from?.text ?? '';
    _compartments.text = (widget.entry?.compartments ?? const []).join(', ');
    _kind = widget.entry?.kind ?? widget.template?.kind ?? 'memory';
    final scopes = widget.board.knowledgeScopes;
    _scope =
        widget.entry?.scope ??
        widget.scope ??
        (scopes.isEmpty ? 'global' : scopes.first);
  }

  @override
  void dispose() {
    _title.dispose();
    _summary.dispose();
    _content.dispose();
    _compartments.dispose();
    super.dispose();
  }

  List<String> get _named => [
    for (final name in _compartments.text.split(','))
      if (name.trim().isNotEmpty) name.trim(),
  ];

  String _scopeLabel(String scope) {
    if (scope == 'person:${widget.board.userSession?.personId}') {
      return 'Of you';
    }
    return scope == 'global' ? 'Everyone' : reportScopeLabel(scope);
  }

  Future<void> _save() async {
    setState(() {
      _saving = true;
      _problem = null;
    });
    try {
      final title = _title.text.trim();
      final compartments = _named;
      final opened = _opened;
      final unchanged =
          opened != null &&
          compartments.toSet().containsAll(opened.compartments) &&
          opened.compartments.toSet().containsAll(compartments);
      await widget.board.saveKnowledge(
        scope: _scope,
        kind: _kind,
        id: opened?.id,
        revision: opened?.revision,
        title: title,
        summary: _summary.text.trim(),
        topics: opened?.topics ?? widget.template?.topics,
        text: _content.text,
        compartments: _scope.startsWith('person:') || unchanged
            ? null
            : compartments,
      );
      if (mounted) Navigator.of(context).pop('Saved $title.');
    } on Object catch (error) {
      if (mounted) setState(() => _problem = '$error');
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final busy = _saving;
    final readOnly = widget.board.demo;
    final opened = _opened;
    final error = Theme.of(context).colorScheme.error;
    final scopes = {...widget.board.knowledgeScopes, _scope}.toList();
    return Scaffold(
      appBar: AppBar(
        title: Text(
          opened == null ? 'New entry' : reportKindWords(opened.kind ?? ''),
        ),
      ),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 12,
          children: [
            if (_problem case final problem?)
              Text(revealInvisible(problem), style: TextStyle(color: error)),
            if (opened != null)
              Text(
                '${knowledgeSource(knowledgeWriter(opened), opened.exposed)}. '
                'Last changed by ${opened.updatedBy ?? 'unknown'}, '
                'revision ${opened.revision ?? 1}.'
                '${opened.exposed ? ' Saving your edit clears the mark.' : ''}',
                key: const Key('knowledge-source'),
                style: opened.exposed
                    ? TextStyle(color: error)
                    : Theme.of(context).textTheme.bodySmall,
              ),
            if (opened == null)
              Row(
                spacing: 12,
                children: [
                  Expanded(
                    child: DropdownButtonFormField<String>(
                      key: const Key('knowledge-scope'),
                      initialValue: _scope,
                      decoration: const InputDecoration(
                        labelText: 'Whose',
                        border: OutlineInputBorder(),
                      ),
                      items: [
                        for (final scope in scopes)
                          DropdownMenuItem(
                            value: scope,
                            child: Text(_scopeLabel(scope)),
                          ),
                      ],
                      onChanged: busy || readOnly
                          ? null
                          : (value) => setState(() => _scope = value ?? _scope),
                    ),
                  ),
                  Expanded(
                    child: DropdownButtonFormField<String>(
                      key: const Key('knowledge-kind'),
                      initialValue: _kind,
                      decoration: const InputDecoration(
                        labelText: 'Kind',
                        border: OutlineInputBorder(),
                      ),
                      items: [
                        for (final kind in knowledgeKinds)
                          DropdownMenuItem(
                            value: kind,
                            child: Text(reportKindWords(kind)),
                          ),
                      ],
                      onChanged: busy || readOnly
                          ? null
                          : (value) => setState(() => _kind = value ?? _kind),
                    ),
                  ),
                ],
              ),
            TextField(
              key: const Key('knowledge-title'),
              controller: _title,
              enabled: !busy && !readOnly,
              decoration: const InputDecoration(
                labelText: 'Title',
                helperText: 'A short name; it may repeat.',
                border: OutlineInputBorder(),
              ),
            ),
            TextField(
              key: const Key('knowledge-summary'),
              controller: _summary,
              enabled: !busy && !readOnly,
              decoration: const InputDecoration(
                labelText: 'Summary (optional)',
                helperText: 'One line saying what it is for; it is what a search shows.',
                border: OutlineInputBorder(),
              ),
            ),
            if (!_scope.startsWith('person:'))
              TextField(
                key: const Key('knowledge-compartments'),
                controller: _compartments,
                enabled: !busy && !readOnly,
                decoration: const InputDecoration(
                  labelText: 'Compartments (optional)',
                  helperText:
                      'Only those read into one of these see it. You add or remove only '
                      'compartments you are read into.',
                  border: OutlineInputBorder(),
                ),
              ),
            Expanded(
              child: TextField(
                key: const Key('knowledge-text'),
                controller: _content,
                enabled: !busy && !readOnly,
                expands: true,
                maxLines: null,
                textAlignVertical: TextAlignVertical.top,
                decoration: const InputDecoration(
                  labelText: 'Content',
                  alignLabelWithHint: true,
                  border: OutlineInputBorder(),
                  hintText: 'What should be known, in Markdown',
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
