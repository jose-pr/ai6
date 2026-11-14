import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../reports/reports_model.dart';
import '../routes/oo_route.dart';
import '../shell/oo_empty.dart';
import '../shell/oo_freshness.dart';
import '../shell/oo_shell.dart' show OoPage;
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'attachment.dart';
import 'attach_view.dart';
import 'charges_model.dart';
import 'mission_attach.dart' show attachNames;

/// The page width from which the selected charge sits beside the list.
const _splitFrom = 1000.0;

const _tabs = [
  ('live', 'Live'),
  ('decisions', 'Decisions'),
  ('logbook', 'Logbook'),
];

/// The Charges page: M's charge, each Operation's and each Station's, with its Run's state,
/// the Agent and where it runs and when it last woke; the selected charge opens beside the
/// list (under it on a narrow screen) with its live attachment, what it decided and its
/// logbook.
class ChargesPage extends StatefulWidget {
  const ChargesPage({
    required this.board,
    required this.route,
    required this.onRoute,
    this.connect,
    this.refreshEvery,
    super.key,
  });

  final MissionBoard board;

  /// A Charges route: `charge=m`, `operation_id` or `station_id` selects a charge, and `tab`
  /// picks what the detail shows.
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  /// Opens the attach socket; tests give a fake one. The gateway's by default.
  final AttachChannel Function()? connect;

  /// How often the list is read again while the page is shown; never when null.
  final Duration? refreshEvery;

  @override
  State<ChargesPage> createState() => _ChargesPageState();
}

class _ChargesPageState extends State<ChargesPage> {
  late Future<List<ChargeSummary>> _charges = widget.board.loadCharges();
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    if (widget.refreshEvery case final every?) {
      _timer = Timer.periodic(every, (_) {
        if (widget.board.visible) _refresh();
      });
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  void _refresh() {
    if (!mounted) return;
    setState(() => _charges = widget.board.loadCharges());
  }

  AttachChannel Function() get _connect =>
      widget.connect ?? () => connectAttach(widget.board.apiOrigin);

  /// The scope the route selects, or null.
  String? get _selected {
    final route = widget.route;
    if (route['charge'] == 'm') return 'm';
    if (route['operation_id'] case final operation?) {
      return 'operation:$operation';
    }
    if (route['station_id'] case final station?) return 'station:$station';
    return null;
  }

  String get _tab => widget.route['tab'] ?? 'live';

  void _select(ChargeSummary charge) =>
      _pick(charge, widget.route['tab'] ?? 'live');

  void _close() => widget.onRoute(OoRoute(OoPage.charges));

  void _pick(ChargeSummary charge, String tab) {
    var route = OoRoute(OoPage.charges);
    for (final MapEntry(:key, :value) in chargeRouteFields(
      charge.scope,
    ).entries) {
      route = route.withField(key, value);
    }
    widget.onRoute(route.withField('tab', tab));
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<List<ChargeSummary>>(
    future: _charges,
    builder: (context, snapshot) {
      if (snapshot.hasError) {
        return OoEmptyState(
          icon: OoIconName.runs,
          title: 'Charges could not be read.',
          body: '${snapshot.error}',
          action: ('Try again', _refresh),
        );
      }
      final charges = snapshot.data;
      if (charges == null) {
        return const Center(child: CircularProgressIndicator());
      }
      if (charges.isEmpty) {
        return const OoEmptyState(
          icon: OoIconName.runs,
          title: 'No charge to show.',
          body:
              'A charge is a standing Run that keeps M\'s area, an Operation '
              'or a Station. The Roster\'s Charges records set them up.',
        );
      }
      return _body(context, charges);
    },
  );

  Widget _body(BuildContext context, List<ChargeSummary> charges) {
    final selected = charges
        .where((charge) => charge.scope == _selected)
        .firstOrNull;
    return LayoutBuilder(
      builder: (context, box) {
        final split = box.maxWidth >= _splitFrom;
        final detail = selected == null
            ? null
            : _ChargeDetail(
                // Keyed so that another charge opens its own attachment.
                key: ValueKey(selected.scope),
                board: widget.board,
                charge: selected,
                tab: _tab,
                connect: _connect,
                onTab: (tab) => _pick(selected, tab),
                onClose: split ? null : _close,
              );
        if (!split && detail != null) return detail;
        final list = ListView(
          padding: const EdgeInsets.all(16),
          children: [
            Align(
              alignment: Alignment.centerRight,
              child: TextButton.icon(
                onPressed: _refresh,
                icon: const Icon(Icons.refresh, size: 18),
                label: const Text('Refresh'),
              ),
            ),
            for (final charge in charges)
              _ChargeCard(
                charge: charge,
                selected: charge.scope == selected?.scope,
                onTap: () => _select(charge),
              ),
          ],
        );
        if (!split) return list;
        return Row(
          children: [
            Expanded(flex: 2, child: list),
            VerticalDivider(width: 1, color: OoTokens.of(context).border),
            Expanded(
              flex: 3,
              child:
                  detail ??
                  Center(
                    child: Text(
                      'Pick a charge to see its Run.',
                      style: TextStyle(color: OoTokens.of(context).muted),
                    ),
                  ),
            ),
          ],
        );
      },
    );
  }
}

class _ChargeCard extends StatelessWidget {
  const _ChargeCard({
    required this.charge,
    required this.selected,
    required this.onTap,
  });

  final ChargeSummary charge;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final muted = text.bodySmall?.copyWith(color: tokens.muted);
    return Card(
      key: ValueKey('charge-${charge.scope}'),
      color: selected ? tokens.raised : null,
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(14),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Expanded(child: Text(charge.title, style: text.titleMedium)),
                  _StateChip(charge.state),
                ],
              ),
              const SizedBox(height: 4),
              Text(charge.agentLine, style: text.bodyMedium),
              const SizedBox(height: 4),
              Text(
                charge.station == null
                    ? 'Not running: no Station holds it'
                    : 'Runs on Station ${charge.station}',
                style: muted,
              ),
              OoTime(charge.lastWake, prefix: 'Last woke '),
              if (charge.lastTurn case final turn? when turn.isNotEmpty)
                Text(
                  turn,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: muted,
                ),
              Text(
                '${charge.turnsLastHour} turns in the last hour',
                style: muted,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _StateChip extends StatelessWidget {
  const _StateChip(this.state);

  final String state;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final color = state == 'running' ? tokens.success : tokens.muted;
    return Semantics(
      label: 'Run $state',
      child: Chip(
        label: Text(state, style: TextStyle(color: color)),
        visualDensity: VisualDensity.compact,
        side: BorderSide(color: color),
      ),
    );
  }
}

class _ChargeDetail extends StatelessWidget {
  const _ChargeDetail({
    required this.board,
    required this.charge,
    required this.tab,
    required this.connect,
    required this.onTab,
    required this.onClose,
    super.key,
  });

  final MissionBoard board;
  final ChargeSummary charge;
  final String tab;
  final AttachChannel Function() connect;
  final ValueChanged<String> onTab;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final text = Theme.of(context).textTheme;
    return Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (onClose != null)
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton.icon(
                onPressed: onClose,
                icon: const Icon(Icons.arrow_back, size: 18),
                label: const Text('Back to Charges'),
              ),
            ),
          Semantics(
            header: true,
            child: Text('${charge.title}\'s charge', style: text.titleLarge),
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            children: [
              for (final (id, label) in _tabs)
                ChoiceChip(
                  key: ValueKey('charge-tab-$id'),
                  label: Text(label),
                  selected: tab == id,
                  onSelected: (_) => onTab(id),
                ),
            ],
          ),
          const SizedBox(height: 8),
          Expanded(
            child: switch (tab) {
              'decisions' => _Decisions(board: board, charge: charge),
              'logbook' => _Logbook(board: board, charge: charge),
              _ => _Live(board: board, charge: charge, connect: connect),
            },
          ),
        ],
      ),
    );
  }
}

class _Live extends StatelessWidget {
  const _Live({
    required this.board,
    required this.charge,
    required this.connect,
  });

  final MissionBoard board;
  final ChargeSummary charge;
  final AttachChannel Function() connect;

  @override
  Widget build(BuildContext context) {
    final token = board.accessToken;
    if (!charge.canConverse || token == null) {
      return Text(
        'You may see this charge but not attach to it: attaching needs '
        'charge.converse on ${charge.title}.',
        style: TextStyle(color: OoTokens.of(context).muted),
      );
    }
    if (!charge.held) {
      return Text(
        'No Station holds this charge, so its Run is not running. Attaching '
        'is possible once one does.',
        style: TextStyle(color: OoTokens.of(context).muted),
      );
    }
    return AttachView(
      token: token,
      target: ChargeTarget(charge.scope),
      connect: connect,
      canInterrupt: charge.canInterrupt,
      agent: charge.agent,
      me: attachNames(board.userSession),
    );
  }
}

/// A charge's logbook, read apart from the list since it needs `charge.converse`.
class _Logbook extends StatelessWidget {
  const _Logbook({required this.board, required this.charge});

  final MissionBoard board;
  final ChargeSummary charge;

  @override
  Widget build(BuildContext context) => _WithDetail(
    board: board,
    charge: charge,
    builder: (context, detail) {
      if (detail.logbook.isEmpty) {
        return const Text('Its logbook has no entry yet.');
      }
      return ListView(
        children: [
          for (final line in detail.logbook.reversed)
            ListTile(
              dense: true,
              title: Text(line.text),
              subtitle: Row(
                children: [
                  Text(line.kind),
                  const SizedBox(width: 8),
                  OoTime(line.at),
                ],
              ),
            ),
        ],
      );
    },
  );
}

/// What a charge decided: what it said at each wake and, for an Operation's, the Reports it
/// assessed.
class _Decisions extends StatelessWidget {
  const _Decisions({required this.board, required this.charge});

  final MissionBoard board;
  final ChargeSummary charge;

  Future<List<ReportSummary>> _assessed() async {
    if (charge.operationId == null) return const [];
    try {
      final listing = await board.loadReports();
      return [
        for (final report in listing.reports)
          if (report.assessment?.by == 'charge:${charge.scope}') report,
      ];
    } on Object {
      return const [];
    }
  }

  @override
  Widget build(BuildContext context) => _WithDetail(
    board: board,
    charge: charge,
    builder: (context, detail) => FutureBuilder<List<ReportSummary>>(
      future: _assessed(),
      builder: (context, snapshot) {
        final reports = snapshot.data ?? const [];
        final turns = [
          for (final line in detail.logbook.reversed)
            if (line.kind == 'turn') line,
        ];
        if (reports.isEmpty && turns.isEmpty) {
          return const Text('It has made no decision yet.');
        }
        return ListView(
          children: [
            for (final report in reports)
              ListTile(
                dense: true,
                title: Text(report.title ?? report.id),
                subtitle: Text(
                  '${report.assessment!.verdict}'
                  '${report.assessment!.comment == null ? '' : ': ${report.assessment!.comment}'}',
                ),
              ),
            for (final line in turns)
              ListTile(
                dense: true,
                title: Text(line.text),
                subtitle: OoTime(line.at),
              ),
          ],
        );
      },
    ),
  );
}

/// Reads [charge]'s detail (its logbook) and builds from it, or says why it cannot.
class _WithDetail extends StatefulWidget {
  const _WithDetail({
    required this.board,
    required this.charge,
    required this.builder,
  });

  final MissionBoard board;
  final ChargeSummary charge;
  final Widget Function(BuildContext, ChargeSummary) builder;

  @override
  State<_WithDetail> createState() => _WithDetailState();
}

class _WithDetailState extends State<_WithDetail> {
  late final Future<ChargeSummary>? _detail = widget.charge.canConverse
      ? widget.board.loadCharge(widget.charge.scope)
      : null;

  @override
  Widget build(BuildContext context) {
    final detail = _detail;
    if (detail == null) {
      return Text(
        'Its logbook is for those who may converse with it: charge.converse on '
        '${widget.charge.title}.',
        style: TextStyle(color: OoTokens.of(context).muted),
      );
    }
    return FutureBuilder<ChargeSummary>(
      future: detail,
      builder: (context, snapshot) {
        if (snapshot.hasError) return Text('${snapshot.error}');
        final read = snapshot.data;
        if (read == null) {
          return const Center(child: CircularProgressIndicator());
        }
        return widget.builder(context, read);
      },
    );
  }
}
