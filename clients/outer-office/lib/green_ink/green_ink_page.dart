import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../routes/oo_scope.dart';
import '../shell/oo_freshness.dart';
import '../shell/oo_empty.dart';
import '../shell/oo_link.dart';
import '../theme/oo_icon.dart';
import 'green_ink_reason.dart';

export 'green_ink_reason.dart' show GreenInkLink, SourcePageSummary;

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
    this.links = const [],
  });

  final String id;
  final String operationId;
  final String missionId;
  final String sessionId;
  final String agent;

  /// `push`, `merge`, `release`, `delete`, `clearance` or a kind the Operation adds.
  final String kind;
  final String target;

  /// The Agent's own words: untrusted Markdown.
  final String reason;

  /// What the request is about, as the Agent named it: untrusted too.
  final List<GreenInkLink> links;
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
      links: [
        for (final link in json['links'] as List<dynamic>? ?? const [])
          GreenInkLink.fromJson(link as Map<String, dynamic>),
      ],
      requestedAt: (json['requested_at'] as num).toDouble(),
      expiresAt: (json['expires_at'] as num).toDouble(),
    );
  }
}

/// A request that expired with no decision.
@immutable
class ExpiredGreenInkSummary {
  const ExpiredGreenInkSummary({
    required this.request,
    required this.expiredAt,
  });

  final GreenInkRequestSummary request;
  final double expiredAt;

  factory ExpiredGreenInkSummary.fromJson(Map<String, dynamic> json) =>
      ExpiredGreenInkSummary(
        request: GreenInkRequestSummary.fromJson(
          json['request'] as Map<String, dynamic>,
        ),
        expiredAt: (json['at'] as num).toDouble(),
      );
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

/// What one read of a page's Operations gave: rows with their Operation, and what failed.
typedef _Read<T> = ({List<(String, T)> rows, Map<String, String> failed});

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
  Future<_Read<GreenInkRequestSummary>>? _requests;
  bool _deciding = false;

  OoScope get _scope => OoScope(widget.route, widget.board.greenInkOperations);

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
    final shown = _scope.shown;
    setState(() {
      _requests = shown.isEmpty
          ? null
          : readEach(shown, widget.board.loadGreenInk, const []);
    });
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
    final scope = _scope;
    final operationId = scope.operation;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operations grant approval.decide to this account.'),
      );
    }
    final picker = OoOperationPicker(
      scope: scope,
      onRoute: widget.onRoute,
      narrower: const ['request_id'],
    );
    final filters = OoFilterBar(
      route: widget.route,
      onRoute: widget.onRoute,
      padding: const EdgeInsets.only(top: 8),
    );
    if (scope.unreadable) {
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
            const Chip(label: Text('Demo data: decisions are off')),
          const SizedBox(height: 8),
          Expanded(
            child: FutureBuilder<_Read<GreenInkRequestSummary>>(
              future: _requests,
              builder: (context, snapshot) {
                if (snapshot.hasError) {
                  return Center(child: Text(snapshot.error.toString()));
                }
                if (!snapshot.hasData) {
                  return const Center(child: CircularProgressIndicator());
                }
                final read = snapshot.data!;
                final several = scope.shown.length > 1;
                final agent = widget.route['agent'];
                final mission = widget.route['mission_id'];
                final requests = [
                  for (final (_, request) in read.rows)
                    if ((requestId == null || request.id == requestId) &&
                        (agent == null || request.agent == agent) &&
                        (mission == null || request.missionId == mission))
                      request,
                ];
                final narrow = OoAddFilter(
                  route: widget.route,
                  onRoute: widget.onRoute,
                  choices: [
                    (
                      field: 'agent',
                      values: [for (final (_, r) in read.rows) r.agent],
                    ),
                    (
                      field: 'mission_id',
                      values: [for (final (_, r) in read.rows) r.missionId],
                    ),
                  ],
                );
                if (requests.isEmpty && read.failed.isNotEmpty && !several) {
                  return Center(child: Text(read.failed.values.single));
                }
                final failures = read.failed.isEmpty
                    ? null
                    : OoReadFailures(failed: read.failed, several: several);
                if (requests.isEmpty) {
                  final empty = OoEmptyState(
                    icon: OoIconName.greenInk,
                    title: switch ((requestId, operationId)) {
                      (null, _) => 'No steps are waiting for green ink.',
                      (_, null) =>
                        'Request $requestId is not waiting in an Operation '
                            'you may sign for.',
                      (_, _) =>
                        'Request $requestId is not waiting in Operation '
                            '$operationId.',
                    },
                    body:
                        'An Agent asks here before an irreversible step, such as a push, '
                        'and waits for your signature.',
                  );
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      narrow,
                      ?failures,
                      Expanded(child: empty),
                    ],
                  );
                }
                final list = ListView.separated(
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
                                OoLinkLine([
                                  if (operationId == null)
                                    (
                                      'Operation ${request.operationId}',
                                      OoLinkTo.operation(request.operationId),
                                    ),
                                  (
                                    'Agent ${request.agent}',
                                    OoLinkTo.agent(
                                      request.agent,
                                      operation: request.operationId,
                                    ),
                                  ),
                                  (
                                    'Mission ${widget.board.handleOf(request.operationId, request.missionId)}',
                                    OoLinkTo.mission(
                                      request.operationId,
                                      request.missionId,
                                    ),
                                  ),
                                ], style: DefaultTextStyle.of(context).style),
                                const Text(' · '),
                                OoTime(
                                  request.expiresAt,
                                  prefix: 'expires ',
                                  style: DefaultTextStyle.of(context).style,
                                ),
                              ],
                            ),
                            const SizedBox(height: 8),
                            GreenInkReason(
                              reason: request.reason,
                              links: request.links,
                              sources: widget.board.loadSourcePages(
                                request.operationId,
                              ),
                            ),
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
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    narrow,
                    ?failures,
                    Expanded(child: list),
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

/// The requests no one decided before they expired. One can be re-armed: it waits for a decision
/// again, and its Mission with it. Needs `approval.decide`.
///
/// The [route] (page `green_ink`) names the Operation.
class ExpiredGreenInkPage extends StatefulWidget {
  const ExpiredGreenInkPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<ExpiredGreenInkPage> createState() => _ExpiredGreenInkPageState();
}

class _ExpiredGreenInkPageState extends State<ExpiredGreenInkPage> {
  Future<_Read<ExpiredGreenInkSummary>>? _records;
  bool _rearming = false;

  OoScope get _scope => OoScope(widget.route, widget.board.greenInkOperations);

  @override
  void initState() {
    super.initState();
    _reload();
  }

  @override
  void didUpdateWidget(ExpiredGreenInkPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      _reload();
    }
  }

  void _reload() {
    final shown = _scope.shown;
    setState(() {
      _records = shown.isEmpty
          ? null
          : readEach(shown, widget.board.loadExpiredGreenInk, const []);
    });
  }

  Future<void> _rearm(GreenInkRequestSummary request) async {
    final note = await showDialog<String>(
      context: context,
      builder: (context) => _RearmDialog(request: request),
    );
    if (note == null || !mounted) return;
    setState(() => _rearming = true);
    try {
      await widget.board.rearmGreenInk(
        operationId: request.operationId,
        requestId: request.id,
        note: note.trim().isEmpty ? null : note.trim(),
      );
      if (!mounted) return;
      _reload();
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('It is waiting for a decision again.')),
      );
    } on Object catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.toString())));
      }
    } finally {
      if (mounted) setState(() => _rearming = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = widget.board.greenInkOperations;
    final scope = _scope;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operations grant approval.decide to this account.'),
      );
    }
    final picker = OoOperationPicker(scope: scope, onRoute: widget.onRoute);
    if (scope.unreadable) {
      return Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            picker,
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
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          picker,
          const SizedBox(height: 16),
          Row(
            children: [
              Expanded(
                child: Text(
                  'Steps no one decided in time. They count as refused until re-armed.',
                  style: Theme.of(context).textTheme.titleMedium,
                ),
              ),
              IconButton(
                tooltip: 'Refresh expired green ink',
                onPressed: _rearming ? null : _reload,
                icon: const Icon(Icons.refresh),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Expanded(
            child: FutureBuilder<_Read<ExpiredGreenInkSummary>>(
              future: _records,
              builder: (context, snapshot) {
                if (snapshot.hasError) {
                  return Center(child: Text(snapshot.error.toString()));
                }
                if (!snapshot.hasData) {
                  return const Center(child: CircularProgressIndicator());
                }
                final read = snapshot.data!;
                final several = scope.shown.length > 1;
                if (read.rows.isEmpty && read.failed.isNotEmpty && !several) {
                  return Center(child: Text(read.failed.values.single));
                }
                if (read.rows.isEmpty && read.failed.isEmpty) {
                  return const OoEmptyState(
                    icon: OoIconName.greenInk,
                    title: 'No step has expired without a decision.',
                    body:
                        'A request no one decides before it expires counts as refused '
                        'and is kept here.',
                  );
                }
                return ListView(
                  children: [
                    if (read.failed.isNotEmpty)
                      OoReadFailures(failed: read.failed, several: several),
                    for (final (_, record) in read.rows)
                      _ExpiredCard(
                        board: widget.board,
                        record: record,
                        onRearm: _rearming || widget.board.demo
                            ? null
                            : () => _rearm(record.request),
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

class _ExpiredCard extends StatelessWidget {
  const _ExpiredCard({
    required this.board,
    required this.record,
    required this.onRearm,
  });

  final MissionBoard board;
  final ExpiredGreenInkSummary record;
  final VoidCallback? onRearm;

  @override
  Widget build(BuildContext context) {
    final request = record.request;
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
                OoLinkLine([
                  (
                    'Agent ${request.agent}',
                    OoLinkTo.agent(
                      request.agent,
                      operation: request.operationId,
                    ),
                  ),
                  (
                    'Mission ${board.handleOf(request.operationId, request.missionId)}',
                    OoLinkTo.mission(request.operationId, request.missionId),
                  ),
                ], style: DefaultTextStyle.of(context).style),
                const Text(' · '),
                OoTime(
                  record.expiredAt,
                  prefix: 'expired ',
                  style: DefaultTextStyle.of(context).style,
                ),
              ],
            ),
            const SizedBox(height: 8),
            GreenInkReason(
              reason: request.reason,
              links: request.links,
              sources: board.loadSourcePages(request.operationId),
            ),
            const SizedBox(height: 8),
            OutlinedButton(onPressed: onRearm, child: const Text('Re-arm…')),
          ],
        ),
      ),
    );
  }
}

class _RearmDialog extends StatefulWidget {
  const _RearmDialog({required this.request});

  final GreenInkRequestSummary request;

  @override
  State<_RearmDialog> createState() => _RearmDialogState();
}

class _RearmDialogState extends State<_RearmDialog> {
  final _note = TextEditingController();

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final request = widget.request;
    return AlertDialog(
      title: const Text('Re-arm this request?'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${request.agent} waits again for a decision to ${request.kind} '
            '${request.target} in Mission ${request.missionId}.',
          ),
          TextField(
            controller: _note,
            decoration: const InputDecoration(labelText: 'Why (recorded)'),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(_note.text),
          child: const Text('Re-arm'),
        ),
      ],
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
  Future<_Read<ClassAuthorizationSummary>>? _records;
  bool _revoking = false;

  OoScope get _scope =>
      OoScope(widget.route, widget.board.classAuthorizationOperations);

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
    final shown = _scope.shown;
    setState(() {
      _records = shown.isEmpty
          ? null
          : readEach(shown, widget.board.loadClassAuthorizations, const []);
    });
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
    final scope = _scope;
    final operationId = scope.operation;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operations grant approval.class to this account.'),
      );
    }
    final picker = OoOperationPicker(scope: scope, onRoute: widget.onRoute);
    final filters = OoFilterBar(
      route: widget.route,
      onRoute: widget.onRoute,
      padding: const EdgeInsets.only(top: 8),
    );
    if (scope.unreadable) {
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
            child: FutureBuilder<_Read<ClassAuthorizationSummary>>(
              future: _records,
              builder: (context, snapshot) {
                if (snapshot.hasError) {
                  return Center(child: Text(snapshot.error.toString()));
                }
                if (!snapshot.hasData) {
                  return const Center(child: CircularProgressIndicator());
                }
                final read = snapshot.data!;
                final several = scope.shown.length > 1;
                if (read.rows.isEmpty && read.failed.isNotEmpty && !several) {
                  return Center(child: Text(read.failed.values.single));
                }
                if (read.rows.isEmpty && read.failed.isEmpty) {
                  return Center(
                    child: Text(
                      operationId == null
                          ? 'No class authorisations in an Operation you may '
                                'manage them in. A class authorisation signs a kind '
                                'of step in advance.'
                          : 'No class authorisations in this Operation. A class '
                                'authorisation signs a kind of step in advance.',
                      textAlign: TextAlign.center,
                    ),
                  );
                }
                final now = _now();
                return ListView(
                  children: [
                    if (read.failed.isNotEmpty)
                      OoReadFailures(failed: read.failed, several: several),
                    for (final (from, record) in read.rows)
                      ListTile(
                        title: Text(
                          '${record.kinds.join(', ')} · ${record.targetPattern}',
                        ),
                        subtitle: Text(
                          [
                            if (operationId == null) 'Operation $from',
                            if (record.agents.isNotEmpty)
                              'Agents ${record.agents.join(', ')}',
                            if (record.roles.isNotEmpty)
                              'roles ${record.roles.join(', ')}',
                            '${record.clearanceMin} and above',
                            record.maxUses == null
                                ? '${record.uses} uses'
                                : '${record.uses}/${record.maxUses} uses',
                            'expires ${ooRelativeTime(record.expiresAt)}',
                            'signed by ${record.signer}',
                          ].join(' · '),
                        ),
                        trailing: record.status(now) == 'live'
                            ? TextButton(
                                onPressed: _revoking || widget.board.demo
                                    ? null
                                    : () => _revoke(from, record),
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
