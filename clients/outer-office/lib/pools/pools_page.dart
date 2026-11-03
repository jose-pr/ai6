import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../routes/oo_scope.dart';
import '../settings/oo_config.dart';
import '../theme/oo_icon.dart';
import 'models_section.dart';
import '../shell/oo_empty.dart';
import '../shell/oo_freshness.dart';
import '../shell/oo_link.dart';

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

  /// `five hour 42.5%`, or `five hour unknown` when no figure was reported.
  String get label {
    final window = name.replaceAll('_', ' ');
    return switch (usedPermille) {
      final used? => '$window ${used ~/ 10}.${used % 10}%',
      null => '$window ${status ?? 'unknown'}',
    };
  }
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

  /// When it comes back.
  String? get limitedLabel => switch (limitedUntil) {
    final until? => 'Limited, back ${ooRelativeTime(until.toDouble())}',
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
  /// The accounts shown, each with the Operation it was read in.
  List<(String, PoolAccountSummary)> _accounts = const [];

  /// The Operations whose accounts could not be read, with why.
  Map<String, String> _failed = const {};
  bool _loading = false;
  Timer? _timer;

  OoScope get _scope => OoScope(widget.route, widget.board.poolOperations);

  /// The read error to show in place of the list: the one Operation's, when only one is read.
  String? get _error =>
      _scope.shown.length == 1 ? _failed[_scope.shown.single] : null;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
    if (widget.refreshEvery case final every?) {
      _timer = Timer.periodic(every, (_) {
        if (widget.board.visible) unawaited(_load());
      });
    }
  }

  @override
  void didUpdateWidget(AccountPoolsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      setState(() {
        _accounts = const [];
        _failed = const {};
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
    final shown = _scope.shown;
    if (shown.isEmpty) return;
    setState(() => _loading = true);
    final read = await readEach(shown, widget.board.loadPools, _accounts);
    if (!mounted || shown.join('\n') != _scope.shown.join('\n')) return;
    setState(() {
      _accounts = read.rows;
      _failed = read.failed;
      _loading = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    final operations = widget.board.poolOperations;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No account pool is visible to this account.'),
      );
    }
    final scope = _scope;
    final operation = scope.operation;
    final shown = scope.shown;
    final pool = widget.route['pool'];
    final mission = widget.route['mission_id'];
    final accounts = [
      for (final row in _accounts)
        if ((pool == null || row.$2.pool == pool) &&
            (mission == null || row.$2.active.contains(mission)))
          row,
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
              OoOperationPicker(
                scope: scope,
                onRoute: widget.onRoute,
                narrower: const ['pool'],
              ),
              IconButton(
                tooltip: 'Refresh account pools',
                onPressed: () => unawaited(_load()),
                icon: const Icon(Icons.refresh),
              ),
              OoAddFilter(
                route: widget.route,
                onRoute: widget.onRoute,
                choices: [
                  (
                    field: 'pool',
                    values: [
                      for (final (_, account) in _accounts) account.pool,
                    ],
                  ),
                  (
                    field: 'mission_id',
                    values: [
                      for (final (_, account) in _accounts) ...account.active,
                    ],
                  ),
                ],
              ),
            ],
          ),
        ),
        OoFilterBar(route: widget.route, onRoute: widget.onRoute),
        if (_failed.isNotEmpty && (shown.length > 1 || accounts.isNotEmpty))
          OoReadFailures(failed: _failed, several: shown.length > 1),
        Expanded(
          child: scope.unreadable
              ? Center(
                  child: Text(
                    'Operation ${widget.route['operation_id']}\'s account pools '
                    'are not visible to this account.',
                  ),
                )
              : accounts.isEmpty && _error != null
              ? OoEmptyState(
                  icon: OoIconName.pools,
                  title: _error!,
                  body: 'Nothing is shown because it could not be read.',
                  action: ('Try again', () => unawaited(_load())),
                )
              : accounts.isEmpty && _loading
              ? const Center(child: CircularProgressIndicator())
              : accounts.isEmpty
              ? OoEmptyState(
                  icon: OoIconName.pools,
                  title: switch ((pool, operation)) {
                    (null, null) => 'No pool account serves an Operation this account may read.',
                    (null, _) => 'No pool account serves this Operation.',
                    (_, null) =>
                      'No account pool named $pool in an Operation this account may read.',
                    (_, _) =>
                      'No account pool named $pool in Operation $operation.',
                  },
                  body:
                      'A pool shares logins among Sessions. Without one, a Session uses '
                      'the login its Agent is given.',
                )
              : ListView(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 24),
                  children: [
                    if (widget.board.canReadRoster)
                      ModelsSection(board: widget.board),
                    for (final group in shown) ...[
                      if (operation == null &&
                          accounts.any((row) => row.$1 == group))
                        OoOperationHeading(group),
                      for (final (from, account) in accounts)
                        if (from == group)
                          _AccountTile(account: account, operation: from),
                    ],
                  ],
                ),
        ),
      ],
    );
  }
}

class _AccountTile extends StatelessWidget {
  const _AccountTile({required this.account, required this.operation});

  final PoolAccountSummary account;
  final String operation;

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
              OoLinkLine([
                ('Used by', null),
                for (final mission in account.active)
                  ('Mission $mission', OoLinkTo.mission(operation, mission)),
              ]),
          ],
        ),
      ),
    );
  }
}
