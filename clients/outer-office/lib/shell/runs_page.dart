import 'operation_names.dart';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../theme/oo_decor.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import '../widgets/oo_data_table.dart';
import 'oo_empty.dart';
import 'oo_freshness.dart';
import 'oo_link.dart';
import 'oo_shell.dart';

/// The Runs a Runs [route] selects from the board, in its order: by Operation,
/// Mission and Run unless it sorts, ties kept in that order.
List<RunSummary> routeRuns(MissionBoard board, OoRoute route) {
  final text = route['text']?.toLowerCase();
  bool matches(RunSummary run) =>
      (route['operation_id'] == null ||
          run.operationId == route['operation_id']) &&
      (route['state'] == null || (run.state ?? 'working') == route['state']) &&
      (route['station_id'] == null || run.stationId == route['station_id']) &&
      (route['agent'] == null || run.agent == route['agent']) &&
      (text == null ||
          [
            run.operationId,
            run.missionId,
            board.handleOf(run.operationId, run.missionId),
            run.runId,
            ?run.stationId,
            ?run.agent,
          ].any((item) => item.toLowerCase().contains(text)));

  int byBoard(RunSummary left, RunSummary right) {
    for (final (a, b) in [
      (left.operationId, right.operationId),
      (left.missionId, right.missionId),
      (left.runId, right.runId),
    ]) {
      final order = a.compareTo(b);
      if (order != 0) return order;
    }
    return 0;
  }

  // The board holds no creation time: a Run's one update time stands in for it.
  final Comparable<Object>? Function(RunSummary)? key = switch (route['sort']) {
    'updated' || 'created' => (run) => run.updatedAt ?? 0,
    'state' => (run) => boardStates.indexOf(run.state ?? 'working'),
    'operation' => (run) => run.operationId,
    'mission' => (run) => run.missionId,
    'run' => (run) => run.runId,
    _ => null,
  };
  final runs = board.runs.where(matches).toList()
    ..sort((left, right) {
      final order = key == null ? 0 : key(left)!.compareTo(key(right)!);
      return order != 0 ? order : byBoard(left, right);
    });
  return route['order'] == 'desc' ? runs.reversed.toList() : runs;
}

/// Every Run on the board, filtered and sorted by the route, in a table; a row opens the
/// Run. The route's `run_id` shows that Run's facts above the table, read-only.
/// On a phone the table is a list of rows, in the same order.
class RunsPage extends StatelessWidget {
  const RunsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    required this.onOpen,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;
  final ValueChanged<RunSummary> onOpen;

  // The width the page has, not the window's: a docked chat takes part of it.
  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) => _page(constraints.maxWidth),
  );

  Widget _page(double width) {
    if (board.runs.isEmpty) {
      if (boardProblem(board, BoardSource.board) case final problem?) {
        return problem;
      }
      if (board.loading) {
        return const Center(child: CircularProgressIndicator());
      }
      return const OoEmptyState(
        icon: OoIconName.runs,
        backdrop: true,
        title: 'No Runs yet',
        body: 'A Run is one run of an Agent on a Mission. They show here once a Mission starts.',
        link: ('Go to Missions', OoRoute(OoPage.missions)),
      );
    }
    final runs = routeRuns(board, route);
    final selectedId = route['run_id'];
    final selected = selectedId == null
        ? null
        : board.runs.where((run) => run.runId == selectedId);
    final controls = Padding(
      padding: const EdgeInsets.fromLTRB(24, 12, 12, 0),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: OoFilterBar(
              route: route,
              onRoute: onRoute,
              padding: const EdgeInsets.only(top: 4),
            ),
          ),
          OoSortMenu(route: route, onRoute: onRoute),
        ],
      ),
    );
    final details = switch (selected) {
      null => null,
      final found when found.isEmpty => Padding(
        padding: const EdgeInsets.fromLTRB(24, 12, 24, 0),
        child: Text('Run $selectedId is not on this board.'),
      ),
      final found => _RunFacts(
        run: found.first,
        handle: board.handleOf(found.first.operationId, found.first.missionId),
        onOpen: onOpen,
      ),
    };
    final Widget table;
    if (runs.isEmpty) {
      table = const Padding(
        padding: EdgeInsets.all(24),
        child: Center(child: Text('No Runs match these filters.')),
      );
    } else if (width < OoBreakpoints.phone) {
      table = _RunList(
        runs: runs,
        handleOf: (run) => board.handleOf(run.operationId, run.missionId),
        onOpen: onOpen,
      );
    } else {
      table = OoDataTable<RunSummary>(
        rows: runs,
        rowId: (run) => run.runId,
        narrowed: true,
        noun: 'Runs',
        route: route,
        onRoute: onRoute,
        onOpen: onOpen,
        searchText: (run) => run.runId,
        defaultSort: ('operation', true),
        cardsBelow: OoBreakpoints.phone,
        columns: [
          OoColumn(
            id: 'operation',
            label: 'Operation',
            sortKey: (run) => run.operationId,
            cell: (context, run) => OoLink(
              run.operationId,
              route: OoLinkTo.operation(run.operationId),
              semanticLabel:
                  'Open Operation ${OperationNames.of(run.operationId)}',
            ),
          ),
          OoColumn(
            id: 'mission',
            label: 'Mission',
            sortKey: (run) => run.missionId,
            cell: (context, run) => OoLink(
              board.handleOf(run.operationId, run.missionId),
              route: OoLinkTo.mission(run.operationId, run.missionId),
              semanticLabel:
                  'Open Mission '
                  '${board.handleOf(run.operationId, run.missionId)}',
            ),
          ),
          OoColumn(
            id: 'run',
            label: 'Run',
            sortKey: (run) => run.runId,
            cell: (context, run) => Text(
              run.runId,
              style: const TextStyle(fontFamily: 'monospace'),
            ),
          ),
          OoColumn(
            id: 'state',
            label: 'State',
            flex: 2,
            sortKey: (run) => boardStates.indexOf(run.state ?? 'working'),
            cell: (context, run) => Row(
              children: [
                OoStateMark(run.state ?? 'working', reason: run.reason),
                const SizedBox(width: 8),
                Flexible(
                  child: Text(
                    [
                      run.state ?? 'working',
                      if (run.reason case final reason?) reasonWords(reason),
                    ].join(' · '),
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
          ),
          OoColumn(
            id: 'updated',
            label: 'Updated',
            sortKey: (run) => run.updatedAt ?? 0,
            cell: (context, run) => OoTime(run.updatedAt, style: null),
          ),
          OoColumn(
            id: 'station',
            label: 'Station',
            cell: (context, run) => run.stationId == null
                ? const Text('—')
                : OoLink(
                    run.stationId!,
                    route: OoLinkTo.station(run.stationId!),
                    semanticLabel: 'Open Station ${run.stationId}',
                  ),
          ),
          OoColumn(
            id: 'agent',
            label: 'Agent',
            cell: (context, run) => run.agent == null
                ? const Text('—')
                : OoLink(
                    run.agent!,
                    route: OoLinkTo.agent(
                      run.agent!,
                      operation: run.operationId,
                    ),
                    semanticLabel: 'Open Agent ${run.agent}',
                  ),
          ),
        ],
      );
    }
    return CustomScrollView(
      slivers: [
        SliverToBoxAdapter(child: controls),
        if (details != null) SliverToBoxAdapter(child: details),
        if (table is _RunList || table is OoDataTable<RunSummary>)
          SliverFillRemaining(child: table)
        else
          SliverToBoxAdapter(child: table),
      ],
    );
  }
}

/// The facts of the Run a route selects; opening it is the person's own click.
class _RunFacts extends StatelessWidget {
  const _RunFacts({
    required this.run,
    required this.handle,
    required this.onOpen,
  });

  final RunSummary run;

  /// What its Mission is called.
  final String handle;
  final ValueChanged<RunSummary> onOpen;

  @override
  Widget build(BuildContext context) {
    final state = run.state ?? 'working';
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 12, 24, 0),
      child: Card(
        child: ListTile(
          leading: const OoTile(OoIconName.runs, size: 44),
          title: Text('Run ${run.runId}'),
          subtitle: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              OoLinkLine([
                (
                  'Operation ${OperationNames.of(run.operationId)}',
                  OoLinkTo.operation(run.operationId),
                ),
                (
                  'Mission $handle',
                  OoLinkTo.mission(run.operationId, run.missionId),
                ),
              ]),
              Text(
                [
                  state,
                  if (run.reason case final reason?) reasonWords(reason),
                ].join(' · '),
              ),
              if (run.exposure.isNotEmpty)
                Text('Has read: ${run.exposure.join(', ')}'),
            ],
          ),
          isThreeLine: true,
          trailing: TextButton(
            onPressed: () => onOpen(run),
            child: const Text('Open'),
          ),
        ),
      ),
    );
  }
}

/// The Runs as rows, like the design reference's phone lists: an icon tile, the
/// Run, its Operation and Mission, and its state beside a dot of its colour.
class _RunList extends StatelessWidget {
  const _RunList({
    required this.runs,
    required this.handleOf,
    required this.onOpen,
  });

  final List<RunSummary> runs;

  /// What a Run's Mission is called.
  final String Function(RunSummary run) handleOf;
  final ValueChanged<RunSummary> onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    return ListView.separated(
      padding: const EdgeInsets.all(12),
      itemCount: runs.length,
      separatorBuilder: (_, _) => const SizedBox(height: 8),
      itemBuilder: (context, index) {
        final run = runs[index];
        final state = run.state ?? 'working';
        return Card(
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: () => onOpen(run),
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Row(
                children: [
                  const OoTile(OoIconName.runs, size: 44),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          run.runId,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: text.titleSmall,
                        ),
                        OoLinkLine([
                          (
                            'Operation ${OperationNames.of(run.operationId)}',
                            OoLinkTo.operation(run.operationId),
                          ),
                          (
                            'Mission ${handleOf(run)}',
                            OoLinkTo.mission(run.operationId, run.missionId),
                          ),
                        ]),
                      ],
                    ),
                  ),
                  const SizedBox(width: 8),
                  OoStatusDot(tokens.stateColor(state)),
                  const SizedBox(width: 6),
                  Text(
                    [
                      state,
                      if (run.reason case final reason?) reasonWords(reason),
                    ].join(' · '),
                    style: text.labelMedium,
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}
