import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_theme.dart';

/// One reviewer's vote in a round. Its reasons are the reviewer's own words: data.
class ReviewVote {
  const ReviewVote({
    required this.iteration,
    required this.round,
    required this.agent,
    required this.model,
    required this.weight,
    required this.vote,
    this.reasons = const [],
  });

  final int iteration;
  final int round;
  final String agent;
  final String model;
  final int weight;

  /// `pass`, `redo`, `abstain` or `absent`.
  final String vote;
  final List<String> reasons;

  factory ReviewVote.fromJson(Map<String, dynamic> json) => ReviewVote(
    iteration: json['iteration'] as int,
    round: json['round'] as int,
    agent: json['agent'] as String,
    model: json['model'] as String,
    weight: json['weight'] as int,
    vote: json['vote'] as String,
    reasons: ((json['reasons'] as List<dynamic>?) ?? const []).cast<String>(),
  );
}

/// One seat of a panel and who sits in it.
class ReviewSeat {
  const ReviewSeat({
    required this.agent,
    required this.model,
    required this.weight,
    required this.veto,
  });

  final String agent;
  final String model;
  final int weight;
  final bool veto;

  factory ReviewSeat.fromJson(Map<String, dynamic> json) => ReviewSeat(
    agent: json['agent'] as String,
    model: json['model'] as String,
    weight: json['weight'] as int,
    veto: json['veto'] as bool? ?? false,
  );
}

/// One gate's review of a Mission, as the gateway shows it.
class MissionReviewSummary {
  const MissionReviewSummary({
    required this.gate,
    required this.policy,
    required this.status,
    required this.iteration,
    required this.round,
    required this.sentBack,
    required this.redoLimit,
    required this.passAt,
    this.why,
    this.panel = const [],
    this.votes = const [],
    this.turns = 0,
    this.tokens = 0,
    this.overruledBy,
  });

  /// `plan` or `output`.
  final String gate;
  final String policy;

  /// `reviewing`, `waiting`, `person`, `sending_back`, `redoing`, `passed` or `refused`.
  final String status;
  final int iteration;
  final int round;
  final int sentBack;
  final int redoLimit;
  final double passAt;
  final String? why;
  final List<ReviewSeat> panel;
  final List<ReviewVote> votes;
  final int turns;
  final int tokens;
  final String? overruledBy;

  /// Whether a person may still decide over it.
  bool get open =>
      status == 'reviewing' || status == 'waiting' || status == 'person';

  String get statusWords => switch (status) {
    'reviewing' => 'the panel is reviewing',
    'waiting' => 'waiting for a panel',
    'person' => 'waiting for a person',
    'sending_back' || 'redoing' => 'sent back to the author',
    'passed' => 'passed',
    'refused' => 'refused',
    final other => other,
  };

  factory MissionReviewSummary.fromJson(Map<String, dynamic> json) =>
      MissionReviewSummary(
        gate: json['gate'] as String,
        policy: json['policy'] as String,
        status: json['status'] as String,
        iteration: json['iteration'] as int,
        round: json['round'] as int,
        sentBack: json['sent_back'] as int,
        redoLimit: json['redo_limit'] as int,
        passAt: (json['pass_at'] as num).toDouble(),
        why: json['why'] as String?,
        panel: ((json['panel'] as List<dynamic>?) ?? const [])
            .map((seat) => ReviewSeat.fromJson(seat as Map<String, dynamic>))
            .toList(),
        votes: ((json['votes'] as List<dynamic>?) ?? const [])
            .map((vote) => ReviewVote.fromJson(vote as Map<String, dynamic>))
            .toList(),
        turns: json['turns'] as int? ?? 0,
        tokens: json['tokens'] as int? ?? 0,
        overruledBy:
            (json['overrule'] as Map<String, dynamic>?)?['by'] as String?,
      );
}

/// A Mission's reviews: each gate's panel, weights, votes with their reasons and rounds, and, for a
/// person with `mission.review`, a way to decide over the panel with a reason.
class MissionReviewsSection extends StatefulWidget {
  const MissionReviewsSection({
    required this.board,
    required this.operationId,
    required this.missionId,
    super.key,
  });

  final MissionBoard board;
  final String operationId;
  final String missionId;

  @override
  State<MissionReviewsSection> createState() => _MissionReviewsSectionState();
}

class _MissionReviewsSectionState extends State<MissionReviewsSection> {
  late Future<List<MissionReviewSummary>> _reviews = _load();

  Future<List<MissionReviewSummary>> _load() =>
      widget.board.loadMissionReviews(widget.operationId, widget.missionId);

  bool get _mayOverrule =>
      !widget.board.demo &&
      widget.board.hasOperationAction(widget.operationId, 'mission.review');

  Future<void> _overrule(MissionReviewSummary review, String decision) async {
    final messenger = ScaffoldMessenger.of(context);
    final reason = await showDialog<String>(
      context: context,
      builder: (context) =>
          _ReasonDialog(decision: decision, gate: review.gate),
    );
    if (reason == null || reason.trim().isEmpty) return;
    try {
      await widget.board.overruleReview(
        widget.operationId,
        widget.missionId,
        review.gate,
        decision,
        reason.trim(),
      );
    } on Object catch (error) {
      messenger.showSnackBar(SnackBar(content: Text(error.toString())));
    }
    if (mounted) {
      setState(() {
        _reviews = _load();
      });
    }
  }

  @override
  Widget build(BuildContext context) =>
      FutureBuilder<List<MissionReviewSummary>>(
        future: _reviews,
        builder: (context, snapshot) {
          final reviews = snapshot.data ?? const <MissionReviewSummary>[];
          if (reviews.isEmpty) return const SizedBox.shrink();
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('REVIEW', style: ooSectionLabel(context)),
              const SizedBox(height: 8),
              for (final review in reviews)
                _ReviewCard(
                  review: review,
                  onOverrule: _mayOverrule && review.open
                      ? (decision) => _overrule(review, decision)
                      : null,
                ),
            ],
          );
        },
      );
}

class _ReviewCard extends StatelessWidget {
  const _ReviewCard({required this.review, this.onOverrule});

  final MissionReviewSummary review;
  final ValueChanged<String>? onOverrule;

  @override
  Widget build(BuildContext context) {
    final text = Theme.of(context).textTheme;
    final tokens = OoTokens.of(context);
    final share = (review.passAt * 100).round();
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '${review.gate == 'plan' ? 'Plan' : 'Output'}: ${review.statusWords}',
              style: text.titleSmall,
            ),
            Text(
              'Policy ${review.policy}, $share% of the weight must pass. '
              'Time ${review.iteration}, round ${review.round}, sent back '
              '${review.sentBack} of ${review.redoLimit}. '
              '${review.turns} reviewer turns, ${review.tokens} tokens.',
              style: text.bodySmall?.copyWith(color: tokens.muted),
            ),
            if (review.why != null)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(review.why!, style: text.bodyMedium),
              ),
            const SizedBox(height: 8),
            for (final seat in review.panel)
              Text(
                '${seat.agent} (${seat.model}), weight ${seat.weight}'
                '${seat.veto ? ', veto' : ''}',
                style: text.bodySmall,
              ),
            const SizedBox(height: 8),
            for (final vote in review.votes)
              Semantics(
                label: 'Round ${vote.round}: ${vote.agent} voted ${vote.vote}',
                child: Padding(
                  padding: const EdgeInsets.only(bottom: 4),
                  child: Text(
                    'Time ${vote.iteration}, round ${vote.round}: ${vote.agent} '
                    'voted ${vote.vote} (weight ${vote.weight})'
                    '${vote.reasons.isEmpty ? '' : ': ${vote.reasons.join('; ')}'}',
                    style: text.bodySmall,
                  ),
                ),
              ),
            if (review.overruledBy != null)
              Text('Decided by ${review.overruledBy}', style: text.bodySmall),
            if (onOverrule != null)
              Wrap(
                spacing: 8,
                children: [
                  TextButton(
                    onPressed: () => onOverrule!('pass'),
                    child: const Text('Pass it'),
                  ),
                  TextButton(
                    onPressed: () => onOverrule!('redo'),
                    child: const Text('Send it back'),
                  ),
                  TextButton(
                    onPressed: () => onOverrule!('refuse'),
                    child: const Text('Refuse it'),
                  ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

class _ReasonDialog extends StatefulWidget {
  const _ReasonDialog({required this.decision, required this.gate});

  final String decision;
  final String gate;

  @override
  State<_ReasonDialog> createState() => _ReasonDialogState();
}

class _ReasonDialogState extends State<_ReasonDialog> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text('Decide the ${widget.gate} review: ${widget.decision}'),
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
        child: const Text('Decide'),
      ),
    ],
  );
}
