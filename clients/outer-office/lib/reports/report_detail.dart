import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../assistant/export.dart' as exporting;
import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';
import '../routes/oo_route.dart';
import '../shell/oo_freshness.dart';
import '../shell/operation_names.dart';
import '../shell/oo_link.dart';
import '../shell/oo_shell.dart';
import '../theme/oo_theme.dart';
import '../widgets/oo_principals.dart';
import 'assess_report.dart';
import 'knowledge_editor.dart';
import 'reports_model.dart';

/// The place a scope is, as a person reads it: the Operation's name, `Station ID`, `M`, or
/// whose knowledge it is.
String reportScopeLabel(String scope) {
  if (scope == 'global') return 'Everyone';
  if (scope.startsWith('person:')) {
    return 'Person ${scope.substring('person:'.length)}';
  }
  if (scope.startsWith('operation:')) {
    return OperationNames.of(scope.substring('operation:'.length));
  }
  if (scope.startsWith('station:')) {
    return 'Station ${scope.substring('station:'.length)}';
  }
  return scope == 'm' ? 'M' : scope;
}

/// Where a scope's page is: an Operation's or a Station's.
OoRoute? reportScopeRoute(String scope) {
  if (scope.startsWith('operation:')) {
    return OoLinkTo.operation(scope.substring('operation:'.length));
  }
  if (scope.startsWith('station:')) {
    return OoLinkTo.station(scope.substring('station:'.length));
  }
  return null;
}

/// Who filed a Report, in words: a person by name, an Agent by codename and Mission, a charge,
/// or a Source by its key.
String reportFilerWords(ReportSummary report, OoPrincipals? principals) {
  final filer = report.filer;
  return switch (report.origin) {
    'person' => ooPrincipalText(principals, 'person:${filer?['person'] ?? ''}'),
    'agent' => 'Agent ${filer?['agent']}',
    'charge' => 'Agent ${filer?['agent']}, on a charge',
    'source' => 'Source ${report.source ?? filer?['source']}',
    'assistant' =>
      'Moneypenny, for ${ooPrincipalText(principals, 'person:${filer?['person'] ?? ''}')}',
    'built_in' => 'This build',
    _ => report.source ?? 'unknown',
  };
}

/// One Report in full: what was filed, who filed it, how far it is trusted, what was decided
/// about it and how it ended, with the actions this person has.
///
/// Its words are other people's unless a person filed them: they show with what renders as
/// nothing spelled out.
class ReportDetail extends StatefulWidget {
  const ReportDetail({
    required this.board,
    required this.reportId,
    required this.onRoute,
    this.priorities = const [],
    this.handlings = const [],
    this.others = const [],
    this.onChanged,
    this.onGone,
    this.onClose,
    this.saveText = exporting.saveText,
    super.key,
  });

  final MissionBoard board;
  final String reportId;
  final ValueChanged<OoRoute> onRoute;

  /// The deployment's priorities, most urgent first, for an assessment to choose from.
  final List<String> priorities;

  /// The deployment's handling marks, narrowest first, for a question to be limited by.
  final List<String> handlings;

  /// The other Reports a duplicate or related assessment may name.
  final List<ReportSummary> others;

  /// Told when an assessment or a reopening changed the Report.
  final ValueChanged<ReportSummary>? onChanged;

  /// Told when the Report was forgotten or retired, so it is no longer where it was.
  final VoidCallback? onGone;

  /// Set where the list is not beside it: a button back to the list.
  final VoidCallback? onClose;

  /// Saves the Markdown document of the Report: a download in a browser, a file on a desktop.
  final Future<String?> Function(String name, String text) saveText;

  @override
  State<ReportDetail> createState() => _ReportDetailState();
}

class _ReportDetailState extends State<ReportDetail> {
  late Future<ReportSummary> _report = widget.board.loadReport(widget.reportId);

  @override
  void didUpdateWidget(ReportDetail oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.reportId != widget.reportId) _reload();
  }

  void _reload() {
    setState(() {
      _report = widget.board.loadReport(widget.reportId);
    });
  }

  void _gone() {
    widget.onGone?.call();
    widget.onClose?.call();
  }

  void _changed(ReportSummary report) {
    setState(() {
      _report = Future.value(report);
    });
    widget.onChanged?.call(report);
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<ReportSummary>(
    future: _report,
    builder: (context, snapshot) {
      final tokens = OoTokens.of(context);
      if (snapshot.hasError) {
        return ListView(
          padding: const EdgeInsets.all(24),
          children: [
            if (widget.onClose != null) _back(),
            Text(
              'This Report could not be shown.',
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 6),
            Text('${snapshot.error}', style: TextStyle(color: tokens.muted)),
            const SizedBox(height: 8),
            TextButton(onPressed: _reload, child: const Text('Try again')),
          ],
        );
      }
      final report = snapshot.data;
      if (report == null) {
        return const Center(child: CircularProgressIndicator());
      }
      return _Facts(
        board: widget.board,
        report: report,
        priorities: widget.priorities,
        handlings: widget.handlings,
        others: widget.others,
        onRoute: widget.onRoute,
        onChanged: _changed,
        onGone: _gone,
        saveText: widget.saveText,
        back: widget.onClose == null ? null : _back(),
      );
    },
  );

  Widget _back() => Align(
    alignment: Alignment.centerLeft,
    child: TextButton.icon(
      onPressed: widget.onClose,
      icon: const Icon(Icons.arrow_back, size: 18),
      label: const Text('Back to the Reports'),
    ),
  );
}

class _Facts extends StatelessWidget {
  const _Facts({
    required this.board,
    required this.report,
    required this.priorities,
    required this.handlings,
    required this.others,
    required this.onRoute,
    required this.onChanged,
    required this.onGone,
    required this.saveText,
    this.back,
  });

  final MissionBoard board;
  final ReportSummary report;
  final List<String> priorities;
  final List<String> handlings;
  final List<ReportSummary> others;
  final ValueChanged<OoRoute> onRoute;
  final ValueChanged<ReportSummary> onChanged;
  final VoidCallback onGone;
  final Future<String?> Function(String name, String text) saveText;
  final Widget? back;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final principals = board.principals;
    final scopeRoute = reportScopeRoute(report.scope);
    final canAssess = board.controlsReports(report.scope) && report.whole;
    final marks = [
      ?report.classification,
      if (report.handling case final handling?) 'handling $handling',
      for (final name in report.compartments) 'compartment $name',
    ];
    return FocusTraversalGroup(
      child: ListView(
        padding: const EdgeInsets.all(24),
        children: [
          ?back,
          Semantics(
            header: true,
            child: Text(
              revealInvisible(
                report.title ??
                    'A Report marked ${marks.isEmpty ? 'above your clearance' : marks.join(', ')}',
              ),
              style: text.titleLarge,
            ),
          ),
          const SizedBox(height: 6),
          Wrap(
            spacing: 14,
            runSpacing: 4,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text(
                report.state == null
                    ? ''
                    : report.state![0].toUpperCase() +
                          report.state!.substring(1),
                style: text.titleSmall,
              ),
              if (report.priority case final priority?)
                Text('Priority $priority', style: text.bodySmall),
              if (report.kind case final kind?)
                Text(kind, style: text.bodySmall),
              if (report.severity case final severity?)
                Text('Severity $severity', style: text.bodySmall),
              if (scopeRoute != null)
                OoLink(
                  reportScopeLabel(report.scope),
                  route: scopeRoute,
                  style: text.bodySmall,
                )
              else
                Text(reportScopeLabel(report.scope), style: text.bodySmall),
            ],
          ),
          const SizedBox(height: 12),
          if (!report.whole)
            _Note(
              report.sight == 'tearline'
                  ? 'You see this Report\'s tearline only: its marks are above '
                        'what you are read in to or cleared for.'
                  : 'You are not read in to this Report, or not cleared for it.',
            ),
          if (report.isKnowledge && report.whole) ..._knowledge(context),
          if (report.untrusted && report.whole && !report.builtIn)
            const _Note(
              'Filed by an Agent or a Source: its words are reported, not '
              'verified.',
            ),
          if (report.text case final body?)
            Padding(
              key: const ValueKey('report-text'),
              padding: const EdgeInsets.only(top: 4, bottom: 12),
              child: SelectableText(
                revealInvisible(body),
                style: text.bodyMedium,
              ),
            ),
          if (report.whole) ...[
            _Row(
              'Filed by',
              Text(reportFilerWords(report, principals)),
              key: const ValueKey('filed-by'),
            ),
            if (report.source case final source?)
              _Row(
                'Source',
                Wrap(
                  spacing: 8,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    Text('$source · reliability ${report.reliability ?? 'F'}'),
                    TextButton(
                      onPressed: () => showSourceStanding(
                        context,
                        board: board,
                        key: source,
                        onOpen: (id) => onRoute(
                          const OoRoute(OoPage.reports)
                              .withField('report_id', id),
                        ),
                      ),
                      child: const Text('Source standing'),
                    ),
                  ],
                ),
              ),
            if (report.subSource case final sub?)
              _Row('Passed on by', Text(revealInvisible(sub))),
            if (report.from != null)
              _Row('Information from', OoTime(report.from)),
            if (report.isKnowledge && report.revision != null)
              _Row(
                'Revision',
                Text(
                  '${report.revision}'
                  '${report.updatedBy == null ? '' : ', changed by ${report.updatedBy}'}',
                ),
              ),
            _Row('Filed', OoTime(report.created)),
            _Row('Updated', OoTime(report.updated)),
            if (report.deadline != null)
              _Row('Needs an answer', OoTime(report.deadline)),
            _Row(
              'Credibility',
              Text(
                report.credibility == null
                    ? 'Not assessed yet'
                    : '${report.credibility} (1 confirmed by other sources, 6 cannot be judged)',
              ),
            ),
          ],
          if (marks.isNotEmpty) _Row('Markings', Text(marks.join(' · '))),
          if (report.tearline case final tear? when report.whole)
            _Row('Tearline', SelectableText(revealInvisible(tear))),
          if (report.assessment case final assessment?) ...[
            const SizedBox(height: 16),
            Text('ASSESSMENT', style: ooSectionLabel(context)),
            const SizedBox(height: 6),
            Text(
              reportVerdicts[assessment.verdict] ?? assessment.verdict,
              style: text.titleSmall,
            ),
            Wrap(
              spacing: 6,
              children: [
                OoPrincipalBadge(
                  assessment.by,
                  style: text.bodySmall?.copyWith(color: tokens.muted),
                ),
                OoTime(assessment.at),
              ],
            ),
            if (assessment.judgement case final judgement?)
              Text('Judged ${judgement.words}', style: text.bodyMedium),
            if (assessment.comment case final comment?)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: SelectableText(revealInvisible(comment)),
              ),
            if (assessment.waitsFor case final waits?)
              Text(
                assessment.verdict == 'later'
                    ? 'Waits for: ${revealInvisible(waits)}'
                    : 'Asked: ${revealInvisible(waits)}',
                style: text.bodyMedium,
              ),
            if (assessment.until != null)
              OoTime(assessment.until, prefix: 'Comes back '),
          ],
          if (report.resolution case final resolution?) ...[
            const SizedBox(height: 16),
            Text('RESOLUTION', style: ooSectionLabel(context)),
            const SizedBox(height: 6),
            Text(
              reportOutcomes[resolution.outcome] ??
                  resolution.outcome ??
                  'Closed',
              style: text.titleSmall,
            ),
            Wrap(
              spacing: 6,
              children: [
                OoPrincipalBadge(
                  resolution.by,
                  style: text.bodySmall?.copyWith(color: tokens.muted),
                ),
                OoTime(resolution.at),
              ],
            ),
            if (resolution.note case final note?)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: SelectableText(revealInvisible(note)),
              ),
          ],
          if (report.links.isNotEmpty && report.whole) ...[
            const SizedBox(height: 16),
            Text('LINKS', style: ooSectionLabel(context)),
            const SizedBox(height: 6),
            for (final link in report.links)
              _Link(link, report, board, onRoute),
          ],
          const SizedBox(height: 20),
          if (report.isKnowledge && report.whole)
            ..._knowledgeActions(context)
          else if (report.state == 'open')
            _Action(
              label: 'Assess this Report',
              allowed: canAssess,
              why: _why(),
              onPressed: () async {
                final changed = await showAssessReport(
                  context,
                  board: board,
                  report: report,
                  priorities: priorities,
                  handlings: handlings,
                  others: [
                    for (final other in others)
                      if (other.id != report.id && other.whole) other,
                  ],
                );
                if (changed != null) onChanged(changed);
              },
            )
          else if (report.state != null)
            _Action(
              label: 'Reopen this Report',
              allowed: canAssess,
              why: _why(),
              onPressed: () async {
                final changed = await showReopenReport(
                  context,
                  board: board,
                  report: report,
                );
                if (changed != null) onChanged(changed);
              },
            ),
          if (report.sight != 'markings' &&
              !(report.isKnowledge && report.whole))
            Align(alignment: Alignment.centerLeft, child: _download(context)),
        ],
      ),
    );
  }

  /// What a memory, tradecraft, rule or contact says about itself, above its text.
  List<Widget> _knowledge(BuildContext context) {
    final text = Theme.of(context).textTheme;
    final proposes = report.proposes;
    return [
      if (report.summary case final summary? when summary.isNotEmpty)
        Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Text(revealInvisible(summary), style: text.bodyLarge),
        ),
      if (report.topics.isNotEmpty)
        Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Wrap(
            spacing: 6,
            runSpacing: 4,
            children: [
              for (final topic in report.topics)
                Chip(
                  label: Text(revealInvisible(topic)),
                  visualDensity: VisualDensity.compact,
                ),
            ],
          ),
        ),
      if (report.builtIn)
        const _Note(
          'Shipped with this build, so it is read only. Override it for this '
          'deployment to change what Agents read: a Report of the same title '
          'ranks above it.',
          key: ValueKey('report-built-in'),
        ),
      if (report.exposed)
        const _Note(
          'An Agent wrote this after reading text from outside, so it is marked '
          'until a person saves an edit.',
          key: ValueKey('report-exposed'),
        ),
      if (report.isProposal && proposes != null)
        _Note(
          proposes['forget'] == true
              ? 'Proposes forgetting ${proposes['target']}.'
              : 'Proposes the next revision of ${proposes['target']}.',
        )
      else if (report.isProposal)
        const _Note('An Agent proposed this; it waits for a person to decide.'),
    ];
  }

  Widget _download(BuildContext context) => TextButton.icon(
    key: const ValueKey('report-download'),
    onPressed: () => unawaited(_save(context)),
    icon: const Icon(Icons.download_outlined, size: 18),
    label: const Text('Download as Markdown'),
  );

  /// The actions on knowledge: edit and forget, accept and decline a proposal, or override a
  /// shipped guide for this deployment.
  List<Widget> _knowledgeActions(BuildContext context) => [
    Wrap(
      spacing: 12,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        if (report.builtIn)
          FilledButton(
            key: const ValueKey('report-override'),
            onPressed: () => unawaited(_override(context)),
            child: const Text('Override for this deployment'),
          )
        else if (report.isProposal) ...[
          FilledButton(
            key: const ValueKey('report-accept'),
            onPressed: () => unawaited(_decide(context, approve: true)),
            child: const Text('Accept'),
          ),
          OutlinedButton(
            key: const ValueKey('report-decline'),
            onPressed: () => unawaited(_decide(context, approve: false)),
            child: const Text('Decline'),
          ),
        ] else ...[
          FilledButton(
            key: const ValueKey('report-edit'),
            onPressed: () => unawaited(_edit(context)),
            child: const Text('Edit'),
          ),
          OutlinedButton(
            key: const ValueKey('report-forget'),
            onPressed: () => unawaited(_forget(context)),
            child: const Text('Forget'),
          ),
        ],
        _download(context),
      ],
    ),
  ];

  void _say(BuildContext context, String words) =>
      ScaffoldMessenger.maybeOf(context)
          ?.showSnackBar(SnackBar(content: Text(words)));

  Future<void> _reload(BuildContext context) async {
    try {
      onChanged(await board.loadReport(report.id));
    } on Object {
      onGone();
    }
  }

  Future<void> _edit(BuildContext context) async {
    final done = await Navigator.of(context).push<String>(
      MaterialPageRoute(
        builder: (_) => KnowledgeEditorPage(board: board, entry: report),
      ),
    );
    if (done == null || !context.mounted) return;
    _say(context, done);
    await _reload(context);
  }

  Future<void> _override(BuildContext context) async {
    final done = await Navigator.of(context).push<String>(
      MaterialPageRoute(
        builder: (_) => KnowledgeEditorPage(
          board: board,
          template: report,
          scope: 'global',
        ),
      ),
    );
    if (done == null || !context.mounted) return;
    _say(context, done);
    onGone();
  }

  Future<void> _forget(BuildContext context) async {
    final what = report.title ?? report.id;
    if (!await confirmForget(context, what) || !context.mounted) return;
    try {
      await board.forgetKnowledge(report.id);
    } on Object catch (error) {
      if (context.mounted) _say(context, '$error');
      return;
    }
    if (!context.mounted) return;
    _say(context, 'Forgotten: $what.');
    onGone();
  }

  Future<void> _decide(BuildContext context, {required bool approve}) async {
    try {
      await board.decideKnowledge(report.id, approve: approve);
    } on Object catch (error) {
      if (context.mounted) _say(context, '$error');
      return;
    }
    if (!context.mounted) return;
    _say(
      context,
      approve ? 'Accepted: ${report.title}.' : 'Declined: ${report.title}.',
    );
    await _reload(context);
  }

  Future<void> _save(BuildContext context) async {
    final name = '${report.id.replaceAll(RegExp(r'[^A-Za-z0-9_.-]'), '-')}.md';
    try {
      final markdown = await board.loadReportMarkdown(report.id);
      final where = await saveText(name, markdown);
      if (context.mounted) {
        _say(context, where == null ? 'Downloaded $name.' : 'Saved to $where');
      }
    } on Object catch (error) {
      if (context.mounted) {
        _say(context, 'The Report was not downloaded: $error');
      }
    }
  }

  String _why() => report.whole
      ? 'Assessing a Report needs mission.control on its Operation '
            '(station.configure on a Station).'
      : 'Assessing a Report needs reading its content.';
}

class _Note extends StatelessWidget {
  const _Note(this.words, {super.key});

  final String words;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 8),
    child: Text(
      words,
      style: Theme.of(context).textTheme.bodySmall
          ?.copyWith(color: OoTokens.of(context).muted),
    ),
  );
}

class _Row extends StatelessWidget {
  const _Row(this.label, this.value, {super.key});

  final String label;
  final Widget value;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 2),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 140,
          child: Text(
            label,
            style: Theme.of(context).textTheme.bodySmall
                ?.copyWith(color: OoTokens.of(context).muted),
          ),
        ),
        Expanded(child: value),
      ],
    ),
  );
}

class _Link extends StatelessWidget {
  const _Link(this.link, this.report, this.board, this.onRoute);

  final ReportLinkSummary link;
  final ReportSummary report;
  final MissionBoard board;
  final ValueChanged<OoRoute> onRoute;

  @override
  Widget build(BuildContext context) {
    final operation = report.operationId;
    return switch (link.kind) {
      'mission' when operation != null => OoLink(
        'Mission ${board.handleOf(operation, link.id)}',
        route: OoLinkTo.mission(operation, link.id),
      ),
      'report' => Align(
        alignment: Alignment.centerLeft,
        child: TextButton(
          onPressed: () => onRoute(
            const OoRoute(OoPage.reports).withField('report_id', link.id),
          ),
          child: Text('Report ${link.id}'),
        ),
      ),
      'decision' when operation != null => OoLink(
        'Decision ${link.id} in Green Ink',
        route: const OoRoute(OoPage.greenInk)
            .withField('operation_id', operation)
            .withField('request_id', link.id),
      ),
      _ => Text('${link.kind}: ${revealInvisible(link.id)}'),
    };
  }
}

/// A button, or a disabled one with what it needs written beside it.
class _Action extends StatelessWidget {
  const _Action({
    required this.label,
    required this.allowed,
    required this.why,
    required this.onPressed,
  });

  final String label;
  final bool allowed;
  final String why;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) => Wrap(
    spacing: 12,
    runSpacing: 4,
    crossAxisAlignment: WrapCrossAlignment.center,
    children: [
      FilledButton(onPressed: allowed ? onPressed : null, child: Text(label)),
      if (!allowed)
        Text(
          why,
          style: Theme.of(context).textTheme.bodySmall
              ?.copyWith(color: OoTokens.of(context).muted),
        ),
    ],
  );
}
