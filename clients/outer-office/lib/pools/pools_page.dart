import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../widgets/oo_data_table.dart';
import '../routes/oo_route.dart';
import '../routes/oo_scope.dart';
import '../settings/oo_config.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'models_section.dart';
import '../shell/oo_empty.dart';
import '../shell/oo_freshness.dart';
import '../shell/oo_link.dart';
import '../shell/operation_names.dart';

/// How much of one provider quota window an account has used.
@immutable
class QuotaWindowSummary {
  const QuotaWindowSummary({
    required this.name,
    this.usedPermille,
    this.resetsAt,
    this.status,
    this.models = const [],
    this.observedAt,
  });

  final String name;

  /// The models it covers, when the provider names them; empty for every model.
  final List<String> models;

  /// When it was last read, in Unix seconds.
  final double? observedAt;

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
        models: [...?(json['models'] as List<dynamic>?)?.cast<String>()],
        observedAt: (json['observed_at'] as num?)?.toDouble(),
      );

  /// `five hour 42.5%`, or `five hour unknown` when no figure was reported.
  String get label {
    final window =
        '${name.replaceAll('_', ' ')}${models.isEmpty ? '' : ' (${models.join(', ')})'}';
    return switch (usedPermille) {
      final used? => '$window ${used ~/ 10}.${used % 10}%',
      null => '$window ${status ?? 'unknown'}',
    };
  }
}

/// A request waiting for a Station that holds a login to act on it.
@immutable
class PoolLoginRequest {
  const PoolLoginRequest({
    required this.action,
    this.station,
    this.by = '',
    this.at,
  });

  /// `add_station`, `remove_station` or `sign_out_and_forget`.
  final String action;
  final String? station;
  final String by;

  /// Unix seconds.
  final double? at;

  factory PoolLoginRequest.fromJson(Map<String, dynamic> json) =>
      PoolLoginRequest(
        action: json['action'] as String,
        station: json['station'] as String?,
        by: json['by'] as String? ?? '',
        at: (json['at'] as num?)?.toDouble(),
      );

  /// What it asks, as `adding Station x`.
  String get label => switch (action) {
    'add_station' => 'adding Station ${station ?? ''}'.trim(),
    'remove_station' => 'removing Station ${station ?? ''}'.trim(),
    'sign_out_and_forget' => 'signing out and forgetting the login',
    _ => action.replaceAll('_', ' '),
  };
}

/// The state of a pool account's sealed login: never its contents.
@immutable
class PoolLogin {
  const PoolLogin({
    this.stations = const [],
    this.expiresAt,
    this.renewedAt,
    this.renewalHeldBy,
    this.renewalHeldUntil,
    this.lastResult,
    this.lastAttemptAt,
    this.failures = 0,
    this.needsPerson,
    this.needsPersonCommand,
    this.requests = const [],
  });

  /// The Stations the login is sealed to.
  final List<String> stations;

  /// Unix seconds, when its files say.
  final double? expiresAt;
  final double? renewedAt;
  final String? renewalHeldBy;
  final double? renewalHeldUntil;

  /// `renewed`, `unchanged`, `added` or `failed: <why>`.
  final String? lastResult;
  final double? lastAttemptAt;
  final int failures;

  /// Why a person is needed, when one is.
  final String? needsPerson;
  final String? needsPersonCommand;
  final List<PoolLoginRequest> requests;

  factory PoolLogin.fromJson(Map<String, dynamic> json) => PoolLogin(
    stations: [...?(json['stations'] as List<dynamic>?)?.cast<String>()],
    expiresAt: (json['expires_at'] as num?)?.toDouble(),
    renewedAt: (json['renewed_at'] as num?)?.toDouble(),
    renewalHeldBy: json['renewal_held_by'] as String?,
    renewalHeldUntil: (json['renewal_held_until'] as num?)?.toDouble(),
    lastResult: json['last_result'] as String?,
    lastAttemptAt: (json['last_attempt_at'] as num?)?.toDouble(),
    failures: json['failures'] as int? ?? 0,
    needsPerson: json['needs_person'] as String?,
    needsPersonCommand: json['needs_person_command'] as String?,
    requests: [
      for (final request in (json['requests'] as List<dynamic>?) ?? const [])
        PoolLoginRequest.fromJson(request as Map<String, dynamic>),
    ],
  );

  /// `Renewal held by station-a until in 4 min`, while a Station holds the renewal.
  String? get holderLabel => switch (renewalHeldBy) {
    final holder? =>
      'Renewal held by $holder'
          '${renewalHeldUntil == null ? '' : ' until ${ooRelativeTime(renewalHeldUntil!)}'}',
    null => null,
  };

  /// `Renewed 3 h ago`, or `Renewal failed: why` when the last attempt failed.
  String? get renewalLabel {
    final result = lastResult;
    if (result != null && result.startsWith('failed')) {
      final why = result.substring(6).replaceFirst(RegExp(r'^:\s*'), '');
      return 'Renewal failed${why.isEmpty ? '' : ': $why'}'
          '${lastAttemptAt == null ? '' : ' (${ooRelativeTime(lastAttemptAt!)})'}';
    }
    if (renewedAt == null) return null;
    final verb = result == 'added' ? 'Added' : 'Renewed';
    return '$verb ${ooRelativeTime(renewedAt!)}';
  }

  /// `Ends in 2 d`, or `Ended 1 h ago`.
  String? get expiryLabel => switch (expiresAt) {
    final at? =>
      at > ooClock().millisecondsSinceEpoch / 1000
          ? 'Ends ${ooRelativeTime(at)}'
          : 'Ended ${ooRelativeTime(at)}',
    null => null,
  };

  /// `May be held by station-a, station-b`, or that no Station may.
  String get stationsLabel => stations.isEmpty
      ? 'No Station may hold it'
      : 'May be held by ${stations.join(', ')}';

  /// One line per request still waiting.
  List<String> get requestLabels => [
    for (final request in requests)
      'Waiting for a Station holding the login: ${request.label}',
  ];

  /// What a person is to run, when one is needed.
  String? get needsPersonLabel => switch (needsPerson) {
    final why? =>
      'Needs a person: $why.'
          '${needsPersonCommand == null ? '' : ' Run `$needsPersonCommand` on a host where you are signed in.'}',
    null => null,
  };
}

/// A change to an account's login a person asks for.
sealed class PoolLoginChange {
  const PoolLoginChange();
}

/// Seal the login to one more Station.
class PoolLoginAddStation extends PoolLoginChange {
  const PoolLoginAddStation(this.station);
  final String station;
}

/// Stop sealing the login to a Station.
class PoolLoginRemoveStation extends PoolLoginChange {
  const PoolLoginRemoveStation(this.station);
  final String station;
}

/// Forget the login, first signing it out at the provider when [signOut].
class PoolLoginForget extends PoolLoginChange {
  const PoolLoginForget({this.signOut = false});
  final bool signOut;
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
    this.otherRuns = 0,
    this.maxConcurrent,
    this.windows = const [],
    this.limitedUntil,
    this.tokensToday = 0,
    this.login,
  });

  final String pool;
  final String account;
  final String handler;
  final String kind;

  /// This Operation's Missions whose Runs use it now.
  final List<String> active;

  /// How many Runs of other Operations use it now.
  final int otherRuns;
  final int? maxConcurrent;
  final List<QuotaWindowSummary> windows;

  /// Unix seconds, while its provider refuses work.
  final double? limitedUntil;
  final int tokensToday;

  /// The sealed login it keeps, or null for an account that keeps none.
  final PoolLogin? login;

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
        otherRuns: json['other_runs'] as int? ?? 0,
        maxConcurrent: json['max_concurrent'] as int?,
        windows: [
          for (final window in json['windows'] as List<dynamic>)
            QuotaWindowSummary.fromJson(window as Map<String, dynamic>),
        ],
        limitedUntil: (json['limited_until'] as num?)?.toDouble(),
        tokensToday: json['tokens_today'] as int,
        login: switch (json['login']) {
          final Map<String, dynamic> login => PoolLogin.fromJson(login),
          _ => null,
        },
      );

  /// `1/2 Runs`, or `1 Run` without a limit.
  String get runs {
    final count = active.length + otherRuns;
    final noun = (maxConcurrent ?? count) == 1 ? 'Run' : 'Runs';
    return maxConcurrent == null
        ? '$count $noun'
        : '$count/$maxConcurrent $noun';
  }

  /// When its usage was last read: the newest of its windows'.
  String? get readLabel {
    final read = [for (final window in windows) ?window.observedAt];
    if (read.isEmpty) return null;
    final newest = read.reduce((a, b) => a > b ? a : b);
    return newest > 0 ? 'Usage read ${ooRelativeTime(newest)}' : null;
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
        OoFilterBar(
          route: withoutTableFields(widget.route, const ['handler']),
          onRoute: widget.onRoute,
        ),
        if (_failed.isNotEmpty && (shown.length > 1 || accounts.isNotEmpty))
          OoReadFailures(failed: _failed, several: shown.length > 1),
        Expanded(
          child: scope.unreadable
              ? Center(
                  child: Text(
                    'Operation ${OperationNames.of('${widget.route['operation_id']}')}\'s account pools '
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
                      'No account pool named $pool in Operation ${OperationNames.of('$operation')}.',
                  },
                  body:
                      'A pool shares logins among Runs. Without one, a Run uses '
                      'the login its Agent is given.',
                )
              : Column(
                  children: [
                    if (widget.board.canReadRoster)
                      ModelsSection(board: widget.board),
                    Expanded(
                      child: OoDataTable<OoGrouped<PoolAccountSummary>>(
                        rows: groupByEntity(
                          accounts,
                          (account) => '${account.pool}/${account.account}',
                        ),
                        rowId: (row) => '${row.row.pool}/${row.row.account}',
                        noun: 'pool accounts',
                        route: widget.route,
                        onRoute: widget.onRoute,
                        defaultSort: ('pool', true),
                        searchText: (row) =>
                            '${row.row.pool} ${row.row.account} ${row.row.handler} '
                            '${row.row.kind} ${row.operations.join(' ')}',
                        facets: [
                          OoFacet(
                            id: 'handler',
                            label: 'Handler',
                            values: (row) => [row.row.handler],
                          ),
                        ],
                        columns: _columns(),
                      ),
                    ),
                  ],
                ),
        ),
      ],
    );
  }

  /// The Operation in which the person may change [row]'s login, when there is one.
  String? _manageIn(OoGrouped<PoolAccountSummary> row) {
    final may = widget.board.poolManageOperations;
    for (final operation in row.operations) {
      if (may.contains(operation)) return operation;
    }
    return null;
  }

  Future<void> _change(
    String operation,
    PoolAccountSummary account,
    PoolLoginChange change,
  ) async {
    final messenger = ScaffoldMessenger.of(context);
    String outcome;
    try {
      outcome = await widget.board.changePoolLogin(
        operation,
        account.pool,
        account.account,
        change,
      );
    } on Object catch (error) {
      outcome = error is StateError
          ? error.message
          : 'The change could not be sent.';
    }
    messenger.showSnackBar(SnackBar(content: Text(outcome)));
    if (mounted) unawaited(_load());
  }

  Future<void> _addStation(String operation, PoolAccountSummary account) async {
    final station = await showDialog<String>(
      context: context,
      builder: (context) => const _StationNameDialog(),
    );
    if (station == null || station.isEmpty || !mounted) return;
    await _change(operation, account, PoolLoginAddStation(station));
  }

  Future<void> _removeLogin(
    String operation,
    PoolAccountSummary account,
  ) async {
    var signOut = false;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: const Text('Remove login'),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'The login kept for ${account.pool}/${account.account} is '
                'forgotten: no Station keeps it afterwards.',
              ),
              CheckboxListTile(
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                value: signOut,
                onChanged: (value) => setState(() => signOut = value ?? false),
                title: const Text('Also sign out at the provider'),
              ),
            ],
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text('Remove login'),
            ),
          ],
        ),
      ),
    );
    if (confirmed != true || !mounted) return;
    await _change(operation, account, PoolLoginForget(signOut: signOut));
  }

  /// The sealed login's state, and what a person may do to it.
  Widget _loginBlock(
    BuildContext context,
    OoGrouped<PoolAccountSummary> row,
    PoolLogin login,
  ) {
    final tokens = OoTokens.of(context);
    final account = row.row;
    final operation = _manageIn(row);
    final needs = login.needsPersonLabel;
    final lines = [
      ?login.holderLabel,
      ?login.renewalLabel,
      ?login.expiryLabel,
      login.stationsLabel,
      ...login.requestLabels,
    ];
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (needs != null)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(Icons.warning_amber, size: 18, color: tokens.warning),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    needs,
                    style: TextStyle(
                      color: tokens.warning,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ],
            ),
          Text('Login', style: Theme.of(context).textTheme.labelMedium),
          for (final line in lines) Text(line),
          if (operation != null)
            Wrap(
              spacing: 8,
              runSpacing: 4,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                for (final station in login.stations)
                  InputChip(
                    label: Text(station),
                    deleteButtonTooltipMessage: 'Remove Station $station',
                    onDeleted: () => unawaited(
                      _change(
                        operation,
                        account,
                        PoolLoginRemoveStation(station),
                      ),
                    ),
                  ),
                TextButton(
                  onPressed: () => unawaited(_addStation(operation, account)),
                  child: const Text('Add Station'),
                ),
                TextButton(
                  onPressed: () => unawaited(_removeLogin(operation, account)),
                  child: const Text('Remove login'),
                ),
              ],
            ),
        ],
      ),
    );
  }

  List<OoColumn<OoGrouped<PoolAccountSummary>>> _columns() => [
    OoColumn(
      id: 'pool',
      label: 'Pool and account',
      flex: 3,
      sortKey: (row) => '${row.row.pool}/${row.row.account}',
      cell: (context, row) => Row(
        children: [
          Icon(
            row.row.login?.needsPerson != null
                ? Icons.warning_amber
                : row.row.limitedLabel == null
                ? Icons.vpn_key_outlined
                : Icons.hourglass_bottom,
            size: 18,
          ),
          const SizedBox(width: 6),
          Flexible(
            child: Text(
              '${row.row.pool}/${row.row.account}',
              style: Theme.of(context).textTheme.titleSmall,
              overflow: TextOverflow.ellipsis,
            ),
          ),
        ],
      ),
    ),
    OoColumn(
      id: 'handler',
      label: 'Handler',
      flex: 2,
      sortKey: (row) => row.row.handler,
      cell: (context, row) => Text(row.row.handler),
    ),
    OoColumn(
      id: 'kind',
      label: 'Login kind',
      flex: 2,
      sortKey: (row) => row.row.kind,
      cell: (context, row) => Text(row.row.kind),
    ),
    OoColumn(
      id: 'operations',
      label: 'Operations',
      flex: 2,
      cell: (context, row) => OoOperationBadges(row.operations),
    ),
    OoColumn(
      id: 'use',
      label: 'Use',
      flex: 5,
      cell: (context, row) {
        final account = row.row;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (account.highestUse case final use?)
              LinearProgressIndicator(value: use),
            Text(
              [
                account.runs,
                if (account.windows.isNotEmpty)
                  account.windows.map((window) => window.label).join(', '),
                '${account.tokensToday} tokens today',
                ?account.limitedLabel,
              ].join(' · '),
            ),
            if (account.readLabel case final read?) Text(read),
            if (account.login case final login?)
              _loginBlock(context, row, login),
            for (final (operation, read) in row.reads)
              if (read.active.isNotEmpty)
                OoLinkLine([
                  ('Used by', null),
                  for (final mission in read.active)
                    ('Mission $mission', OoLinkTo.mission(operation, mission)),
                ]),
          ],
        );
      },
    ),
  ];
}

/// Asks for the name of the Station to add; pops the trimmed name.
class _StationNameDialog extends StatefulWidget {
  const _StationNameDialog();

  @override
  State<_StationNameDialog> createState() => _StationNameDialogState();
}

class _StationNameDialogState extends State<_StationNameDialog> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Add Station'),
    content: TextField(
      controller: _controller,
      autofocus: true,
      decoration: const InputDecoration(labelText: 'Station name'),
      onSubmitted: (value) => Navigator.of(context).pop(value.trim()),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: () => Navigator.of(context).pop(_controller.text.trim()),
        child: const Text('Add'),
      ),
    ],
  );
}
