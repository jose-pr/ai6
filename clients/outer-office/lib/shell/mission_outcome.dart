import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_theme.dart';
import 'oo_markdown.dart';

/// How a Mission ended, as far as a person can check it: its Agent's final message, what a person
/// found, and, for one with `mission.verify`, a way to say what they found.
class MissionOutcomeSection extends StatefulWidget {
  const MissionOutcomeSection({
    required this.board,
    required this.operationId,
    required this.missionId,
    required this.state,
    super.key,
  });

  final MissionBoard board;
  final String operationId;
  final String missionId;

  /// The Mission's state on the board.
  final String state;

  @override
  State<MissionOutcomeSection> createState() => _MissionOutcomeSectionState();
}

class _MissionOutcomeSectionState extends State<MissionOutcomeSection> {
  late Future<MissionAttemptSummary?> _latest = _load();

  Future<MissionAttemptSummary?> _load() =>
      widget.board.loadLatestAttempt(widget.operationId, widget.missionId);

  @override
  void didUpdateWidget(MissionOutcomeSection oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.missionId != widget.missionId ||
        oldWidget.state != widget.state) {
      _latest = _load();
    }
  }

  bool get _settled => const {'done', 'held'}.contains(widget.state);

  bool get _mayJudge =>
      _settled &&
      !widget.board.demo &&
      widget.board.hasOperationAction(widget.operationId, 'mission.verify');

  Future<void> _judge(String verdict) async {
    final messenger = ScaffoldMessenger.of(context);
    final note = await showDialog<String>(
      context: context,
      builder: (context) => _VerdictDialog(verdict: verdict),
    );
    if (note == null) return;
    try {
      await widget.board.giveVerdict(
        operationId: widget.operationId,
        missionId: widget.missionId,
        verdict: verdict,
        note: note.trim().isEmpty ? null : note.trim(),
      );
    } on Object catch (error) {
      messenger.showSnackBar(SnackBar(content: Text(error.toString())));
    }
    if (mounted) {
      setState(() {
        _latest = _load();
      });
    }
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<MissionAttemptSummary?>(
    future: _latest,
    builder: (context, snapshot) {
      final text = Theme.of(context).textTheme;
      final tokens = OoTokens.of(context);
      final attempt = snapshot.data;
      final report = attempt?.finalReport;
      final verdict = attempt?.verdict;
      if (widget.state == 'verifying') return const SizedBox.shrink();
      if (report == null && verdict == null && !_settled) {
        return const SizedBox.shrink();
      }
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('OUTCOME', style: ooSectionLabel(context)),
          const SizedBox(height: 8),
          if (widget.state == 'held')
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Text(
                'Held: its Session ended done, and its role requires a green '
                'ink decision it does not have. A verdict settles it.',
                style: text.bodyMedium,
              ),
            ),
          if (verdict != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Text(verdict.describe(), style: text.titleSmall),
            ),
          if (report != null)
            OoMarkdown(report)
          else if (snapshot.connectionState == ConnectionState.done)
            Text(
              'No final message is on record.',
              style: TextStyle(color: tokens.muted),
            ),
          if (_mayJudge)
            Wrap(
              spacing: 8,
              children: [
                TextButton(
                  onPressed: () => _judge('confirmed'),
                  child: const Text('Confirm it'),
                ),
                TextButton(
                  onPressed: () => _judge('disputed'),
                  child: const Text('Dispute it'),
                ),
                TextButton(
                  onPressed: () => _judge('failed-verification'),
                  child: const Text('It failed verification'),
                ),
              ],
            ),
          const SizedBox(height: 20),
        ],
      );
    },
  );
}

class _VerdictDialog extends StatefulWidget {
  const _VerdictDialog({required this.verdict});

  final String verdict;

  @override
  State<_VerdictDialog> createState() => _VerdictDialogState();
}

class _VerdictDialogState extends State<_VerdictDialog> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text('Verdict: ${verdictWords(widget.verdict)}'),
    content: TextField(
      controller: _controller,
      autofocus: true,
      minLines: 1,
      maxLines: 6,
      decoration: const InputDecoration(labelText: 'Why (recorded)'),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: () => Navigator.of(context).pop(_controller.text),
        child: const Text('Record it'),
      ),
    ],
  );
}
