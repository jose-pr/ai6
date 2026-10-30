import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../shell/oo_freshness.dart';

/// The time in UTC, to the second.
String utcTime(double at) => DateTime.fromMillisecondsSinceEpoch(
  (at * 1000).round(),
  isUtc: true,
).toIso8601String().replaceFirst(RegExp(r'\.\d+Z$'), 'Z');

/// An Agent's request to take one irreversible step, waiting for green ink.
@immutable
class GreenInkRequestSummary {
  const GreenInkRequestSummary({
    required this.id,
    required this.operationId,
    required this.missionId,
    required this.sessionId,
    required this.agent,
    required this.kind,
    required this.target,
    required this.reason,
    required this.requestedAt,
    required this.expiresAt,
  });

  final String id;
  final String operationId;
  final String missionId;
  final String sessionId;
  final String agent;

  /// `push`, `merge`, `release`, `delete`, `clearance` or a kind the Operation adds.
  final String kind;
  final String target;

  /// The Agent's own words: untrusted text.
  final String reason;
  final double requestedAt;

  /// When the request expires as refused.
  final double expiresAt;

  factory GreenInkRequestSummary.fromJson(Map<String, dynamic> json) {
    final scope = json['scope'] as Map<String, dynamic>;
    return GreenInkRequestSummary(
      id: json['id'] as String,
      operationId: json['operation'] as String,
      missionId: json['mission'] as String,
      sessionId: json['session'] as String,
      agent: json['agent'] as String,
      kind: scope['kind'] as String,
      target: scope['target'] as String,
      reason: json['reason'] as String,
      requestedAt: (json['requested_at'] as num).toDouble(),
      expiresAt: (json['expires_at'] as num).toDouble(),
    );
  }
}

/// A standing approval of a kind of step.
@immutable
class ClassAuthorizationSummary {
  const ClassAuthorizationSummary({
    required this.id,
    required this.kinds,
    required this.targetPattern,
    required this.agents,
    required this.roles,
    required this.clearanceMin,
    required this.uses,
    required this.expiresAt,
    required this.signer,
    this.maxUses,
    this.revokedBy,
  });

  final String id;
  final List<String> kinds;
  final String targetPattern;
  final List<String> agents;
  final List<String> roles;
  final String clearanceMin;
  final int? maxUses;
  final int uses;
  final double expiresAt;
  final String signer;
  final String? revokedBy;

  factory ClassAuthorizationSummary.fromJson(Map<String, dynamic> json) =>
      ClassAuthorizationSummary(
        id: json['id'] as String,
        kinds: [for (final kind in json['kinds'] as List<dynamic>) '$kind'],
        targetPattern: json['target_pattern'] as String,
        agents: [
          for (final agent in json['agents'] as List<dynamic>? ?? const [])
            '$agent',
        ],
        roles: [
          for (final role in json['roles'] as List<dynamic>? ?? const [])
            '$role',
        ],
        clearanceMin: json['clearance_min'] as String,
        maxUses: json['max_uses'] as int?,
        uses: json['uses'] as int? ?? 0,
        expiresAt: (json['expires_at'] as num).toDouble(),
        signer: json['signer'] as String,
        revokedBy: (json['revoked'] as Map<String, dynamic>?)?['by'] as String?,
      );

  /// `revoked`, `expired`, `used up` or `live` at [now] (Unix seconds).
  String status(double now) {
    if (revokedBy != null) return 'revoked';
    if (expiresAt <= now) return 'expired';
    if (maxUses case final max? when uses >= max) return 'used up';
    return 'live';
  }
}

/// How long an approval lasts, as the signer may choose it.
const approvalLifetimes = <(String, int)>[
  ('15 minutes', 15 * 60),
  ('1 hour', 60 * 60),
  ('4 hours', 4 * 60 * 60),
  ('24 hours', 24 * 60 * 60),
];

double _now() => DateTime.now().millisecondsSinceEpoch / 1000;

/// The Operations a page may show, and the one shown.
class _OperationPicker extends StatelessWidget {
  const _OperationPicker({
    required this.operations,
    required this.selected,
    required this.onChanged,
  });

  final List<String> operations;
  final String? selected;
  final ValueChanged<String?> onChanged;

  @override
  Widget build(BuildContext context) => ConstrainedBox(
    constraints: const BoxConstraints(maxWidth: 420),
    child: DropdownButtonFormField<String>(
      // The field keeps its first value; a new selection is a new field.
      key: ValueKey(selected),
      initialValue: selected,
      decoration: const InputDecoration(labelText: 'Operation'),
      items: [
        for (final id in operations)
          DropdownMenuItem(value: id, child: Text(id)),
      ],
      onChanged: onChanged,
    ),
  );
}

/// The Operation a green ink page shows: [route]'s `operation_id` when it is one of
/// [operations], `null` when it is not, else the first of them.
String? _routeOperation(OoRoute route, List<String> operations) {
  final named = route['operation_id'];
  if (named != null) return operations.contains(named) ? named : null;
  return operations.isEmpty ? null : operations.first;
}

/// The Green ink inbox: steps Agents wait to take, each approved for a chosen time or
/// rejected with a reason. Needs `approval.decide`.
///
/// The [route] (page `green_ink`) names the Operation and may narrow the inbox to one
/// `request_id`.
class GreenInkPage extends StatefulWidget {
  const GreenInkPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<GreenInkPage> createState() => _GreenInkPageState();
}

class _GreenInkPageState extends State<GreenInkPage> {
  Future<List<GreenInkRequestSummary>>? _requests;
  bool _deciding = false;

  String? get _operationId =>
      _routeOperation(widget.route, widget.board.greenInkOperations);

  @override
  void initState() {
    super.initState();
    _reload();
  }

  @override
  void didUpdateWidget(GreenInkPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      _reload();
    }
  }

  void _reload() {
    final operationId = _operationId;
    setState(() {
      _requests = operationId == null
          ? null
          : widget.board.loadGreenInk(operationId);
    });
  }

  void _select(String? operationId) {
    if (operationId == null || operationId == _operationId) return;
    widget.onRoute(
      widget.route.without('request_id').withField('operation_id', operationId),
    );
  }

  Future<void> _ask(
    GreenInkRequestSummary request, {
    required bool approve,
  }) async {
    final decision = await showDialog<_Decision>(
      context: context,
      builder: (context) => _DecisionDialog(request: request, approve: approve),
    );
    if (decision == null || !mounted) return;
    await _decide(
      request,
      approve: approve,
      expiresInSecs: approve ? decision.lifetime : null,
      note: decision.note,
    );
  }

  Future<void> _decide(
    GreenInkRequestSummary request, {
    required bool approve,
    int? expiresInSecs,
    String? note,
  }) async {
    setState(() => _deciding = true);
    try {
      await widget.board.decideGreenInk(
        operationId: request.operationId,
        requestId: request.id,
        approve: approve,
        expiresInSecs: expiresInSecs,
        note: note,
      );
      if (!mounted) return;
      _reload();
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(approve ? 'Signed in green ink.' : 'Step rejected.'),
        ),
      );
    } on Object catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.toString())));
      }
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = widget.board.greenInkOperations;
    final operationId = _operationId;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operations grant approval.decide to this account.'),
      );
    }
    final picker = _OperationPicker(
      operations: operations,
      selected: operationId,
      onChanged: _select,
    );
    final filters = OoFilterBar(
      route: widget.route,
      onRoute: widget.onRoute,
      padding: const EdgeInsets.only(top: 8),
    );
    if (operationId == null) {
      return Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            picker,
            filters,
            Expanded(
              child: Center(
                child: Text(
                  'Operation ${widget.route['operation_id']} does not grant '
                  'approval.decide to this account.',
                ),
              ),
            ),
          ],
        ),
      );
    }
    final requestId = widget.route['request_id'];
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          picker,
          filters,
          const SizedBox(height: 16),
          Row(
            children: [
              Expanded(
                child: Text(
                  'Irreversible steps wait here for a person to sign.',
                  style: Theme.of(context).textTheme.titleMedium,
                ),
              ),
              IconButton(
                tooltip: 'Refresh green ink',
                onPressed: _deciding ? null : _reload,
                icon: const Icon(Icons.refresh),
              ),
            ],
          ),
          if (widget.board.demo)
            const Chip(label: Text('Preview data — decisions disabled')),
          const SizedBox(height: 8),
          Expanded(
            child: FutureBuilder<List<GreenInkRequestSummary>>(
              future: _requests,
              builder: (context, snapshot) {
                if (snapshot.hasError) {
                  return Center(child: Text(snapshot.error.toString()));
                }
                if (!snapshot.hasData) {
                  return const Center(child: CircularProgressIndicator());
                }
                final requests = [
                  for (final request in snapshot.data!)
                    if (requestId == null || request.id == requestId) request,
                ];
                if (requests.isEmpty) {
                  return Center(
                    child: Text(
                      requestId == null
                          ? 'No steps are waiting for green ink.'
                          : 'Request $requestId is not waiting in Operation '
                                '$operationId.',
                    ),
                  );
                }
                return ListView.separated(
                  itemCount: requests.length,
                  separatorBuilder: (context, index) =>
                      const SizedBox(height: 8),
                  itemBuilder: (context, index) {
                    final request = requests[index];
                    return Card(
                      child: Padding(
                        padding: const EdgeInsets.all(16),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              '${request.kind} · ${request.target}',
                              style: Theme.of(context).textTheme.titleMedium,
                            ),
                            const SizedBox(height: 4),
                            Wrap(
                              crossAxisAlignment: WrapCrossAlignment.center,
                              children: [
                                Text(
                                  'Agent ${request.agent} · '
                                  'Mission ${request.missionId} · ',
                                ),
                                OoTime(
                                  request.expiresAt,
                                  prefix: 'expires ',
                                  style: DefaultTextStyle.of(context).style,
                                ),
                              ],
                            ),
                            const SizedBox(height: 8),
                            Text(request.reason),
                            const SizedBox(height: 8),
                            Wrap(
                              spacing: 8,
                              children: [
                                FilledButton(
                                  onPressed: _deciding || widget.board.demo
                                      ? null
                                      : () => _ask(request, approve: true),
                                  child: const Text('Approve…'),
                                ),
                                OutlinedButton(
                                  onPressed: _deciding || widget.board.demo
                                      ? null
                                      : () => _ask(request, approve: false),
                                  child: const Text('Reject…'),
                                ),
                              ],
                            ),
                          ],
                        ),
                      ),
                    );
                  },
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

/// What the signer chose: how long an approval lasts, and what to tell the Agent.
typedef _Decision = ({int lifetime, String? note});

/// Asks the signer to confirm: an approval with its lifetime and an optional note, or a
/// refusal with an optional reason.
class _DecisionDialog extends StatefulWidget {
  const _DecisionDialog({required this.request, required this.approve});

  final GreenInkRequestSummary request;
  final bool approve;

  @override
  State<_DecisionDialog> createState() => _DecisionDialogState();
}

class _DecisionDialogState extends State<_DecisionDialog> {
  final _note = TextEditingController();
  int _lifetime = approvalLifetimes[1].$2;

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final request = widget.request;
    final approve = widget.approve;
    return AlertDialog(
      title: Text(approve ? 'Sign in green ink?' : 'Reject this step?'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${approve ? 'Let' : 'Refuse'} ${request.agent} '
            '${request.kind} ${request.target} in Mission ${request.missionId}.',
          ),
          const SizedBox(height: 12),
          if (approve)
            DropdownButtonFormField<int>(
              initialValue: _lifetime,
              decoration: const InputDecoration(labelText: 'Approval lasts'),
              items: [
                for (final (label, seconds) in approvalLifetimes)
                  DropdownMenuItem(value: seconds, child: Text(label)),
              ],
              onChanged: (seconds) =>
                  setState(() => _lifetime = seconds ?? _lifetime),
            ),
          TextField(
            controller: _note,
            decoration: InputDecoration(
              labelText: approve
                  ? 'Note for the Agent (optional)'
                  : 'Reason for the Agent (optional)',
            ),
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
            final note = _note.text.trim();
            Navigator.of(context).pop<_Decision>((
              lifetime: _lifetime,
              note: note.isEmpty ? null : note,
            ));
          },
          child: Text(approve ? 'Approve' : 'Reject'),
        ),
      ],
    );
  }
}

/// An Operation's class authorisations, with a way to revoke the live ones. Needs
/// `approval.class`.
///
/// The [route] (page `green_ink`) names the Operation.
class ClassAuthorizationsPage extends StatefulWidget {
  const ClassAuthorizationsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<ClassAuthorizationsPage> createState() =>
      _ClassAuthorizationsPageState();
}

class _ClassAuthorizationsPageState extends State<ClassAuthorizationsPage> {
  Future<List<ClassAuthorizationSummary>>? _records;
  bool _revoking = false;

  String? get _operationId =>
      _routeOperation(widget.route, widget.board.classAuthorizationOperations);

  @override
  void initState() {
    super.initState();
    _reload();
  }

  @override
  void didUpdateWidget(ClassAuthorizationsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      _reload();
    }
  }

  void _reload() {
    final operationId = _operationId;
    setState(() {
      _records = operationId == null
          ? null
          : widget.board.loadClassAuthorizations(operationId);
    });
  }

  void _select(String? operationId) {
    if (operationId == null || operationId == _operationId) return;
    widget.onRoute(
      widget.route.without('request_id').withField('operation_id', operationId),
    );
  }

  Future<void> _revoke(
    String operationId,
    ClassAuthorizationSummary record,
  ) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Revoke class authorisation?'),
        content: Text(
          'Requests to ${record.kinds.join(', ')} ${record.targetPattern} will '
          'wait for green ink again.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Revoke'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    setState(() => _revoking = true);
    try {
      await widget.board.revokeClassAuthorization(
        operationId: operationId,
        id: record.id,
      );
      if (mounted) _reload();
    } on Object catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.toString())));
      }
    } finally {
      if (mounted) setState(() => _revoking = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = widget.board.classAuthorizationOperations;
    final operationId = _operationId;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operations grant approval.class to this account.'),
      );
    }
    final picker = _OperationPicker(
      operations: operations,
      selected: operationId,
      onChanged: _select,
    );
    final filters = OoFilterBar(
      route: widget.route,
      onRoute: widget.onRoute,
      padding: const EdgeInsets.only(top: 8),
    );
    if (operationId == null) {
      return Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            picker,
            filters,
            Expanded(
              child: Center(
                child: Text(
                  'Operation ${widget.route['operation_id']} does not grant '
                  'approval.class to this account.',
                ),
              ),
            ),
          ],
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          picker,
          filters,
          const SizedBox(height: 16),
          Text(
            'Standing approvals: a matching request is signed without asking.',
            style: Theme.of(context).textTheme.titleMedium,
          ),
          const SizedBox(height: 8),
          Expanded(
            child: FutureBuilder<List<ClassAuthorizationSummary>>(
              future: _records,
              builder: (context, snapshot) {
                if (snapshot.hasError) {
                  return Center(child: Text(snapshot.error.toString()));
                }
                if (!snapshot.hasData) {
                  return const Center(child: CircularProgressIndicator());
                }
                final records = snapshot.data!;
                if (records.isEmpty) {
                  return const Center(
                    child: Text('No class authorisations in this Operation.'),
                  );
                }
                final now = _now();
                return ListView(
                  children: [
                    for (final record in records)
                      ListTile(
                        title: Text(
                          '${record.kinds.join(', ')} · ${record.targetPattern}',
                        ),
                        subtitle: Text(
                          [
                            if (record.agents.isNotEmpty)
                              'Agents ${record.agents.join(', ')}',
                            if (record.roles.isNotEmpty)
                              'roles ${record.roles.join(', ')}',
                            '${record.clearanceMin} and above',
                            record.maxUses == null
                                ? '${record.uses} uses'
                                : '${record.uses}/${record.maxUses} uses',
                            'expires ${utcTime(record.expiresAt)}',
                            'signed by ${record.signer}',
                          ].join(' · '),
                        ),
                        trailing: record.status(now) == 'live'
                            ? TextButton(
                                onPressed: _revoking || widget.board.demo
                                    ? null
                                    : () => _revoke(operationId, record),
                                child: const Text('Revoke'),
                              )
                            : Chip(
                                label: Text(record.status(now).toUpperCase()),
                              ),
                      ),
                  ],
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}
