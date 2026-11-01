import 'package:material_ui/material_ui.dart';

import '../assistant/pages.dart' show launchPage;
import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';
import '../shell/oo_link.dart';
import '../theme/oo_theme.dart';

/// A question on a linked issue that waits for an answer.
class IssueQuestionSummary {
  const IssueQuestionSummary({required this.askedAt, required this.expiresAt});

  final double askedAt;
  final double expiresAt;

  factory IssueQuestionSummary.fromJson(Map<String, dynamic> json) =>
      IssueQuestionSummary(
        askedAt: (json['asked_at'] as num).toDouble(),
        expiresAt: (json['expires_at'] as num).toDouble(),
      );
}

/// A tracker issue tied to a Mission. Its title and labels are other people's text.
class LinkedIssueSummary {
  const LinkedIssueSummary({
    required this.issue,
    required this.relation,
    required this.state,
    this.url,
    this.title,
    this.issueState,
    this.labels = const [],
    this.phase,
    this.question,
  });

  /// `TRACKER:ITEM`.
  final String issue;

  /// `source`, `related`, `child`, `parent` or `mirror`.
  final String relation;

  /// `proposed` or `active`.
  final String state;
  final String? url;
  final String? title;
  final String? issueState;
  final List<String> labels;

  /// The phase AI6 last wrote on it.
  final String? phase;
  final IssueQuestionSummary? question;

  factory LinkedIssueSummary.fromJson(Map<String, dynamic> json) =>
      LinkedIssueSummary(
        issue: json['issue'] as String,
        relation: json['relation'] as String,
        state: json['state'] as String,
        url: json['url'] as String?,
        title: json['title'] as String?,
        issueState: json['issue_state'] as String?,
        labels: ((json['labels'] as List<dynamic>?) ?? const []).cast<String>(),
        phase: json['phase'] as String?,
        question: json['open_question'] == null
            ? null
            : IssueQuestionSummary.fromJson(
                json['open_question'] as Map<String, dynamic>,
              ),
      );
}

/// One of the Mission's writes to a tracker, without its text.
class IssueEntrySummary {
  const IssueEntrySummary({
    required this.id,
    required this.kind,
    required this.issue,
    required this.state,
    this.url,
    this.reason,
    this.dropped = const [],
  });

  final String id;
  final String kind;
  final String issue;

  /// `pending`, `held`, `sending`, `posted`, `refused` or `failed`.
  final String state;
  final String? url;
  final String? reason;
  final List<String> dropped;

  factory IssueEntrySummary.fromJson(Map<String, dynamic> json) =>
      IssueEntrySummary(
        id: json['id'] as String,
        kind: json['kind'] as String,
        issue: json['issue'] as String,
        state: json['state'] as String,
        url: json['url'] as String?,
        reason: json['reason'] as String?,
        dropped: ((json['dropped'] as List<dynamic>?) ?? const [])
            .cast<String>(),
      );

  /// What the chip says.
  String get stateWords => switch (state) {
    'pending' || 'sending' => 'queued',
    'held' => 'waiting for green ink',
    final other => other,
  };
}

/// The issues of one Mission, what it posted, and the replies counted.
class MissionIssuesSummary {
  const MissionIssuesSummary({
    this.issues = const [],
    this.entries = const [],
    this.delivered = 0,
    this.ignored = 0,
  });

  final List<LinkedIssueSummary> issues;
  final List<IssueEntrySummary> entries;
  final int delivered;
  final int ignored;

  factory MissionIssuesSummary.fromJson(Map<String, dynamic> json) {
    final replies = (json['replies'] as Map<String, dynamic>?) ?? const {};
    return MissionIssuesSummary(
      issues: ((json['issues'] as List<dynamic>?) ?? const [])
          .map(
            (issue) =>
                LinkedIssueSummary.fromJson(issue as Map<String, dynamic>),
          )
          .toList(growable: false),
      entries: ((json['entries'] as List<dynamic>?) ?? const [])
          .map(
            (entry) =>
                IssueEntrySummary.fromJson(entry as Map<String, dynamic>),
          )
          .toList(growable: false),
      delivered: (replies['delivered'] as num?)?.toInt() ?? 0,
      ignored: (replies['ignored'] as num?)?.toInt() ?? 0,
    );
  }
}

/// One issue an Agent asked for.
class ProposedIssueSummary {
  const ProposedIssueSummary({
    required this.tracker,
    required this.title,
    required this.body,
    this.labels = const [],
    this.parent,
    this.mirrorOf,
  });

  final String tracker;
  final String title;
  final String body;
  final List<String> labels;
  final String? parent;
  final String? mirrorOf;

  factory ProposedIssueSummary.fromJson(Map<String, dynamic> json) =>
      ProposedIssueSummary(
        tracker: json['tracker'] as String,
        title: json['title'] as String,
        body: (json['body'] as String?) ?? '',
        labels: ((json['labels'] as List<dynamic>?) ?? const []).cast<String>(),
        parent: json['parent'] as String?,
        mirrorOf: json['mirror_of'] as String?,
      );
}

/// What an Agent asked to create on a tracker.
class IssueProposalSummary {
  const IssueProposalSummary({
    required this.id,
    required this.missionId,
    required this.proposer,
    required this.state,
    required this.specs,
    this.findings = const [],
    this.dropped = const [],
    this.exposure = const [],
    this.created = const [],
    this.failed = const [],
    this.markReady = false,
  });

  final String id;
  final String missionId;
  final String proposer;

  /// `pending`, `approved`, `rejected` or `created`.
  final String state;
  final List<ProposedIssueSummary> specs;
  final List<String> findings;
  final List<String> dropped;
  final List<String> exposure;
  final List<String> created;
  final List<String> failed;
  final bool markReady;

  factory IssueProposalSummary.fromJson(Map<String, dynamic> json) {
    List<String> strings(String key) =>
        ((json[key] as List<dynamic>?) ?? const []).cast<String>();
    return IssueProposalSummary(
      id: json['id'] as String,
      missionId: json['mission_id'] as String,
      proposer: json['proposer'] as String,
      state: json['state'] as String,
      specs: ((json['specs'] as List<dynamic>?) ?? const [])
          .map(
            (spec) =>
                ProposedIssueSummary.fromJson(spec as Map<String, dynamic>),
          )
          .toList(growable: false),
      findings: strings('findings'),
      dropped: strings('dropped'),
      exposure: strings('exposure'),
      created: strings('created'),
      failed: strings('failed'),
      markReady: json['mark_ready'] as bool? ?? false,
    );
  }
}

/// A button shown to everyone: without the action it is disabled, and says what it needs.
class _ActionButton extends StatelessWidget {
  const _ActionButton({
    required this.label,
    required this.action,
    required this.allowed,
    required this.onPressed,
  });

  final String label;
  final String action;
  final bool allowed;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final button = TextButton(
      onPressed: allowed ? onPressed : null,
      child: Text(label),
    );
    return allowed
        ? button
        : Tooltip(message: '$label needs $action', child: button);
  }
}

/// The issues a Mission is linked to: each with its relation, its name opening it outside the
/// app, title, state, labels, phase and open question; then what the Mission posted, with the
/// state of each. Issue text is shown as data, with what renders as nothing spelled out.
class LinkedIssuesSection extends StatefulWidget {
  const LinkedIssuesSection({
    required this.board,
    required this.operationId,
    required this.missionId,
    super.key,
  });

  final MissionBoard board;
  final String operationId;
  final String missionId;

  @override
  State<LinkedIssuesSection> createState() => _LinkedIssuesSectionState();
}

class _LinkedIssuesSectionState extends State<LinkedIssuesSection> {
  late Future<MissionIssuesSummary> _issues = _load();

  Future<MissionIssuesSummary> _load() =>
      widget.board.loadMissionIssues(widget.operationId, widget.missionId);

  bool _may(String action) =>
      !widget.board.demo &&
      widget.board.hasOperationAction(widget.operationId, action);

  Future<void> _write(Future<void> Function() work) async {
    final messenger = ScaffoldMessenger.of(context);
    try {
      await work();
    } on Object catch (error) {
      messenger.showSnackBar(SnackBar(content: Text(error.toString())));
    }
    if (mounted) {
      setState(() {
        _issues = _load();
      });
    }
  }

  Future<String?> _askText(String title, String label) => showDialog<String>(
    context: context,
    builder: (context) => _TextDialog(title: title, label: label),
  );

  Future<void> _comment(LinkedIssueSummary issue) async {
    final body = await _askText('Comment on ${issue.issue}', 'Comment');
    if (body == null || body.trim().isEmpty) return;
    await _write(
      () => widget.board.commentOnIssue(
        widget.operationId,
        widget.missionId,
        issue.issue,
        body,
      ),
    );
  }

  Future<void> _link() async {
    final issue = await _askText('Link an issue', 'TRACKER:ITEM');
    if (issue == null || !issue.contains(':')) return;
    await _write(
      () => widget.board.linkIssue(widget.operationId, widget.missionId, issue),
    );
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    return FutureBuilder<MissionIssuesSummary>(
      future: _issues,
      builder: (context, snapshot) {
        final loaded = snapshot.data;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text('LINKED ISSUES', style: ooSectionLabel(context)),
                ),
                _ActionButton(
                  label: 'Link an issue',
                  action: 'issue.link',
                  allowed: _may('issue.link'),
                  onPressed: _link,
                ),
              ],
            ),
            if (snapshot.hasError)
              Text(
                'Linked issues could not be read.',
                style: TextStyle(color: tokens.muted),
              )
            else if (loaded == null)
              const SizedBox(height: 2, child: LinearProgressIndicator())
            else if (loaded.issues.isEmpty)
              Text(
                'No issue is linked to this Mission.',
                style: TextStyle(color: tokens.muted),
              )
            else ...[
              for (final issue in loaded.issues) _issueCard(issue),
              if (loaded.delivered > 0 || loaded.ignored > 0)
                Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: Text(
                    '${loaded.delivered} replies reached the Agent · '
                    '${loaded.ignored} ignored',
                    style: text.bodySmall?.copyWith(color: tokens.muted),
                  ),
                ),
            ],
            if (loaded != null && loaded.entries.isNotEmpty) ...[
              const SizedBox(height: 6),
              Text('POSTS', style: ooSectionLabel(context)),
              const SizedBox(height: 4),
              for (final entry in loaded.entries) _entryRow(entry),
            ],
          ],
        );
      },
    );
  }

  Widget _issueCard(LinkedIssueSummary issue) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final url = issue.url == null ? null : Uri.tryParse(issue.url!);
    final question = issue.question;
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Wrap(
              spacing: 8,
              runSpacing: 2,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                Text(issue.relation, style: text.labelMedium),
                if (url == null)
                  Text(issue.issue, style: text.titleSmall)
                else
                  TextButton.icon(
                    onPressed: () => launchPage(url),
                    icon: const Icon(Icons.open_in_new, size: 16),
                    label: Text(issue.issue),
                  ),
                if (issue.state == 'proposed')
                  const Text('proposed by an Agent'),
              ],
            ),
            if (issue.title case final title?)
              Text(revealInvisible(title), style: text.bodyMedium),
            Text(
              [
                ?issue.issueState,
                if (issue.labels.isNotEmpty)
                  issue.labels.map(revealInvisible).join(', '),
                if (issue.phase case final phase?) 'AI6 wrote: $phase',
              ].join(' · '),
              style: text.bodySmall?.copyWith(color: tokens.muted),
            ),
            if (question != null)
              Text(
                'Waiting for an answer: asked '
                '${widget.board.describeAge(question.askedAt)}',
                style: text.bodySmall?.copyWith(color: tokens.warning),
              ),
            Wrap(
              spacing: 4,
              children: [
                if (issue.state == 'proposed')
                  _ActionButton(
                    label: 'Accept',
                    action: 'issue.link',
                    allowed: _may('issue.link'),
                    onPressed: () => _write(
                      () => widget.board.acceptIssue(
                        widget.operationId,
                        widget.missionId,
                        issue.issue,
                      ),
                    ),
                  ),
                if (issue.state == 'active') ...[
                  _ActionButton(
                    label: 'Comment',
                    action: 'issue.comment',
                    allowed: _may('issue.comment'),
                    onPressed: () => _comment(issue),
                  ),
                  _ActionButton(
                    label: 'Close',
                    action: 'issue.update',
                    allowed: _may('issue.update'),
                    onPressed: () => _write(
                      () => widget.board.closeIssue(
                        widget.operationId,
                        widget.missionId,
                        issue.issue,
                      ),
                    ),
                  ),
                ],
                _ActionButton(
                  label: 'Unlink',
                  action: 'issue.link',
                  allowed: _may('issue.link'),
                  onPressed: () => _write(
                    () => widget.board.unlinkIssue(
                      widget.operationId,
                      widget.missionId,
                      issue.issue,
                    ),
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _entryRow(IssueEntrySummary entry) {
    final text = Theme.of(context).textTheme;
    final refused = entry.state == 'refused' || entry.state == 'failed';
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Wrap(
        spacing: 8,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Chip(
            label: Text(entry.stateWords),
            visualDensity: VisualDensity.compact,
          ),
          Text('${entry.kind} · ${entry.issue}', style: text.bodySmall),
          if (refused && entry.reason != null)
            Text(revealInvisible(entry.reason!), style: text.bodySmall),
          if (entry.dropped.isNotEmpty)
            Text(
              'Not carried over: ${entry.dropped.join(', ')}',
              style: text.bodySmall,
            ),
        ],
      ),
    );
  }
}

class _TextDialog extends StatefulWidget {
  const _TextDialog({required this.title, required this.label});

  final String title;
  final String label;

  @override
  State<_TextDialog> createState() => _TextDialogState();
}

class _TextDialogState extends State<_TextDialog> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.title),
    content: TextField(
      controller: _controller,
      autofocus: true,
      minLines: 1,
      maxLines: 6,
      decoration: InputDecoration(labelText: widget.label),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: () => Navigator.of(context).pop(_controller.text),
        child: const Text('Send'),
      ),
    ],
  );
}

/// What Agents asked to create in an Operation, each with its destination, the exact text, the
/// fields a copy cannot carry and what vetting found. A person approves or rejects each; ticking
/// "mark ready" needs `mission.create` too.
class IssueProposalsPanel extends StatefulWidget {
  const IssueProposalsPanel({
    required this.board,
    required this.operationId,
    this.itemId,
    super.key,
  });

  final MissionBoard board;
  final String operationId;

  /// The one proposal the route names, when it names one.
  final String? itemId;

  @override
  State<IssueProposalsPanel> createState() => _IssueProposalsPanelState();
}

class _IssueProposalsPanelState extends State<IssueProposalsPanel> {
  late Future<List<IssueProposalSummary>> _proposals = _load();
  final Set<String> _ready = {};
  bool _deciding = false;

  Future<List<IssueProposalSummary>> _load() =>
      widget.board.loadIssueProposals(widget.operationId);

  @override
  void didUpdateWidget(IssueProposalsPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.operationId != widget.operationId) _proposals = _load();
  }

  Future<void> _decide(IssueProposalSummary proposal, bool approve) async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _deciding = true);
    try {
      await widget.board.decideIssueProposal(
        widget.operationId,
        proposal.id,
        approve: approve,
        markReady: approve && _ready.contains(proposal.id),
      );
    } on Object catch (error) {
      messenger.showSnackBar(SnackBar(content: Text(error.toString())));
    }
    if (mounted) {
      setState(() {
        _deciding = false;
        _proposals = _load();
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return FutureBuilder<List<IssueProposalSummary>>(
      future: _proposals,
      builder: (context, snapshot) {
        final all = snapshot.data;
        if (all == null) return const SizedBox.shrink();
        final shown = [
          for (final proposal in all)
            if (proposal.state == 'pending' &&
                (widget.itemId == null || proposal.id == widget.itemId))
              proposal,
        ];
        if (shown.isEmpty) return const SizedBox.shrink();
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('ISSUE PROPOSALS', style: ooSectionLabel(context)),
            const SizedBox(height: 6),
            for (final proposal in shown) _card(proposal),
          ],
        );
      },
    );
  }

  Widget _card(IssueProposalSummary proposal) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final board = widget.board;
    final mayDecide =
        !board.demo &&
        board.hasOperationAction(widget.operationId, 'issue.create');
    final mayReady =
        mayDecide &&
        board.hasOperationAction(widget.operationId, 'mission.create');
    final buttons = _deciding || !mayDecide;
    return Card(
      margin: const EdgeInsets.only(bottom: 10),
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(proposal.proposer, style: text.labelMedium),
            OoLinkLine([
              (
                'Mission ${proposal.missionId}',
                OoLinkTo.mission(widget.operationId, proposal.missionId),
              ),
            ]),
            for (final spec in proposal.specs) ...[
              const SizedBox(height: 6),
              Text(
                [
                  'In ${spec.tracker}',
                  if (spec.parent case final parent?) 'child of $parent',
                  if (spec.mirrorOf case final original?) 'copy of $original',
                ].join(' · '),
                style: text.bodySmall?.copyWith(color: tokens.muted),
              ),
              Text(revealInvisible(spec.title), style: text.titleSmall),
              if (spec.body.isNotEmpty)
                SelectableText(
                  revealInvisible(spec.body),
                  style: text.bodySmall,
                ),
              if (spec.labels.isNotEmpty)
                Text(
                  'Labels: ${spec.labels.join(', ')}',
                  style: text.bodySmall,
                ),
            ],
            if (proposal.dropped.isNotEmpty)
              Text(
                'Not carried over: ${proposal.dropped.join(', ')}',
                style: text.bodySmall,
              ),
            if (proposal.findings.isNotEmpty)
              Text(
                'Vetting found: ${proposal.findings.join(', ')}',
                style: text.bodySmall?.copyWith(color: tokens.warning),
              ),
            if (proposal.exposure.isNotEmpty)
              Text(
                'Its Session read outside text: ${proposal.exposure.join(', ')}',
                style: text.bodySmall?.copyWith(color: tokens.warning),
              ),
            Tooltip(
              message: mayReady ? '' : 'Mark ready needs mission.create',
              child: CheckboxListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                title: const Text('Mark ready, so intake may take it'),
                value: _ready.contains(proposal.id),
                onChanged: !mayReady
                    ? null
                    : (ticked) => setState(() {
                        if (ticked == true) {
                          _ready.add(proposal.id);
                        } else {
                          _ready.remove(proposal.id);
                        }
                      }),
              ),
            ),
            Wrap(
              spacing: 8,
              children: [
                OutlinedButton(
                  onPressed: buttons ? null : () => _decide(proposal, false),
                  child: const Text('Reject'),
                ),
                FilledButton(
                  onPressed: buttons ? null : () => _decide(proposal, true),
                  child: const Text('Approve and create'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
