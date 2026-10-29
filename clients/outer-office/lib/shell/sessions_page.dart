import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_icon.dart';

/// Every Session on the board in a table sorted by any column; a row opens the Session.
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
