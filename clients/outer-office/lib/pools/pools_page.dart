import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../settings/oo_config.dart';

/// How much of one provider quota window an account has used.
@immutable
class QuotaWindowSummary {
  const QuotaWindowSummary({
    required this.name,
    this.usedPermille,
    this.resetsAt,
    this.status,
  });

  final String name;

  /// Thousandths used, when the provider reported a figure.
  final int? usedPermille;

  /// Unix seconds.
  final double? resetsAt;
  final String? status;

  factory QuotaWindowSummary.fromJson(Map<String, dynamic> json) =>
      QuotaWindowSummary(
        name: json['name'] as String,
        usedPermille: json['used_permille'] as int?,
        resetsAt: (json['resets_at'] as num?)?.toDouble(),
        status: json['status'] as String?,
      );

  /// `five_hour 42.5%`, or `five_hour ?` when no figure was reported.
  String get label => switch (usedPermille) {
    final used? => '$name ${used ~/ 10}.${used % 10}%',
    null => '$name ${status ?? '?'}',
  };
}

/// One pool account as the board shows it: never a credential.
@immutable
class PoolAccountSummary {
  const PoolAccountSummary({
    required this.pool,
    required this.account,
    required this.handler,
    required this.kind,
    required this.active,
    this.otherSessions = 0,
    this.maxConcurrent,
    this.windows = const [],
    this.limitedUntil,
    this.tokensToday = 0,
  });

  final String pool;
  final String account;
  final String handler;
  final String kind;

  /// This Operation's Missions whose Sessions use it now.
  final List<String> active;

  /// How many Sessions of other Operations use it now.
  final int otherSessions;
  final int? maxConcurrent;
  final List<QuotaWindowSummary> windows;

  /// Unix seconds, while its provider refuses work.
  final double? limitedUntil;
  final int tokensToday;

  factory PoolAccountSummary.fromJson(Map<String, dynamic> json) =>
      PoolAccountSummary(
        pool: json['pool'] as String,
        account: json['account'] as String,
        handler: json['handler'] as String,
        kind: json['kind'] as String,
        active: [
          for (final claim in json['active'] as List<dynamic>)
            (claim as Map<String, dynamic>)['mission_id'] as String,
        ],
        otherSessions: json['other_sessions'] as int? ?? 0,
        maxConcurrent: json['max_concurrent'] as int?,
        windows: [
          for (final window in json['windows'] as List<dynamic>)
            QuotaWindowSummary.fromJson(window as Map<String, dynamic>),
        ],
        limitedUntil: (json['limited_until'] as num?)?.toDouble(),
        tokensToday: json['tokens_today'] as int,
      );

  /// `1/2 Sessions`, or `1 Session` without a limit.
  String get sessions {
    final count = active.length + otherSessions;
    final noun = (maxConcurrent ?? count) == 1 ? 'Session' : 'Sessions';
    return maxConcurrent == null
        ? '$count $noun'
        : '$count/$maxConcurrent $noun';
  }

  /// The highest window use, as a fraction, when any was reported.
  double? get highestUse {
    final used = [for (final window in windows) ?window.usedPermille];
    if (used.isEmpty) return null;
    return used.reduce((a, b) => a > b ? a : b) / 1000;
  }

  /// When it comes back, in UTC to the minute.
  String? get limitedLabel => switch (limitedUntil) {
    final until? =>
      'Limited until ${DateTime.fromMillisecondsSinceEpoch((until * 1000).round(), isUtc: true).toIso8601String().substring(0, 16)}Z',
    null => null,
  };
}

/// The pool accounts an Operation may draw from.
///
/// The [route] (page `pools`) names the Operation and may narrow the list to one `pool`.
class AccountPoolsPage extends StatefulWidget {
  const AccountPoolsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    this.refreshEvery = RefreshSettings.defaultPools,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  /// How often the accounts are fetched again while the page is shown.
  final Duration? refreshEvery;

  @override
  State<AccountPoolsPage> createState() => _AccountPoolsPageState();
}

class _AccountPoolsPageState extends State<AccountPoolsPage> {
  List<PoolAccountSummary> _accounts = const [];
  bool _loading = false;
  String? _error;
  Timer? _timer;

  /// The route's Operation when the person may read it, `null` when not, else the first.
  String? get _operation {
    final operations = widget.board.poolOperations;
    final named = widget.route['operation_id'];
    if (named != null) return operations.contains(named) ? named : null;
    return operations.isEmpty ? null : operations.first;
  }

  @override
  void initState() {
    super.initState();
    unawaited(_load());
    if (widget.refreshEvery case final every?) {
      _timer = Timer.periodic(every, (_) => unawaited(_load()));
    }
  }

  @override
  void didUpdateWidget(AccountPoolsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      setState(() {
        _accounts = const [];
        _error = null;
      });
      unawaited(_load());
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    final operation = _operation;
    if (operation == null) return;
    setState(() => _loading = true);
    try {
      final accounts = await widget.board.loadPools(operation);
      if (!mounted || operation != _operation) return;
      setState(() {
        _accounts = accounts;
        _error = null;
      });
    } on StateError catch (error) {
      if (mounted) setState(() => _error = error.message);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = widget.board.poolOperations;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No account pool is visible to this account.'),
      );
    }
    final operation = _operation;
    final pool = widget.route['pool'];
    final accounts = [
      for (final account in _accounts)
        if (pool == null || account.pool == pool) account,
    ];
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
                        .without('pool')
                        .withField('operation_id', chosen),
                  );
                },
              ),
              IconButton(
                tooltip: 'Refresh account pools',
                onPressed: () => unawaited(_load()),
                icon: const Icon(Icons.refresh),
              ),
            ],
          ),
        ),
        OoFilterBar(route: widget.route, onRoute: widget.onRoute),
        if (_error case final error?)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 24),
            child: Text(error),
          ),
        Expanded(
          child: operation == null
              ? Center(
                  child: Text(
                    'Operation ${widget.route['operation_id']}\'s account pools '
                    'are not visible to this account.',
                  ),
                )
              : accounts.isEmpty
              ? Center(
                  child: _loading
                      ? const CircularProgressIndicator()
                      : Text(
                          pool == null
                              ? 'No pool account serves this Operation.'
                              : 'No account pool named $pool in Operation '
                                    '$operation.',
                        ),
                )
              : ListView(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 24),
                  children: [
                    for (final account in accounts)
                      _AccountTile(account: account),
                  ],
                ),
        ),
      ],
    );
  }
}

class _AccountTile extends StatelessWidget {
  const _AccountTile({required this.account});

  final PoolAccountSummary account;

  @override
  Widget build(BuildContext context) {
    final limited = account.limitedLabel;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  limited == null
                      ? Icons.vpn_key_outlined
                      : Icons.hourglass_bottom,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    '${account.pool}/${account.account}',
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                ),
                Text('${account.handler} · ${account.kind}'),
              ],
            ),
            const SizedBox(height: 8),
            if (account.highestUse case final use?)
              LinearProgressIndicator(value: use),
            const SizedBox(height: 8),
            Text(
              [
                account.sessions,
                if (account.windows.isNotEmpty)
                  account.windows.map((window) => window.label).join(', '),
                '${account.tokensToday} tokens today',
                ?limited,
              ].join(' · '),
            ),
            if (account.active.isNotEmpty)
              Text(
                'Used by ${account.active.join(', ')}',
                style: Theme.of(context).textTheme.bodySmall,
              ),
          ],
        ),
      ),
    );
  }
}
