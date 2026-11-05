import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../shell/oo_link.dart';

/// What the scorecard calls an attempt's outcome, in the order a row lists them, with the words
/// a person reads. The first five are the Agent's own and are judged; the rest are shown and
/// not held against it.
const scorecardJudgments = <(String, String)>[
  ('done', 'done'),
  ('failed', 'failed'),
  ('blown', 'blown'),
  ('stalled', 'stalled'),
  ('disputed', 'disputed'),
  ('waiting_on_person', 'waiting on a person'),
  ('budget_stop', 'budget stops'),
  ('parked', 'parked by a limit'),
  ('lost', 'lost with a Station'),
  ('handed_off', 'handed to another'),
  ('withdrawn', 'withdrawn by a person'),
  ('running', 'running'),
  ('unrecorded', 'no outcome recorded'),
];

/// One attempt behind a number.
@immutable
class ScorecardAttempt {
  const ScorecardAttempt({required this.missionId, required this.sessionId});

  final String missionId;
  final String sessionId;

  factory ScorecardAttempt.fromJson(Map<String, dynamic> json) =>
      ScorecardAttempt(
        missionId: json['mission_id'] as String,
        sessionId: json['session_id'] as String,
      );
}

/// What a group of attempts came to.
@immutable
class ScorecardStanding {
  const ScorecardStanding({
    this.attempts = const {},
    this.judged = 0,
    this.failureRate,
    this.smallSample = true,
    this.refusedTools = 0,
    this.tokens = 0,
    this.costMicroUsd = 0,
    this.wallSecs = 0,
  });

  /// The attempts of each judgment, by its wire name.
  final Map<String, List<ScorecardAttempt>> attempts;
  final int judged;
  final double? failureRate;

  /// Too few judged attempts to rank on.
  final bool smallSample;
  final int refusedTools;
  final int tokens;
  final int costMicroUsd;
  final double wallSecs;

  factory ScorecardStanding.fromJson(Map<String, dynamic> json) {
    final usage = json['usage'] as Map<String, dynamic>? ?? const {};
    return ScorecardStanding(
      attempts: {
        for (final entry
            in (json['attempts'] as Map<String, dynamic>? ?? const {}).entries)
          entry.key: [
            for (final attempt in entry.value as List<dynamic>)
              ScorecardAttempt.fromJson(attempt as Map<String, dynamic>),
          ],
      },
      judged: json['judged'] as int? ?? 0,
      failureRate: (json['failure_rate'] as num?)?.toDouble(),
      smallSample: json['small_sample'] as bool? ?? true,
      refusedTools: json['refused_tools'] as int? ?? 0,
      tokens: (usage['input'] as int? ?? 0) + (usage['output'] as int? ?? 0),
      costMicroUsd: usage['cost_micro_usd'] as int? ?? 0,
      wallSecs: (json['wall_secs'] as num?)?.toDouble() ?? 0,
    );
  }

  int count(String judgment) => attempts[judgment]?.length ?? 0;

  /// `3 of 4 judged · 25 % failed`, or what stands in its place with nothing judged.
  String get summary {
    if (judged == 0) return 'nothing judged yet';
    final rate = failureRate;
    return [
      '$judged judged',
      if (rate != null) '${(rate * 100).round()} % failed',
      if (smallSample) 'small sample: not ranked',
    ].join(' · ');
  }

  /// `$1.23 · 4,500 tokens · 12 min`.
  String get spend => [
    '\$${(costMicroUsd / 1e6).toStringAsFixed(2)}',
    '$tokens tokens',
    if (refusedTools > 0) '$refusedTools tools refused',
    '${(wallSecs / 60).round()} min',
  ].join(' · ');
}

/// One row of the scorecard: a name and what its attempts came to.
@immutable
class ScorecardRow {
  const ScorecardRow({
    required this.title,
    required this.standing,
    this.children = const [],
  });

  final String title;
  final ScorecardStanding standing;
  final List<ScorecardRow> children;
}

/// An Operation's scorecard.
@immutable
class ScorecardSummary {
  const ScorecardSummary({
    required this.windowDays,
    required this.smallSample,
    required this.agents,
    required this.models,
    this.unattributed = 0,
  });

  final int windowDays;
  final int smallSample;
  final List<ScorecardRow> agents;
  final List<ScorecardRow> models;

  /// Attempts whose record names no Agent.
  final int unattributed;

  factory ScorecardSummary.fromJson(Map<String, dynamic> json) {
    String model(Object? name) => name == null ? 'default model' : '$name';
    return ScorecardSummary(
      windowDays: json['window_days'] as int? ?? 0,
      smallSample: json['small_sample'] as int? ?? 0,
      unattributed: json['unattributed'] as int? ?? 0,
      agents: [
        for (final agent in json['agents'] as List<dynamic>? ?? const [])
          ScorecardRow(
            title: (agent as Map<String, dynamic>)['agent'] as String,
            standing: ScorecardStanding.fromJson(
              agent['standing'] as Map<String, dynamic>,
            ),
            children: [
              for (final route in agent['routes'] as List<dynamic>)
                ScorecardRow(
                  title: [
                    (route as Map<String, dynamic>)['route']['handler'],
                    model(route['route']['model']),
                    if (route['route']['pool'] case final pool?) 'pool $pool',
                  ].join(' · '),
                  standing: ScorecardStanding.fromJson(
                    route['standing'] as Map<String, dynamic>,
                  ),
                ),
            ],
          ),
      ],
      models: [
        for (final entry in json['models'] as List<dynamic>? ?? const [])
          ScorecardRow(
            title: model((entry as Map<String, dynamic>)['model']),
            standing: ScorecardStanding.fromJson(
              entry['standing'] as Map<String, dynamic>,
            ),
          ),
      ],
    );
  }

  String get window =>
      windowDays == 0 ? 'every attempt on record' : 'the last $windowDays days';
}

/// The scorecard of an Operation's Agents, routes and models, with every number opening the
/// Missions behind it.
class ScorecardSection extends StatefulWidget {
  const ScorecardSection({
    required this.board,
    required this.operation,
    super.key,
  });

  final MissionBoard board;
  final String operation;

  @override
  State<ScorecardSection> createState() => _ScorecardSectionState();
}

class _ScorecardSectionState extends State<ScorecardSection> {
  ScorecardSummary? _card;
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void didUpdateWidget(ScorecardSection oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.operation != widget.operation) {
      setState(() => _card = null);
      unawaited(_load());
    }
  }

  Future<void> _load() async {
    final operation = widget.operation;
    try {
      final card = await widget.board.loadScorecard(operation);
      if (!mounted || operation != widget.operation) return;
      setState(() {
        _card = card;
        _error = null;
      });
    } on StateError catch (error) {
      if (mounted) setState(() => _error = error.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final card = _card;
    final theme = Theme.of(context);
    return Card(
      child: ExpansionTile(
        title: const Text('Scorecard'),
        subtitle: Text(
          card == null
              ? (_error ?? 'Reading the Sessions…')
              : 'Over ${card.window}. Done, failed, blown, stalled and disputed '
                    'are the Agent\'s own; the rest are shown, not held against it.',
        ),
        childrenPadding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
        expandedCrossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (card != null) ...[
            for (final row in card.agents)
              _RowTile(
                row: row,
                operation: widget.operation,
                board: widget.board,
                depth: 0,
              ),
            if (card.agents.isEmpty)
              const Text('No Agent has run a Mission here in this window.'),
            if (card.models.isNotEmpty) ...[
              const SizedBox(height: 12),
              Text('By model', style: theme.textTheme.titleSmall),
              for (final row in card.models)
                _RowTile(
                  row: row,
                  operation: widget.operation,
                  board: widget.board,
                  depth: 0,
                ),
            ],
            if (card.unattributed > 0)
              Text(
                '${card.unattributed} attempts name no Agent; they count '
                'toward their model only.',
                style: theme.textTheme.bodySmall,
              ),
            Text(
              'A row with fewer than ${card.smallSample} judged attempts is a '
              'small sample and ranks nothing. Grades compare within one '
              'Operation\'s work; they say nothing of a model in general.',
              style: theme.textTheme.bodySmall,
            ),
          ],
        ],
      ),
    );
  }
}

class _RowTile extends StatelessWidget {
  const _RowTile({
    required this.row,
    required this.operation,
    required this.board,
    required this.depth,
  });

  final ScorecardRow row;
  final String operation;
  final MissionBoard board;
  final int depth;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final standing = row.standing;
    return Padding(
      padding: EdgeInsets.only(left: depth * 16.0, top: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(row.title, style: theme.textTheme.titleSmall),
          Text(standing.summary, style: theme.textTheme.bodySmall),
          Wrap(
            spacing: 4,
            children: [
              for (final (name, words) in scorecardJudgments)
                if (standing.count(name) > 0)
                  ActionChip(
                    label: Text('${standing.count(name)} $words'),
                    tooltip: 'Open the Missions behind this number',
                    onPressed: () {
                      // Resolved here: a dialog sits above the page that knows the links.
                      final opened = [
                        for (final attempt in standing.attempts[name]!)
                          (
                            board.handleOf(operation, attempt.missionId),
                            OoLinks.opener(
                              context,
                              OoLinkTo.mission(operation, attempt.missionId),
                            ),
                          ),
                      ];
                      unawaited(
                        showDialog<void>(
                          context: context,
                          builder: (_) => _AttemptsDialog(
                            title: '${row.title}: $words',
                            missions: opened,
                          ),
                        ),
                      );
                    },
                  ),
            ],
          ),
          Text(standing.spend, style: theme.textTheme.bodySmall),
          for (final child in row.children)
            _RowTile(
              row: child,
              operation: operation,
              board: board,
              depth: depth + 1,
            ),
        ],
      ),
    );
  }
}

class _AttemptsDialog extends StatelessWidget {
  const _AttemptsDialog({required this.title, required this.missions});

  final String title;

  /// Each Mission behind the number and what opens it; null where this account may not.
  final List<(String, VoidCallback?)> missions;

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(title),
    content: SizedBox(
      width: 360,
      child: ListView(
        shrinkWrap: true,
        children: [
          for (final (mission, open) in missions)
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton(
                onPressed: open == null
                    ? null
                    : () {
                        Navigator.of(context).pop();
                        open();
                      },
                child: Text(mission),
              ),
            ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Close'),
      ),
    ],
  );
}
