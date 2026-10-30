import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_decor.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'oo_freshness.dart';
import 'oo_shell.dart';

/// The Sessions a Sessions [route] selects from the board, in its order: by Operation,
/// Mission and Session unless it sorts, ties kept in that order.
List<SessionSummary> routeSessions(MissionBoard board, OoRoute route) {
  bool matches(SessionSummary session) =>
      (route['operation_id'] == null ||
          session.operationId == route['operation_id']) &&
      (route['state'] == null ||
          (session.state ?? 'working') == route['state']) &&
      (route['station_id'] == null ||
          session.stationId == route['station_id']) &&
      (route['agent'] == null || session.agent == route['agent']);

  int byBoard(SessionSummary left, SessionSummary right) {
    for (final (a, b) in [
      (left.operationId, right.operationId),
      (left.missionId, right.missionId),
      (left.sessionId, right.sessionId),
    ]) {
      final order = a.compareTo(b);
      if (order != 0) return order;
    }
    return 0;
  }

  // The board holds no creation time: a Session's one update time stands in for it.
  final Comparable<Object>? Function(SessionSummary)? key =
      switch (route['sort']) {
        'updated' || 'created' => (session) => session.updatedAt ?? 0,
        'state' => (session) => boardStates.indexOf(session.state ?? 'working'),
        'operation' => (session) => session.operationId,
        'mission' => (session) => session.missionId,
        'session' => (session) => session.sessionId,
        _ => null,
      };
  final sessions = board.sessions.where(matches).toList()
    ..sort((left, right) {
      final order = key == null ? 0 : key(left)!.compareTo(key(right)!);
      return order != 0 ? order : byBoard(left, right);
    });
  return route['order'] == 'desc' ? sessions.reversed.toList() : sessions;
}

/// Every Session on the board, filtered and sorted by the route, in a table; a row opens the
/// Session. The route's `session_id` shows that Session's facts above the table, read-only.
/// On a phone the table is a list of rows, in the same order.
class SessionsPage extends StatelessWidget {
  const SessionsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    required this.onOpen,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;
  final ValueChanged<SessionSummary> onOpen;

  void _sort(String sort, bool ascending) => onRoute(
    route
        .withField('sort', sort)
        .withField('order', ascending ? 'asc' : 'desc'),
  );

  // The width the page has, not the window's: a docked chat takes part of it.
  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) => _page(constraints.maxWidth),
  );

  Widget _page(double width) {
    if (board.sessions.isEmpty) {
      return const Center(
        child: OoBackdrop(
          child: Padding(
            padding: EdgeInsets.all(20),
            child: Text('No Sessions are on the board.'),
          ),
        ),
      );
    }
    final sessions = routeSessions(board, route);
    final selectedId = route['session_id'];
    final selected = selectedId == null
        ? null
        : board.sessions.where((session) => session.sessionId == selectedId);
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
        child: Text('Session $selectedId is not on this board.'),
      ),
      final found => _SessionFacts(session: found.first, onOpen: onOpen),
    };
    final Widget table;
    if (sessions.isEmpty) {
      table = const Padding(
        padding: EdgeInsets.all(24),
        child: Center(child: Text('No Sessions match these filters.')),
      );
    } else if (width < OoBreakpoints.phone) {
      table = _SessionList(sessions: sessions, onOpen: onOpen);
    } else {
      final sort = route['sort'];
      table = Padding(
        padding: const EdgeInsets.all(24),
        child: Card(
          clipBehavior: Clip.antiAlias,
          child: SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: DataTable(
              sortColumnIndex: switch (sort) {
                'operation' => 0,
                'mission' => 1,
                'session' => 2,
                'state' => 3,
                'updated' || 'created' => 4,
                _ => null,
              },
              sortAscending: route['order'] != 'desc',
              showCheckboxColumn: false,
              columns: [
                for (final (label, sort) in const [
                  ('Operation', 'operation'),
                  ('Mission', 'mission'),
                  ('Session', 'session'),
                  ('State', 'state'),
                  ('Updated', 'updated'),
                ])
                  DataColumn(
                    label: Text(label),
                    onSort: (_, ascending) => _sort(sort, ascending),
                  ),
              ],
              rows: [
                for (final session in sessions)
                  DataRow(
                    key: ValueKey(session.sessionId),
                    selected: session.sessionId == selectedId,
                    onSelectChanged: (_) => onOpen(session),
                    cells: [
                      DataCell(Text(session.operationId)),
                      DataCell(Text(session.missionId)),
                      DataCell(
                        Text(
                          session.sessionId,
                          style: const TextStyle(fontFamily: 'monospace'),
                        ),
                      ),
                      DataCell(
                        Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            OoStateMark(
                              session.state ?? 'working',
                              reason: session.reason,
                            ),
                            const SizedBox(width: 8),
                            Text(
                              [
                                session.state ?? 'working',
                                ?session.reason?.replaceAll('-', ' '),
                              ].join(' · '),
                            ),
                          ],
                        ),
                      ),
                      DataCell(OoTime(session.updatedAt, style: null)),
                    ],
                  ),
              ],
            ),
          ),
        ),
      );
    }
    return CustomScrollView(
      slivers: [
        SliverToBoxAdapter(child: controls),
        if (details != null) SliverToBoxAdapter(child: details),
        if (table is _SessionList)
          SliverFillRemaining(child: table)
        else
          SliverToBoxAdapter(child: table),
      ],
    );
  }
}

/// The facts of the Session a route selects; opening it is the person's own click.
class _SessionFacts extends StatelessWidget {
  const _SessionFacts({required this.session, required this.onOpen});

  final SessionSummary session;
  final ValueChanged<SessionSummary> onOpen;

  @override
  Widget build(BuildContext context) {
    final state = session.state ?? 'working';
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 12, 24, 0),
      child: Card(
        child: ListTile(
          leading: const OoTile(OoIconName.sessions, size: 44),
          title: Text('Session ${session.sessionId}'),
          subtitle: Text(
            [
              'Operation ${session.operationId} · Mission ${session.missionId}',
              [state, ?session.reason?.replaceAll('-', ' ')].join(' · '),
              if (session.exposure.isNotEmpty)
                'Has read: ${session.exposure.join(', ')}',
            ].join('\n'),
          ),
          isThreeLine: true,
          trailing: TextButton(
            onPressed: () => onOpen(session),
            child: const Text('Open'),
          ),
        ),
      ),
    );
  }
}

/// The Sessions as rows, like the design reference's phone lists: an icon tile, the
/// Session, its Operation and Mission, and its state beside a dot of its colour.
class _SessionList extends StatelessWidget {
  const _SessionList({required this.sessions, required this.onOpen});

  final List<SessionSummary> sessions;
  final ValueChanged<SessionSummary> onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    return ListView.separated(
      padding: const EdgeInsets.all(12),
      itemCount: sessions.length,
      separatorBuilder: (_, _) => const SizedBox(height: 8),
      itemBuilder: (context, index) {
        final session = sessions[index];
        final state = session.state ?? 'working';
        return Card(
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: () => onOpen(session),
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Row(
                children: [
                  const OoTile(OoIconName.sessions, size: 44),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          session.sessionId,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: text.titleSmall,
                        ),
                        Text(
                          'Operation ${session.operationId} · '
                          'Mission ${session.missionId}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: text.bodySmall?.copyWith(color: tokens.muted),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 8),
                  OoStatusDot(tokens.stateColor(state)),
                  const SizedBox(width: 6),
                  Text(
                    [state, ?session.reason?.replaceAll('-', ' ')].join(' · '),
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
