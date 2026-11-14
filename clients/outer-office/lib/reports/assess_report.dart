import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';
import '../shell/oo_freshness.dart';
import '../theme/oo_theme.dart';
import 'reports_model.dart';

/// The ways a person waits, as the length an `awaiting` or `later` assessment names.
const _units = <(String, int)>[
  ('minutes', 60),
  ('hours', 3600),
  ('days', 86400),
];

/// Assesses [report] in a dialog; answers the Report as the gateway now has it, or null when the
/// person cancelled. The gateway's own words show when it refuses.
Future<ReportSummary?> showAssessReport(
  BuildContext context, {
  required MissionBoard board,
  required ReportSummary report,
  required List<String> priorities,
  required List<ReportSummary> others,
  List<String> handlings = const [],
}) => showDialog<ReportSummary>(
  context: context,
  builder: (_) => AssessReportDialog(
    board: board,
    report: report,
    priorities: priorities,
    handlings: handlings,
    others: others,
  ),
);

/// Opens [report] again, asking why; answers the Report as it now is, or null when cancelled.
Future<ReportSummary?> showReopenReport(
  BuildContext context, {
  required MissionBoard board,
  required ReportSummary report,
}) => showDialog<ReportSummary>(
  context: context,
  builder: (_) => _ReopenDialog(board: board, report: report),
);

class AssessReportDialog extends StatefulWidget {
  const AssessReportDialog({
    required this.board,
    required this.report,
    required this.priorities,
    required this.others,
    this.handlings = const [],
    super.key,
  });

  final MissionBoard board;
  final ReportSummary report;
  final List<String> priorities;

  /// The deployment's handling marks, narrowest first.
  final List<String> handlings;
  final List<ReportSummary> others;

  @override
  State<AssessReportDialog> createState() => _AssessReportDialogState();
}

class _AssessReportDialogState extends State<AssessReportDialog> {
  String _verdict = 'mission';
  final _reason = TextEditingController();
  final _mission = TextEditingController();
  final _question = TextEditingController();
  final _ask = TextEditingController();
  final _condition = TextEditingController();
  final _wait = TextEditingController();
  String? _of;
  String? _via;
  int _unit = 1;
  int? _credibility;
  String? _priority;
  String? _likelihood;
  String? _confidence;
  String? _problem;
  bool _sending = false;

  @override
  void dispose() {
    for (final controller in [
      _reason,
      _mission,
      _question,
      _ask,
      _condition,
      _wait,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  bool get _takesQuestion => _verdict == 'awaiting' || _verdict == 'decision';
  bool get _waits => _verdict == 'awaiting' || _verdict == 'later';

  /// The handling marks a question about this Report may go as far as: up to the Report's own,
  /// the narrowest when it has none.
  List<String> get _viaChoices {
    final held = widget.handlings.indexOf(widget.report.handling ?? '');
    return widget.handlings.take((held < 0 ? 0 : held) + 1).toList();
  }

  /// The PUT's body, or null with [_problem] set when what is entered cannot be sent.
  Map<String, Object?>? _body() {
    String? problem;
    final body = <String, Object?>{
      'assessment': _verdict,
      'reason': _reason.text.trim(),
    };
    if (_reason.text.trim().isEmpty) problem = 'Write a note for the record.';
    switch (_verdict) {
      case 'mission':
        final mission = _mission.text.trim();
        if (mission.isNotEmpty) {
          body['mission'] = mission;
        } else if (!widget.report.isSourceItem) {
          problem ??= 'Name the Mission this Report is about.';
        }
      case 'duplicate' || 'related':
        if (_of == null) {
          problem ??= 'Pick the other Report.';
        } else {
          body['of'] = _of;
        }
      case 'awaiting' || 'decision':
        final question = _question.text.trim();
        if (question.isEmpty) problem ??= 'Write the question.';
        body['question'] = question;
    }
    if (_verdict == 'awaiting') {
      final ask = _ask.text.trim();
      if (ask.isNotEmpty) body['ask'] = ask;
      if (_via != null) body['via'] = _via;
    }
    if (_verdict == 'later') {
      final condition = _condition.text.trim();
      if (condition.isNotEmpty) body['condition'] = condition;
    }
    if (_waits) {
      final amount = int.tryParse(_wait.text.trim());
      if (_wait.text.trim().isNotEmpty && (amount == null || amount < 0)) {
        problem ??= 'The wait is a whole number.';
      } else if (amount != null) {
        body['wait_seconds'] = amount * _units[_unit].$2;
      } else if (_verdict == 'awaiting') {
        problem ??= 'Say how long to wait for the answer.';
      } else if (_condition.text.trim().isEmpty) {
        problem ??= 'Say how long to wait, what to wait for, or both.';
      }
    }
    if (_credibility != null) body['credibility'] = _credibility;
    if (_priority != null) body['priority'] = _priority;
    if ((_likelihood == null) != (_confidence == null)) {
      problem ??= 'A judgement takes both a likelihood and a confidence.';
    }
    if (_likelihood != null && _confidence != null) {
      body['likelihood'] = _likelihood;
      body['confidence'] = _confidence;
    }
    setState(() => _problem = problem);
    return problem == null ? body : null;
  }

  Future<void> _save() async {
    final body = _body();
    if (body == null) return;
    setState(() => _sending = true);
    try {
      final changed = await widget.board.assessReport(widget.report.id, body);
      if (mounted) Navigator.of(context).pop(changed);
    } on Object catch (error) {
      if (!mounted) return;
      setState(() {
        _sending = false;
        _problem = '$error';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final source = widget.report.isSourceItem;
    return AlertDialog(
      title: Text(
        'Assess ${revealInvisible(widget.report.title ?? widget.report.id)}',
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              DropdownButtonFormField<String>(
                key: const ValueKey('assessment'),
                initialValue: _verdict,
                isExpanded: true,
                decoration: const InputDecoration(labelText: 'Assess as'),
                items: [
                  for (final MapEntry(:key, :value) in reportVerdicts.entries)
                    DropdownMenuItem(
                      value: key,
                      child: Text(
                        _describe(key, value),
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                ],
                onChanged: _sending
                    ? null
                    : (value) => setState(() => _verdict = value ?? _verdict),
              ),
              const SizedBox(height: 12),
              if (_verdict == 'mission')
                TextField(
                  key: const ValueKey('mission'),
                  controller: _mission,
                  decoration: InputDecoration(
                    labelText: 'Existing Mission (ID or handle)',
                    helperText: source
                        ? 'Leave empty to queue this Source item\'s brief as a '
                              'Mission.'
                        : 'A Report that is not a Source item names the Mission '
                              'it is about.',
                    helperMaxLines: 3,
                  ),
                ),
              if (_verdict == 'duplicate' || _verdict == 'related')
                DropdownButtonFormField<String>(
                  key: const ValueKey('of'),
                  initialValue: _of,
                  isExpanded: true,
                  decoration: InputDecoration(
                    labelText: _verdict == 'duplicate'
                        ? 'Duplicate of which Report'
                        : 'Related to which Report',
                    helperText: widget.others.isEmpty
                        ? 'No other Report is listed to pick.'
                        : null,
                  ),
                  items: [
                    for (final other in widget.others)
                      DropdownMenuItem(
                        value: other.id,
                        child: Text(
                          revealInvisible(other.title ?? other.id),
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                  onChanged: _sending
                      ? null
                      : (value) => setState(() => _of = value),
                ),
              if (_takesQuestion)
                TextField(
                  key: const ValueKey('question'),
                  controller: _question,
                  minLines: 1,
                  maxLines: 4,
                  decoration: InputDecoration(
                    labelText: _verdict == 'decision'
                        ? 'The decision asked of the people who may sign it'
                        : 'The question asked',
                    helperText: _verdict == 'decision'
                        ? 'It is filed as green ink of kind decision.'
                        : null,
                  ),
                ),
              if (_verdict == 'awaiting')
                TextField(
                  key: const ValueKey('ask'),
                  controller: _ask,
                  decoration: const InputDecoration(
                    labelText: 'Asked of (optional): a channel, tracker item or person',
                  ),
                ),
              if (_verdict == 'awaiting' && _viaChoices.isNotEmpty)
                DropdownButtonFormField<String?>(
                  key: const ValueKey('via'),
                  initialValue: _via,
                  isExpanded: true,
                  decoration: InputDecoration(
                    labelText: 'May go as far as (optional)',
                    helperText: widget.report.handling == null
                        ? 'This Report has no handling mark, so only the narrowest.'
                        : "Up to this Report's handling mark, "
                              '${widget.report.handling}.',
                  ),
                  items: [
                    const DropdownMenuItem<String?>(
                      value: null,
                      child: Text('No limit named'),
                    ),
                    for (final mark in _viaChoices)
                      DropdownMenuItem<String?>(value: mark, child: Text(mark)),
                  ],
                  onChanged: _sending
                      ? null
                      : (value) => setState(() => _via = value),
                ),
              if (_verdict == 'later')
                TextField(
                  key: const ValueKey('condition'),
                  controller: _condition,
                  decoration: const InputDecoration(
                    labelText: 'Waits until this holds (optional)',
                  ),
                ),
              if (_waits)
                Row(
                  children: [
                    Expanded(
                      child: TextField(
                        key: const ValueKey('wait'),
                        controller: _wait,
                        keyboardType: TextInputType.number,
                        decoration: InputDecoration(
                          labelText: _verdict == 'awaiting'
                              ? 'Deadline: wait for'
                              : 'Come back after (optional)',
                        ),
                      ),
                    ),
                    const SizedBox(width: 8),
                    DropdownButton<int>(
                      key: const ValueKey('unit'),
                      value: _unit,
                      items: [
                        for (final (index, (name, _)) in _units.indexed)
                          DropdownMenuItem(value: index, child: Text(name)),
                      ],
                      onChanged: _sending
                          ? null
                          : (value) => setState(() => _unit = value ?? _unit),
                    ),
                  ],
                ),
              const SizedBox(height: 12),
              TextField(
                key: const ValueKey('reason'),
                controller: _reason,
                minLines: 2,
                maxLines: 5,
                decoration: const InputDecoration(
                  labelText: 'Note: why',
                  helperText: 'Kept as your comment, apart from the Report\'s own words.',
                ),
              ),
              const SizedBox(height: 16),
              Text('Optional', style: text.labelMedium),
              Wrap(
                spacing: 12,
                runSpacing: 8,
                children: [
                  _Pick<int>(
                    label: 'Credibility',
                    value: _credibility,
                    options: [
                      for (var level = 1; level <= 6; level++)
                        (
                          level,
                          '$level'
                              '${level == 1
                                  ? ' confirmed by other sources'
                                  : level == 6
                                  ? ' cannot be judged'
                                  : ''}',
                        ),
                    ],
                    onChanged: _sending
                        ? null
                        : (value) => setState(() => _credibility = value),
                  ),
                  _Pick<String>(
                    label: 'Priority',
                    value: _priority,
                    options: [
                      for (final name in widget.priorities) (name, name),
                    ],
                    onChanged: _sending
                        ? null
                        : (value) => setState(() => _priority = value),
                  ),
                  _Pick<String>(
                    label: 'Likelihood',
                    value: _likelihood,
                    options: [
                      for (final (wire, words, band) in reportLikelihoods)
                        (wire, '$words ($band)'),
                    ],
                    onChanged: _sending
                        ? null
                        : (value) => setState(() => _likelihood = value),
                  ),
                  _Pick<String>(
                    label: 'Confidence',
                    value: _confidence,
                    options: [
                      for (final name in reportConfidences) (name, name),
                    ],
                    onChanged: _sending
                        ? null
                        : (value) => setState(() => _confidence = value),
                  ),
                ],
              ),
              if (_problem case final problem?)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Semantics(
                    liveRegion: true,
                    child: Text(
                      problem,
                      key: const ValueKey('problem'),
                      style: TextStyle(color: tokens.danger),
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: _sending ? null : () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: _sending ? null : _save,
          child: Text(_sending ? 'Saving' : 'Save the assessment'),
        ),
      ],
    );
  }

  String _describe(String verdict, String words) {
    final source = widget.report.isSourceItem;
    return switch (verdict) {
      'mission' =>
        source
            ? 'Mission: queue it as a brief, or name one'
            : 'Mission: name the Mission',
      'duplicate' => 'Duplicate of another Report',
      'related' => 'Related to another Report',
      'decision' => 'Decision: asked as green ink',
      _ => words,
    };
  }
}

/// A choice that may be left unset: "Leave as it is" is its first item.
class _Pick<T> extends StatelessWidget {
  const _Pick({
    required this.label,
    required this.value,
    required this.options,
    required this.onChanged,
  });

  final String label;
  final T? value;
  final List<(T, String)> options;
  final ValueChanged<T?>? onChanged;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: 240,
    child: DropdownButtonFormField<T?>(
      key: ValueKey('pick-$label'),
      initialValue: value,
      isExpanded: true,
      decoration: InputDecoration(labelText: label),
      items: [
        DropdownMenuItem<T?>(value: null, child: const Text('Leave as it is')),
        for (final (item, words) in options)
          DropdownMenuItem<T?>(
            value: item,
            child: Text(words, overflow: TextOverflow.ellipsis),
          ),
      ],
      onChanged: onChanged,
    ),
  );
}

class _ReopenDialog extends StatefulWidget {
  const _ReopenDialog({required this.board, required this.report});

  final MissionBoard board;
  final ReportSummary report;

  @override
  State<_ReopenDialog> createState() => _ReopenDialogState();
}

class _ReopenDialogState extends State<_ReopenDialog> {
  final _reason = TextEditingController();
  String? _problem;
  bool _sending = false;

  @override
  void dispose() {
    _reason.dispose();
    super.dispose();
  }

  Future<void> _reopen() async {
    if (_reason.text.trim().isEmpty) {
      setState(() => _problem = 'Say why it comes back.');
      return;
    }
    setState(() {
      _sending = true;
      _problem = null;
    });
    try {
      final changed = await widget.board.reopenReport(
        widget.report.id,
        _reason.text.trim(),
      );
      if (mounted) Navigator.of(context).pop(changed);
    } on Object catch (error) {
      if (!mounted) return;
      setState(() {
        _sending = false;
        _problem = '$error';
      });
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Reopen this Report'),
    content: SizedBox(
      width: 420,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'It becomes open again, to be assessed anew. The reopening is audited.',
          ),
          const SizedBox(height: 12),
          TextField(
            key: const ValueKey('reopen-reason'),
            controller: _reason,
            minLines: 2,
            maxLines: 4,
            decoration: const InputDecoration(labelText: 'Why it comes back'),
          ),
          if (_problem case final problem?)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: Text(
                problem,
                key: const ValueKey('problem'),
                style: TextStyle(color: OoTokens.of(context).danger),
              ),
            ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: _sending ? null : () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: _sending ? null : _reopen,
        child: Text(_sending ? 'Reopening' : 'Reopen'),
      ),
    ],
  );
}

/// Shows a source's standing: its reliability, how it was earned and the closed Reports behind
/// it, each of which [onOpen] opens.
Future<void> showSourceStanding(
  BuildContext context, {
  required MissionBoard board,
  required String key,
  required void Function(String reportId) onOpen,
}) => showDialog<void>(
  context: context,
  builder: (dialog) => AlertDialog(
    title: const Text('Source standing'),
    content: SizedBox(
      width: 520,
      child: FutureBuilder<ReportSourceStanding>(
        future: board.loadReportSource(key),
        builder: (context, snapshot) {
          if (snapshot.hasError) {
            return Text('${snapshot.error}');
          }
          final standing = snapshot.data;
          if (standing == null) return const LinearProgressIndicator();
          return _Standing(
            standing: standing,
            onOpen: (id) {
              Navigator.of(dialog).pop();
              onOpen(id);
            },
          );
        },
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(dialog).pop(),
        child: const Text('Close'),
      ),
    ],
  ),
);

class _Standing extends StatelessWidget {
  const _Standing({required this.standing, required this.onOpen});

  final ReportSourceStanding standing;
  final void Function(String reportId) onOpen;

  @override
  Widget build(BuildContext context) {
    final text = Theme.of(context).textTheme;
    final tokens = OoTokens.of(context);
    return SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(revealInvisible(standing.source), style: text.titleSmall),
          const SizedBox(height: 6),
          Text(
            'Reliability ${standing.reliability}',
            key: const ValueKey('reliability'),
            style: text.titleMedium,
          ),
          Text(
            standing.rated
                ? '${standing.confirmed} of ${standing.counted} counted Reports '
                      'confirmed.'
                : '${standing.counted} of the ${standing.minReports} counted '
                      'Reports needed before it is rated; until then it stays F.',
            key: const ValueKey('tally'),
          ),
          const SizedBox(height: 6),
          Text(
            'Counted outcomes: ${_words(standing.counts)}. Confirming: '
            '${_words(standing.confirms)}.'
            '${standing.bands.isEmpty ? '' : ' Percent confirmed for A, B, C and D: ${standing.bands.join(', ')}.'}',
            style: text.bodySmall?.copyWith(color: tokens.muted),
          ),
          const SizedBox(height: 12),
          Text('CLOSED REPORTS BEHIND IT', style: ooSectionLabel(context)),
          if (standing.reports.isEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                'None yet that you may read.',
                style: TextStyle(color: tokens.muted),
              ),
            ),
          for (final report in standing.reports)
            ListTile(
              dense: true,
              contentPadding: EdgeInsets.zero,
              title: Text(revealInvisible(report.title ?? report.id)),
              subtitle: Text(
                reportOutcomes[report.resolution?.outcome] ??
                    report.resolution?.outcome ??
                    report.state ??
                    '',
              ),
              trailing: OoTime(report.resolution?.at ?? report.updated),
              onTap: () => onOpen(report.id),
            ),
        ],
      ),
    );
  }

  String _words(List<String> outcomes) => outcomes.isEmpty
      ? 'none'
      : outcomes
            .map((outcome) => reportOutcomes[outcome] ?? outcome)
            .join(', ');
}
