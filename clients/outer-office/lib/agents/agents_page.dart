import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../widgets/oo_data_table.dart';
import '../roster/record_editor.dart';
import '../routes/oo_route.dart';
import '../routes/oo_scope.dart';
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

  /// Runs lost with their Station or runner: not the Agent's failures.
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

/// A hold that keeps an Agent, or one of its routes, out of use until a person lifts it.
@immutable
class HoldSummary {
  const HoldSummary({
    required this.by,
    required this.reason,
    this.handler,
    this.model,
    this.pool,
  });

  /// A person, or `m` when M held it by itself.
  final String by;
  final String reason;

  /// The route held; null holds the whole Agent.
  final String? handler;
  final String? model;
  final String? pool;

  factory HoldSummary.fromJson(Map<String, dynamic> json) {
    final route = json['route'] as Map<String, dynamic>?;
    return HoldSummary(
      by: json['by'] as String,
      reason: json['reason'] as String,
      handler: route?['handler'] as String?,
      model: route?['model'] as String?,
      pool: route?['pool'] as String?,
    );
  }

  bool get wholeAgent => handler == null;

  /// `Held by m: reason`, naming the route when it is one.
  String get label => [
    wholeAgent
        ? 'Held by $by'
        : 'Route ${[handler, model ?? 'default model', if (pool case final pool?) 'pool $pool'].join(' · ')} held by $by',
    reason,
  ].join(': ');
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
    this.holds = const [],
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

  /// What keeps it, or a route of it, out of use; a route hold is left out for a person
  /// without `roster.read`.
  final List<HoldSummary> holds;

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
      holds: [
        for (final hold in json['holds'] as List<dynamic>? ?? const [])
          HoldSummary.fromJson(hold as Map<String, dynamic>),
      ],
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

  /// The Agents shown, each with the Operation it was read in.
  List<(String, AgentSummary)> _agents = const [];

  /// The Operations whose Agents could not be read, with why.
  Map<String, String> _failed = const {};
  bool _loading = false;
  bool _changing = false;

  OoScope get _scope => OoScope(widget.route, widget.board.agentOperations);
  String? get _operation => _scope.operation;
  bool get _unreadable => _scope.unreadable;
  List<String> get _shown => _scope.shown;

  /// The read error to show in place of the list: the one Operation's, when only one is read.
  String? get _error => _shown.length == 1 ? _failed[_shown.single] : null;

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
        _failed = const {};
      });
      unawaited(_load());
    }
  }

  Future<void> _load() async {
    final shown = _shown;
    if (shown.isEmpty) return;
    setState(() => _loading = true);
    final read = await readEach(shown, widget.board.loadAgents, _agents);
    if (!mounted || shown.join('\n') != _shown.join('\n')) return;
    setState(() {
      _agents = read.rows;
      _failed = read.failed;
      _loading = false;
    });
  }

  Future<void> _lower(String operation, AgentSummary agent) async {
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

  Future<void> _restore(String operation, AgentSummary agent) async {
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

  Future<void> _liftHold(
    String operation,
    AgentSummary agent,
    HoldSummary hold,
  ) async {
    await _change(
      () => widget.board.liftHold(operation, agent.agent, hold),
      hold.wholeAgent
          ? '${agent.agent} takes new work again.'
          : 'The route of ${agent.agent} takes new work again.',
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
    final operations = widget.board.agentOperations;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operation\'s Agents are visible to this account.'),
      );
    }
    final operation = _operation;
    final shown = _shown;
    final named = widget.route['agent'];
    final mission = widget.route['mission_id'];
    // The Agents whose Runs ran the Mission, by Operation.
    final ran = {
      for (final run in widget.board.runs)
        if (run.missionId == mission && run.agent != null)
          (run.operationId, run.agent!),
    };
    final agents = [
      for (final row in _agents)
        if ((named == null || row.$2.agent == named) &&
            (mission == null || ran.contains((row.$1, row.$2.agent))))
          row,
    ];
    final shownAgents = {
      for (final (from, agent) in _agents) (from, agent.agent),
    };
    bool canConfigure(String operation) =>
        !widget.board.demo && widget.board.canConfigureAgentRoles(operation);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 16, 24, 8),
          child: Wrap(
            spacing: 16,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              OoOperationPicker(
                scope: _scope,
                onRoute: widget.onRoute,
                narrower: const ['agent'],
              ),
              IconButton(
                tooltip: 'Refresh Agents',
                onPressed: () => unawaited(_load()),
                icon: const Icon(Icons.refresh),
              ),
              OoAddFilter(
                route: widget.route,
                onRoute: widget.onRoute,
                choices: [
                  (
                    field: 'agent',
                    values: [for (final (_, agent) in _agents) agent.agent],
                  ),
                  (
                    field: 'mission_id',
                    values: [
                      for (final run in widget.board.runs)
                        if (run.agent case final agent?
                            when shownAgents.contains((
                              run.operationId,
                              agent,
                            )))
                          run.missionId,
                    ],
                  ),
                ],
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
        OoFilterBar(
          route: withoutTableFields(widget.route, const ['handler']),
          onRoute: widget.onRoute,
        ),
        if (_failed.isNotEmpty && (shown.length > 1 || agents.isNotEmpty))
          OoReadFailures(failed: _failed, several: shown.length > 1),
        Expanded(
          child: _unreadable
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
                      : Text(switch ((named, operation)) {
                          (null, null) =>
                            'No Agent is posted to an Operation this account may '
                                'read. An Agent is a codename with a handler and model; '
                                'the roster names them.',
                          (null, _) =>
                            'No Agent is posted to this Operation. An Agent is a '
                                'codename with a handler and model; the roster names them.',
                          (_, null) =>
                            'Agent $named is not in an Operation this account may read.',
                          (_, _) =>
                            'Agent $named is not in Operation $operation.',
                        }),
                )
              : Column(
                  children: [
                    if (named == null)
                      ScorecardsArea(
                        board: widget.board,
                        operations: [
                          for (final group in shown)
                            if (widget.board.canReadScorecard(group)) group,
                        ],
                      ),
                    Expanded(
                      child: OoDataTable<OoGrouped<AgentSummary>>(
                        rows: groupByEntity(agents, (agent) => agent.agent),
                        rowId: (row) => row.row.agent,
                        noun: 'agents',
                        route: widget.route,
                        onRoute: widget.onRoute,
                        defaultSort: ('agent', true),
                        searchText: (row) => [
                          row.row.agent,
                          ...row.operations,
                          ...row.row.modelLabels,
                          for (final route in row.row.routes) ?route.handler,
                          for (final (_, agent) in row.reads) ...[
                            ?agent.clearance,
                            for (final hold in agent.holds) hold.label,
                          ],
                        ].join(' '),
                        facets: [
                          OoFacet(
                            id: 'handler',
                            label: 'Handler',
                            values: (row) => [
                              for (final route in row.row.routes)
                                ?route.handler,
                            ],
                          ),
                        ],
                        columns: _columns(
                          canConfigure: canConfigure,
                          several: shown.length > 1,
                        ),
                      ),
                    ),
                  ],
                ),
        ),
      ],
    );
  }

  /// One cell per read of the Agent, each named by its Operation when several are shown.
  Widget _perRead(
    OoGrouped<AgentSummary> row,
    bool several,
    Widget Function(String operation, AgentSummary agent) cell,
  ) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      for (final (operation, agent) in row.reads)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 2),
          child: row.reads.length > 1 || several
              ? Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      operation,
                      style: Theme.of(context).textTheme.labelSmall,
                    ),
                    cell(operation, agent),
                  ],
                )
              : cell(operation, agent),
        ),
    ],
  );

  List<OoColumn<OoGrouped<AgentSummary>>> _columns({
    required bool Function(String operation) canConfigure,
    required bool several,
  }) {
    final theme = Theme.of(context);
    String? handler(OoGrouped<AgentSummary> row) =>
        row.row.routes.map((route) => route.handler).nonNulls.firstOrNull;
    return [
      OoColumn(
        id: 'agent',
        label: 'Agent',
        flex: 2,
        sortKey: (row) => row.row.agent,
        cell: (context, row) => Row(
          children: [
            Icon(
              row.row.retirement?.status == 'retired'
                  ? Icons.person_off_outlined
                  : Icons.badge_outlined,
              size: 18,
            ),
            const SizedBox(width: 6),
            Flexible(
              child: OoLink(
                row.row.agent,
                route: OoLinkTo.missions().withField('agent', row.row.agent),
                semanticLabel: 'Open the Missions of Agent ${row.row.agent}',
                style: theme.textTheme.titleSmall,
              ),
            ),
          ],
        ),
      ),
      OoColumn(
        id: 'operations',
        label: 'Operations and clearance',
        flex: 2,
        cell: (context, row) => _perRead(row, several, (operation, agent) {
          final clearance = agent.clearance;
          final configured = agent.configuredClearance;
          return clearance == null
              ? const SizedBox.shrink()
              : Text(
                  configured != null && configured != clearance
                      ? '$clearance (configured $configured)'
                      : clearance,
                );
        }),
      ),
      OoColumn(
        id: 'handler',
        label: 'Handler and model',
        flex: 3,
        sortKey: handler,
        cell: (context, row) => Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              [
                ...row.row.modelLabels,
                if (row.row.lastMissionAt case final at?)
                  'last Mission ${_date(at)}',
                if (!row.row.posted) 'not posted',
              ].join(' · '),
              style: theme.textTheme.bodySmall,
            ),
            if (row.row.earlierModels case final earlier
                when earlier.isNotEmpty)
              Text(
                'Earlier: ${earlier.join(', ')}',
                style: theme.textTheme.bodySmall,
              ),
          ],
        ),
      ),
      OoColumn(
        id: 'done',
        label: 'Record',
        flex: 3,
        sortKey: (row) =>
            row.reads.fold<int>(0, (sum, read) => sum + read.$2.operation.done),
        cell: (context, row) => _perRead(
          row,
          several,
          (operation, agent) => Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(agent.operation.label),
              if (agent.everyOperation.quarantined > 0)
                Text(
                  '${agent.everyOperation.quarantined} quarantined in every Operation',
                ),
            ],
          ),
        ),
      ),
      OoColumn(
        id: 'status',
        label: 'Holds and status',
        flex: 4,
        cell: (context, row) => _perRead(row, several, (operation, agent) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (agent.retirement case final retirement?)
                Row(
                  children: [
                    Icon(Icons.event_busy, color: theme.colorScheme.error),
                    const SizedBox(width: 6),
                    Flexible(child: Text(retirement.label)),
                  ],
                ),
              if (agent.grantUnreadable)
                Text(
                  'Clearance grant unreadable: Stations work it at desk',
                  style: TextStyle(color: theme.colorScheme.error),
                ),
              if (agent.grant case final grant?) Text(grant.label),
              if (agent.promotion case final promotion?)
                Text('Promotion to $promotion awaits green ink'),
              for (final hold in agent.holds)
                Row(
                  children: [
                    Icon(
                      Icons.pause_circle_outline,
                      color: theme.colorScheme.error,
                    ),
                    const SizedBox(width: 6),
                    Flexible(child: Text(hold.label)),
                    if (canConfigure(operation) && !_changing)
                      TextButton(
                        onPressed: () =>
                            unawaited(_liftHold(operation, agent, hold)),
                        child: const Text('Lift hold'),
                      ),
                  ],
                ),
            ],
          );
        }),
      ),
      OoColumn(
        id: 'actions',
        label: 'Actions',
        width: 200,
        cell: (context, row) => Wrap(
          alignment: WrapAlignment.end,
          children: [
            if (canChangeRecords)
              IconButton(
                tooltip: 'Edit ${row.row.agent}',
                onPressed: () => unawaited(editRecord(row.row.agent)),
                icon: const Icon(Icons.edit_outlined),
              ),
            if (canChangeRecords)
              IconButton(
                tooltip: 'Remove ${row.row.agent}',
                onPressed: () => unawaited(removeRecord(row.row.agent)),
                icon: const Icon(Icons.delete_outline),
              ),
            for (final (operation, agent) in row.reads) ...[
              if (canConfigure(operation) && !_changing && agent.grant != null)
                TextButton(
                  onPressed: () => unawaited(_restore(operation, agent)),
                  child: Text(
                    row.reads.length > 1
                        ? 'Restore configured clearance in $operation'
                        : 'Restore configured clearance',
                  ),
                ),
              if (canConfigure(operation) &&
                  !_changing &&
                  agent.lowerClearances.isNotEmpty)
                TextButton(
                  onPressed: () => unawaited(_lower(operation, agent)),
                  child: Text(
                    row.reads.length > 1
                        ? 'Lower clearance in $operation'
                        : 'Lower clearance',
                  ),
                ),
            ],
          ],
        ),
      ),
    ];
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
