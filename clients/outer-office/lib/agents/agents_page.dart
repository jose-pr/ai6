import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../roster/record_editor.dart';
import '../routes/oo_route.dart';
import '../settings/oo_config.dart';
import '../shell/oo_empty.dart';
import '../shell/oo_link.dart';
import '../theme/oo_icon.dart';
import 'scorecard.dart';

/// The clearances, lowest first.
const clearances = ['desk', 'field', '00'];

/// Counts of an Agent's outcomes in one Operation or at one clearance.
@immutable
class AgentTallySummary {
  const AgentTallySummary({
    this.done = 0,
    this.failed = 0,
    this.blown = 0,
    this.abandoned = 0,
    this.stalled = 0,
    this.lost = 0,
    this.quarantined = 0,
    this.budgetStops = 0,
    this.greenInkRejected = 0,
  });

  final int done;
  final int failed;
  final int blown;
  final int abandoned;
  final int stalled;

  /// Sessions lost with their Station or runner: not the Agent's failures.
  final int lost;
  final int quarantined;
  final int budgetStops;
  final int greenInkRejected;

  factory AgentTallySummary.fromJson(Map<String, dynamic>? json) => json == null
      ? const AgentTallySummary()
      : AgentTallySummary(
          done: json['done'] as int? ?? 0,
          failed: json['failed'] as int? ?? 0,
          blown: json['blown'] as int? ?? 0,
          abandoned: json['abandoned'] as int? ?? 0,
          stalled: json['stalled'] as int? ?? 0,
          lost: json['lost'] as int? ?? 0,
          quarantined: json['quarantined'] as int? ?? 0,
          budgetStops: json['budget_stops'] as int? ?? 0,
          greenInkRejected: json['green_ink_rejected'] as int? ?? 0,
        );

  /// `12 done · 1 failed · 2 stalled`, leaving out what never happened.
  String get label {
    final parts = [
      '$done done',
      if (failed > 0) '$failed failed',
      if (blown > 0) '$blown blown',
      if (abandoned > 0) '$abandoned abandoned',
      if (stalled > 0) '$stalled stalled',
      if (lost > 0) '$lost lost',

      if (quarantined > 0) '$quarantined quarantined',
      if (budgetStops > 0) '$budgetStops budget stops',
      if (greenInkRejected > 0) '$greenInkRejected green ink refused',
    ];
    return parts.join(' · ');
  }
}

/// Where an Agent stands against its configured retirement date.
@immutable
class RetirementSummary {
  const RetirementSummary({
    required this.retireOn,
    required this.onRetire,
    required this.status,
  });

  /// The UTC date, `YYYY-MM-DD`.
  final String retireOn;

  /// `desk` or `retired`.
  final String onRetire;

  /// `retiring` (inside the warning window), `desk_duty` or `retired`.
  final String status;

  factory RetirementSummary.fromJson(Map<String, dynamic> json) =>
      RetirementSummary(
        retireOn: json['retire_on'] as String,
        onRetire: json['on_retire'] as String,
        status: json['status'] as String,
      );

  String get label => switch (status) {
    'retiring' =>
      onRetire == 'retired'
          ? 'Retires on $retireOn'
          : 'Desk duty from $retireOn',
    'desk_duty' => 'Desk duty since $retireOn',
    _ => 'Retired on $retireOn',
  };
}

/// An Agent's clearance in one Operation, set apart from its configured one.
@immutable
class ClearanceGrantSummary {
  const ClearanceGrantSummary({
    required this.clearance,
    required this.signer,
    this.approval,
    this.reason,
    this.expiresAt,
  });

  final String clearance;
  final String signer;

  /// The green ink request that raised it; null for a grant that only lowers.
  final String? approval;
  final String? reason;

  /// Unix seconds.
  final double? expiresAt;

  factory ClearanceGrantSummary.fromJson(Map<String, dynamic> json) =>
      ClearanceGrantSummary(
        clearance: json['clearance'] as String,
        signer: json['signer'] as String,
        approval: json['approval'] as String?,
        reason: json['reason'] as String?,
        expiresAt: (json['expires_at'] as num?)?.toDouble(),
      );

  bool get raises => approval != null;

  String get label => raises
      ? 'Raised to $clearance by $signer'
      : 'Held at $clearance by $signer${reason == null ? '' : ': $reason'}';
}

const _defaultModel = "The handler's default model";

/// One way an Agent's roster record says it reaches its model.
@immutable
class AgentRouteSummary {
  const AgentRouteSummary({this.handler, this.model, this.pool});

  final String? handler;

  /// Null when the record sets none, so the handler's default.
  final String? model;

  /// The account pool the route names, when it names one.
  final String? pool;

  factory AgentRouteSummary.fromJson(Map<String, dynamic> json) =>
      AgentRouteSummary(
        handler: json['handler'] as String?,
        model: json['model'] as String?,
        pool: json['pool'] as String?,
      );
}

/// A model that ran in the codename in this Operation.
@immutable
class ModelTenureSummary {
  const ModelTenureSummary({required this.from, this.model, this.to});

  /// Null for the handler's default.
  final String? model;

  /// Unix seconds of its first counted outcome.
  final double from;

  /// Unix seconds when another model's outcome was first counted.
  final double? to;

  factory ModelTenureSummary.fromJson(Map<String, dynamic> json) =>
      ModelTenureSummary(
        model: json['model'] as String?,
        from: (json['from'] as num).toDouble(),
        to: (json['to'] as num?)?.toDouble(),
      );
}

/// One Agent of an Operation and its record there.
@immutable
class AgentSummary {
  const AgentSummary({
    required this.agent,
    required this.posted,
    required this.operation,
    this.everyOperation = const AgentTallySummary(),
    this.lastMissionAt,
    this.routes = const [],
    this.modelHistory = const [],
    this.clearance,
    this.configuredClearance,
    this.retirement,
    this.grant,
    this.promotion,
    this.grantUnreadable = false,
  });

  final String agent;

  /// Whether the Headquarters Station's roster posts it to the Operation.
  final bool posted;
  final AgentTallySummary operation;

  /// Quarantines of every Operation, and work outside any.
  final AgentTallySummary everyOperation;

  /// Unix seconds.
  final double? lastMissionAt;

  /// How its roster record says it reaches its model; empty for an Agent no longer posted and
  /// for a person who may not read the roster.
  final List<AgentRouteSummary> routes;

  /// The models that ran in the codename here, oldest first; a model with no finished Mission
  /// is not among them.
  final List<ModelTenureSummary> modelHistory;

  /// The clearance it works at in this Operation.
  final String? clearance;
  final String? configuredClearance;
  final RetirementSummary? retirement;
  final ClearanceGrantSummary? grant;

  /// The clearance a waiting promotion request asks for.
  final String? promotion;

  /// Its grant could not be read; Stations work it at `desk`.
  final bool grantUnreadable;

  factory AgentSummary.fromJson(Map<String, dynamic> json) {
    final promotion = json['promotion'] as Map<String, dynamic>?;
    final target =
        (promotion?['scope'] as Map<String, dynamic>?)?['target'] as String?;
    return AgentSummary(
      agent: json['agent'] as String,
      posted: json['posted'] as bool? ?? false,
      operation: AgentTallySummary.fromJson(
        json['operation'] as Map<String, dynamic>?,
      ),
      everyOperation: AgentTallySummary.fromJson(
        json['every_operation'] as Map<String, dynamic>?,
      ),
      lastMissionAt: (json['last_mission_at'] as num?)?.toDouble(),
      routes: [
        for (final route in json['routes'] as List<dynamic>? ?? const [])
          AgentRouteSummary.fromJson(route as Map<String, dynamic>),
      ],
      modelHistory: [
        for (final tenure
            in json['model_history'] as List<dynamic>? ?? const [])
          ModelTenureSummary.fromJson(tenure as Map<String, dynamic>),
      ],
      clearance: json['clearance'] as String?,
      configuredClearance: json['configured_clearance'] as String?,
      retirement: switch (json['retirement']) {
        final Map<String, dynamic> retirement => RetirementSummary.fromJson(
          retirement,
        ),
        _ => null,
      },
      grant: switch (json['grant']) {
        final Map<String, dynamic> grant => ClearanceGrantSummary.fromJson(
          grant,
        ),
        _ => null,
      },
      grantUnreadable: json['grant_unreadable'] as bool? ?? false,
      // The target names `AGENT:CLEARANCE@OPERATION`.
      promotion: target == null
          ? null
          : RegExp(r':([^@]+)@').firstMatch(target)?.group(1),
    );
  }

  /// What the card says of its model: each route its record gives, the date only from a tenure
  /// of that same model. With no record to read, what ran last, and nothing when nothing did:
  /// an empty history does not mean the handler's default.
  List<String> get modelLabels {
    String name(String? model) => model == null ? '' : 'Model $model';
    String since(String? model) {
      final last = modelHistory.lastOrNull;
      return last == null || last.model != model
          ? ''
          : ' since ${_date(last.from)}';
    }

    if (routes.isEmpty) {
      final last = modelHistory.lastOrNull;
      if (last == null) return const [];
      return [
        '${last.model == null ? _defaultModel : name(last.model)}'
            '${since(last.model)}',
      ];
    }
    return [
      for (final route in routes)
        [
          '${route.model == null ? _defaultModel : name(route.model)}'
              '${since(route.model)}',
          if (route.handler case final handler?) 'handler $handler',
          if (route.pool case final pool?) 'pool $pool',
        ].join(', '),
    ];
  }

  /// The models that ran here before the one it has now, oldest first.
  List<String> get earlierModels {
    final now = routes.map((route) => route.model).toSet();
    final last = modelHistory.lastOrNull;
    return [
      for (final tenure in modelHistory)
        if (!identical(tenure, last) ||
            (routes.isNotEmpty && !now.contains(tenure.model)))
          [
            tenure.model ?? _defaultModel,
            if (tenure.to case final to?)
              '${_date(tenure.from)} to ${_date(to)}'
            else
              'from ${_date(tenure.from)}',
          ].join(' '),
    ];
  }

  /// The clearances it may be lowered to: those below the one it works at.
  List<String> get lowerClearances {
    final index = clearances.indexOf(clearance ?? '');
    return index <= 0 ? const [] : clearances.sublist(0, index);
  }
}

String _date(double unix) => DateTime.fromMillisecondsSinceEpoch(
  (unix * 1000).round(),
  isUtc: true,
).toIso8601String().substring(0, 10);

/// The Agents of an Operation: their record there, clearance, retirement and promotion.
///
/// The [route] (page `agents`) names the Operation (`operation_id`, else the first the person
/// may read) and may narrow the list to one `agent`.
class AgentsPage extends StatefulWidget {
  const AgentsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<AgentsPage> createState() => _AgentsPageState();
}

class _AgentsPageState extends State<AgentsPage>
    with RecordActions<AgentsPage> {
  @override
  MissionBoard get recordBoard => widget.board;

  @override
  String get recordKind => 'agents';

  @override
  Future<void> recordsChanged() => _load();

  List<AgentSummary> _agents = const [];
  bool _loading = false;
  bool _changing = false;
  String? _error;

  List<String> get _operations => widget.board.agentOperations;

  /// The Operation shown: the route's when the person may read it, else none.
  String? get _operation {
    final operations = _operations;
    final named = widget.route['operation_id'];
    if (named != null) return operations.contains(named) ? named : null;
    return operations.isEmpty ? null : operations.first;
  }

  @override
  void initState() {
    super.initState();
    unawaited(_load());
    unawaited(checkRecordsTakeEdits());
  }

  @override
  void didUpdateWidget(AgentsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      setState(() {
        _agents = const [];
        _error = null;
      });
      unawaited(_load());
    }
  }

  Future<void> _load() async {
    final operation = _operation;
    if (operation == null) return;
    setState(() => _loading = true);
    try {
      final agents = await widget.board.loadAgents(operation);
      if (!mounted || operation != _operation) return;
      setState(() {
        _agents = agents;
        _error = null;
      });
    } on StateError catch (error) {
      if (mounted) setState(() => _error = error.message);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _lower(AgentSummary agent) async {
    final operation = _operation;
    if (operation == null) return;
    final lowered = await showDialog<(String, String)>(
      context: context,
      builder: (_) => _LowerClearanceDialog(
        agent: agent,
        reasonChars: widget.board.limits.inputs.reasonChars,
      ),
    );
    if (lowered == null || !mounted) return;
    await _change(
      () => widget.board.lowerClearance(
        operation,
        agent.agent,
        lowered.$1,
        lowered.$2,
      ),
      '${agent.agent} now works at ${lowered.$1} in $operation.',
    );
  }

  Future<void> _restore(AgentSummary agent) async {
    final operation = _operation;
    if (operation == null) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Restore ${agent.agent}\'s configured clearance?'),
        content: Text(
          'Removes the grant in $operation; ${agent.agent} works at '
          '${agent.configuredClearance ?? 'its configured clearance'} again. '
          'A raise it had earned must be signed again.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Restore'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    await _change(
      () => widget.board.revokeClearanceGrant(operation, agent.agent),
      '${agent.agent} is back at its configured clearance.',
    );
  }

  Future<void> _change(Future<void> Function() change, String done) async {
    setState(() => _changing = true);
    try {
      await change();
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(done)));
      await _load();
    } on StateError catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.message)));
      }
    } finally {
      if (mounted) setState(() => _changing = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = _operations;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operation\'s Agents are visible to this account.'),
      );
    }
    final operation = _operation;
    final named = widget.route['agent'];
    final agents = [
      for (final agent in _agents)
        if (named == null || agent.agent == named) agent,
    ];
    final canConfigure =
        operation != null &&
        !widget.board.demo &&
        widget.board.canConfigureAgentRoles(operation);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 16, 24, 8),
          child: Wrap(
            spacing: 16,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              DropdownButton<String>(
                value: operation,
                hint: const Text('Operation'),
                items: [
                  for (final operation in operations)
                    DropdownMenuItem(value: operation, child: Text(operation)),
                ],
                onChanged: (chosen) {
                  if (chosen == null || chosen == operation) return;
                  widget.onRoute(
                    widget.route
                        .without('agent')
                        .withField('operation_id', chosen),
                  );
                },
              ),
              IconButton(
                tooltip: 'Refresh Agents',
                onPressed: () => unawaited(_load()),
                icon: const Icon(Icons.refresh),
              ),
              if (canChangeRecords)
                FilledButton.icon(
                  onPressed: () => unawaited(editRecord()),
                  icon: const Icon(Icons.add),
                  label: const Text('Add Agent'),
                ),
            ],
          ),
        ),
        if (recordReachNote() case final note?)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 8),
            child: note,
          ),
        OoFilterBar(route: widget.route, onRoute: widget.onRoute),
        if (_error case final error? when agents.isNotEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 24),
            child: Text(error),
          ),
        Expanded(
          child: operation == null
              ? Center(
                  child: Text(
                    'Operation ${widget.route['operation_id']}\'s Agents are '
                    'not visible to this account.',
                  ),
                )
              : agents.isEmpty && _error != null
              ? OoEmptyState(
                  icon: OoIconName.agents,
                  title: _error!,
                  body: 'Nothing is shown because it could not be read.',
                  action: ('Try again', () => unawaited(_load())),
                )
              : agents.isEmpty
              ? Center(
                  child: _loading
                      ? const CircularProgressIndicator()
                      : Text(
                          named == null
                              ? 'No Agent is posted to this Operation. An Agent is a '
                                    'codename with a handler and model; the roster names them.'
                              : 'Agent $named is not in Operation $operation.',
                        ),
                )
              : ListView(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 24),
                  children: [
                    if (widget.board.canReadScorecard(operation) &&
                        named == null)
                      ScorecardSection(
                        board: widget.board,
                        operation: operation,
                      ),
                    for (final agent in agents)
                      _AgentTile(
                        agent: agent,
                        onEdit: canChangeRecords
                            ? () => unawaited(editRecord(agent.agent))
                            : null,
                        onRemove: canChangeRecords
                            ? () => unawaited(removeRecord(agent.agent))
                            : null,
                        onLower:
                            canConfigure &&
                                !_changing &&
                                agent.lowerClearances.isNotEmpty
                            ? () => unawaited(_lower(agent))
                            : null,
                        onRestore:
                            canConfigure && !_changing && agent.grant != null
                            ? () => unawaited(_restore(agent))
                            : null,
                      ),
                  ],
                ),
        ),
      ],
    );
  }
}

class _AgentTile extends StatelessWidget {
  const _AgentTile({
    required this.agent,
    this.onLower,
    this.onRestore,
    this.onEdit,
    this.onRemove,
  });

  final AgentSummary agent;

  /// Opens its record's form, and removes the record; `null` without `roster.manage`.
  final VoidCallback? onEdit;
  final VoidCallback? onRemove;
  final VoidCallback? onLower;
  final VoidCallback? onRestore;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final quarantinedEverywhere = agent.everyOperation.quarantined;
    final clearance = agent.clearance;
    final configured = agent.configuredClearance;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  agent.retirement?.status == 'retired'
                      ? Icons.person_off_outlined
                      : Icons.badge_outlined,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Align(
                    alignment: Alignment.centerLeft,
                    child: OoLink(
                      agent.agent,
                      route: OoLinkTo.missions().withField(
                        'agent',
                        agent.agent,
                      ),
                      semanticLabel:
                          'Open the Missions of Agent ${agent.agent}',
                      style: theme.textTheme.titleMedium,
                    ),
                  ),
                ),
                if (clearance != null)
                  Chip(
                    label: Text(
                      configured != null && configured != clearance
                          ? '$clearance (configured $configured)'
                          : clearance,
                    ),
                  ),
                if (onEdit != null)
                  IconButton(
                    tooltip: 'Edit ${agent.agent}',
                    onPressed: onEdit,
                    icon: const Icon(Icons.edit_outlined),
                  ),
                if (onRemove != null)
                  IconButton(
                    tooltip: 'Remove ${agent.agent}',
                    onPressed: onRemove,
                    icon: const Icon(Icons.delete_outline),
                  ),
              ],
            ),
            const SizedBox(height: 8),
            Text(agent.operation.label),
            if (quarantinedEverywhere > 0)
              Text('$quarantinedEverywhere quarantined in every Operation'),
            Text(
              [
                ...agent.modelLabels,
                if (agent.lastMissionAt case final at?)
                  'last Mission ${_date(at)}',
                if (!agent.posted) 'not posted',
              ].join(' · '),
              style: theme.textTheme.bodySmall,
            ),
            if (agent.earlierModels case final earlier when earlier.isNotEmpty)
              Text(
                'Earlier: ${earlier.join(', ')}',
                style: theme.textTheme.bodySmall,
              ),
            if (agent.retirement case final retirement?)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Row(
                  children: [
                    Icon(Icons.event_busy, color: theme.colorScheme.error),
                    const SizedBox(width: 6),
                    Text(retirement.label),
                  ],
                ),
              ),
            if (agent.grantUnreadable)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  'Clearance grant unreadable: Stations work it at desk',
                  style: TextStyle(color: theme.colorScheme.error),
                ),
              ),
            if (agent.grant case final grant?)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(grant.label),
              ),
            if (agent.promotion case final promotion?)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text('Promotion to $promotion awaits green ink'),
              ),
            if (onLower != null || onRestore != null)
              OverflowBar(
                alignment: MainAxisAlignment.end,
                children: [
                  if (onRestore != null)
                    TextButton(
                      onPressed: onRestore,
                      child: const Text('Restore configured clearance'),
                    ),
                  if (onLower != null)
                    TextButton(
                      onPressed: onLower,
                      child: const Text('Lower clearance'),
                    ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

class _LowerClearanceDialog extends StatefulWidget {
  const _LowerClearanceDialog({required this.agent, required this.reasonChars});

  final AgentSummary agent;

  /// The most characters the reason may hold; `0` for no bound.
  final int reasonChars;

  @override
  State<_LowerClearanceDialog> createState() => _LowerClearanceDialogState();
}

class _LowerClearanceDialogState extends State<_LowerClearanceDialog> {
  late String _clearance = widget.agent.lowerClearances.first;
  final _reason = TextEditingController();

  @override
  void dispose() {
    _reason.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text('Lower ${widget.agent.agent}\'s clearance'),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        DropdownButton<String>(
          value: _clearance,
          items: [
            for (final clearance in widget.agent.lowerClearances)
              DropdownMenuItem(value: clearance, child: Text(clearance)),
          ],
          onChanged: (clearance) {
            if (clearance != null) setState(() => _clearance = clearance);
          },
        ),
        TextField(
          controller: _reason,
          decoration: const InputDecoration(labelText: 'Reason'),
          maxLength: InputSettings.field(widget.reasonChars),
        ),
      ],
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: () {
          final reason = _reason.text.trim();
          if (reason.isEmpty) return;
          Navigator.of(context).pop((_clearance, reason));
        },
        child: const Text('Lower'),
      ),
    ],
  );
}
