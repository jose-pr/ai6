import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_theme.dart';
import 'oo_freshness.dart';
import 'oo_markdown.dart';

/// A Mission whose Session ended done and that waits for a decision: what it was to deliver
/// with what the Station's probes found of each, the Agent's final report, and, for a person
/// with `mission.verify`, a way to probe again and to decide.
class MissionVerificationPanel extends StatefulWidget {
  const MissionVerificationPanel({
    required this.board,
    required this.operationId,
    required this.missionId,
    super.key,
  });

  final MissionBoard board;
  final String operationId;
  final String missionId;

  @override
  State<MissionVerificationPanel> createState() =>
      _MissionVerificationPanelState();
}

class _MissionVerificationPanelState extends State<MissionVerificationPanel> {
  late Future<MissionAttemptSummary?> _latest = _load();
  bool _busy = false;
  String? _refusal;

  Future<MissionAttemptSummary?> _load() =>
      widget.board.loadLatestAttempt(widget.operationId, widget.missionId);

  @override
  void didUpdateWidget(MissionVerificationPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.missionId != widget.missionId ||
        oldWidget.operationId != widget.operationId) {
      _latest = _load();
      _refusal = null;
    }
  }

  bool get _mayDecide =>
      !widget.board.demo &&
      widget.board.hasOperationAction(widget.operationId, 'mission.verify');

  /// Runs [action], showing the gateway's own words when it refuses.
  Future<void> _run(Future<void> Function() action) async {
    setState(() {
      _busy = true;
      _refusal = null;
    });
    try {
      await action();
    } on Object catch (error) {
      _refusal = error.toString();
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _latest = _load();
    });
  }

  Future<void> _decide(String decision, int attempt) async {
    final answer = await showDialog<_Decision>(
      context: context,
      builder: (context) =>
          _DecisionDialog(decision: decision, attempt: attempt),
    );
    if (answer == null) return;
    await _run(
      () => widget.board.verifyMission(
        operationId: widget.operationId,
        missionId: widget.missionId,
        decision: decision,
        reason: answer.reason,
        amendment: answer.amendment,
      ),
    );
  }

  Future<void> _probe() => _run(() async {
    await widget.board.probeMission(
      operationId: widget.operationId,
      missionId: widget.missionId,
    );
  });

  @override
  Widget build(BuildContext context) => FutureBuilder<MissionAttemptSummary?>(
    future: _latest,
    builder: (context, snapshot) {
      final text = Theme.of(context).textTheme;
      final tokens = OoTokens.of(context);
      final attempt = snapshot.data;
      final report = attempt?.finalReport;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('VERIFYING', style: ooSectionLabel(context)),
          const SizedBox(height: 8),
          Text(
            'Waiting for a decision: '
            '${attempt == null ? 'its' : 'attempt ${attempt.attempt}\'s'} '
            'Session ended done, and the Mission is not done until it is '
            'decided.',
            style: text.bodyMedium,
          ),
          const SizedBox(height: 10),
          if (snapshot.hasError)
            Text(
              'The attempt could not be loaded.',
              style: TextStyle(color: tokens.muted),
            )
          else if (attempt == null &&
              snapshot.connectionState != ConnectionState.done)
            const LinearProgressIndicator()
          else if (attempt != null) ...[
            Text('DELIVERABLES', style: ooSectionLabel(context)),
            const SizedBox(height: 4),
            if (attempt.deliverables.isEmpty)
              Text(
                'This Mission declared nothing to check.',
                style: TextStyle(color: tokens.muted),
              ),
            for (final deliverable in attempt.deliverables)
              _DeliverableRow(
                deliverable: deliverable,
                evidence: attempt.evidence.lastWhereOrNull(
                  (item) => item.deliverable == deliverable,
                ),
              ),
            if (attempt.probeRequested != null)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  'A probe was asked for; the Station has not run it yet.',
                  style: text.bodySmall?.copyWith(color: tokens.muted),
                ),
              ),
            const SizedBox(height: 10),
            Text('FINAL REPORT', style: ooSectionLabel(context)),
            const SizedBox(height: 4),
            if (report != null)
              OoMarkdown(report)
            else
              Text(
                'No final message is on record.',
                style: TextStyle(color: tokens.muted),
              ),
            if (_mayDecide) ...[
              const SizedBox(height: 10),
              Wrap(
                spacing: 8,
                runSpacing: 4,
                children: [
                  OutlinedButton(
                    onPressed: _busy ? null : _probe,
                    child: const Text('Probe now'),
                  ),
                  FilledButton(
                    onPressed: _busy
                        ? null
                        : () => _decide('done', attempt.attempt),
                    child: const Text('Done'),
                  ),
                  TextButton(
                    onPressed: _busy
                        ? null
                        : () => _decide('redispatch', attempt.attempt),
                    child: Text('Redispatch as attempt ${attempt.attempt + 1}'),
                  ),
                  TextButton(
                    onPressed: _busy
                        ? null
                        : () => _decide('failed', attempt.attempt),
                    child: const Text('Failed'),
                  ),
                ],
              ),
            ],
          ],
          if (_refusal case final refusal?)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                refusal,
                style: text.bodyMedium?.copyWith(color: tokens.danger),
              ),
            ),
          const SizedBox(height: 20),
        ],
      );
    },
  );
}

extension<T> on List<T> {
  T? lastWhereOrNull(bool Function(T) test) {
    for (final item in reversed) {
      if (test(item)) return item;
    }
    return null;
  }
}

class _DeliverableRow extends StatelessWidget {
  const _DeliverableRow({required this.deliverable, required this.evidence});

  final String deliverable;

  /// The latest probe of it, when one has run.
  final ProbeEvidenceSummary? evidence;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final found = evidence;
    final color = switch (found?.result) {
      'found' => tokens.success,
      'missing' || 'failed' => tokens.danger,
      _ => tokens.muted,
    };
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Wrap(
        spacing: 10,
        runSpacing: 2,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Text(deliverable, style: text.titleSmall),
          Text(
            found?.result ?? 'not probed yet',
            style: text.labelMedium?.copyWith(color: color),
          ),
          if (found != null && found.detail.isNotEmpty)
            Text(found.detail, style: text.bodySmall),
          if (found != null) OoTime(found.at, prefix: 'Probed '),
        ],
      ),
    );
  }
}

class _Decision {
  const _Decision(this.reason, this.amendment);

  final String reason;
  final String? amendment;
}

class _DecisionDialog extends StatefulWidget {
  const _DecisionDialog({required this.decision, required this.attempt});

  final String decision;
  final int attempt;

  @override
  State<_DecisionDialog> createState() => _DecisionDialogState();
}

class _DecisionDialogState extends State<_DecisionDialog> {
  final _reason = TextEditingController();
  final _amendment = TextEditingController();

  @override
  void dispose() {
    _reason.dispose();
    _amendment.dispose();
    super.dispose();
  }

  bool get _redispatch => widget.decision == 'redispatch';

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(switch (widget.decision) {
      'done' => 'Decide: done',
      'failed' => 'Decide: failed',
      _ => 'Decide: redispatch as attempt ${widget.attempt + 1}',
    }),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (_redispatch)
          const Padding(
            padding: EdgeInsets.only(bottom: 8),
            child: Text(
              'The Mission is queued again as the next attempt, and its new '
              'Session is told the reason and the amendment after its brief.',
            ),
          ),
        TextField(
          controller: _reason,
          autofocus: true,
          minLines: 1,
          maxLines: 6,
          onChanged: (_) => setState(() {}),
          decoration: const InputDecoration(labelText: 'Reason (required)'),
        ),
        if (_redispatch)
          TextField(
            controller: _amendment,
            minLines: 1,
            maxLines: 6,
            decoration: const InputDecoration(labelText: 'Amendment'),
          ),
      ],
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: _reason.text.trim().isEmpty
            ? null
            : () => Navigator.of(context).pop(
                _Decision(
                  _reason.text.trim(),
                  _redispatch && _amendment.text.trim().isNotEmpty
                      ? _amendment.text.trim()
                      : null,
                ),
              ),
        child: const Text('Record it'),
      ),
    ],
  );
}
