import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_decor.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'oo_shell.dart';

/// Every Session on the board in a table sorted by any column; a row opens the Session. On
/// a phone the table is a list of rows, in the same order.
class SessionsPage extends StatefulWidget {
  const SessionsPage({required this.board, required this.onOpen, super.key});

  final MissionBoard board;
  final ValueChanged<SessionSummary> onOpen;

  @override
  State<SessionsPage> createState() => _SessionsPageState();
}

class _SessionsPageState extends State<SessionsPage> {
  int _column = 0;
  bool _ascending = true;

  /// Orders by the sorted column, then by Session ID so equal rows keep one order.
  int _compare(SessionSummary left, SessionSummary right) {
    final order = switch (_column) {
      0 => left.operationId.compareTo(right.operationId),
      1 => left.missionId.compareTo(right.missionId),
      2 => left.sessionId.compareTo(right.sessionId),
      3 => (left.state ?? 'working').compareTo(right.state ?? 'working'),
      _ => left.sequence.compareTo(right.sequence),
    };
    return order != 0 ? order : left.sessionId.compareTo(right.sessionId);
  }

  void _sort(int column, bool ascending) => setState(() {
    _column = column;
    _ascending = ascending;
  });

  @override
  Widget build(BuildContext context) {
    final sessions = [...widget.board.sessions];
    if (sessions.isEmpty) {
      return const Center(
        child: OoBackdrop(
          child: Padding(
            padding: EdgeInsets.all(20),
            child: Text('No Sessions are on the board.'),
          ),
        ),
      );
    }
    sessions.sort(
      (left, right) =>
          _ascending ? _compare(left, right) : _compare(right, left),
    );
    // The width the page has, not the window's: a docked chat takes part of it.
    return LayoutBuilder(
      builder: (context, constraints) =>
          constraints.maxWidth < OoBreakpoints.phone
          ? _SessionList(sessions: sessions, onOpen: widget.onOpen)
          : _table(sessions),
    );
  }

  Widget _table(List<SessionSummary> sessions) {
    DataColumn column(String label, {bool numeric = false}) =>
        DataColumn(label: Text(label), numeric: numeric, onSort: _sort);
    return SingleChildScrollView(
      padding: const EdgeInsets.all(24),
      child: Card(
        clipBehavior: Clip.antiAlias,
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: DataTable(
            sortColumnIndex: _column,
            sortAscending: _ascending,
            showCheckboxColumn: false,
            columns: [
              column('Operation'),
              column('Mission'),
              column('Session'),
              column('State'),
              column('Event', numeric: true),
            ],
            rows: [
              for (final session in sessions)
                DataRow(
                  key: ValueKey(session.sessionId),
                  onSelectChanged: (_) => widget.onOpen(session),
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
                    DataCell(Text('${session.sequence}')),
                  ],
                ),
            ],
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
