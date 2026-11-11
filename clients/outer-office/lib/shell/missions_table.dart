import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../controls/mission_actions.dart';
import '../main.dart';
import '../routes/oo_route.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import '../widgets/oo_data_table.dart';
import 'mission_pending.dart';
import 'missions_board.dart';
import 'oo_freshness.dart';
import 'oo_link.dart';

/// The Missions a Missions route selects, as a table. The page narrows and sorts them by the
/// route (its rules decide what a state, a text or a sort means); the table shows them as given
/// and writes a header click, a search or a filter back to the route.
class MissionsTable extends StatelessWidget {
  const MissionsTable({
    required this.board,
    required this.missions,
    required this.route,
    required this.onRoute,
    required this.onDetails,
    this.open,
    super.key,
  });

  final MissionBoard board;
  final List<MissionGroup> missions;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;
  final ValueChanged<MissionGroup> onDetails;

  /// The Mission whose detail is open.
  final MissionGroup? open;

  String _id(MissionGroup mission) =>
      '${mission.operationId}/${mission.missionId}';

  @override
  Widget build(BuildContext context) {
    final muted = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: OoTokens.of(context).muted);
    return OoDataTable<MissionGroup>(
      rows: missions,
      rowId: _id,
      narrowed: true,
      noun: 'Missions',
      route: route,
      onRoute: onRoute,
      defaultSort: ('updated', false),
      searchText: (mission) => mission.missionId,
      onOpen: onDetails,
      bulkActions: [
        for (final (action, label, icon) in const [
          ('cancel', 'Cancel', Icons.cancel_outlined),
          ('retry', 'Retry', Icons.replay),
          ('resume', 'Resume', Icons.play_arrow_outlined),
        ])
          OoBulkAction<MissionGroup>(
            label: label,
            icon: icon,
            enabled: (rows) => rows.any((row) => _allows(row, action)),
            onSelected: (rows) => _bulk(context, action, label, rows),
          ),
      ],
      columns: [
        OoColumn(
          id: 'mission',
          label: 'Mission',
          flex: 3,
          sortKey: (mission) => missionOrder(mission.missionId),
          cell: (context, mission) => Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              OoLink(
                'Mission ${board.handleOf(mission.operationId, mission.missionId)}',
                route: OoLinkTo.mission(mission.operationId, mission.missionId),
                onTap: () => onDetails(mission),
                style: Theme.of(context).textTheme.titleSmall,
              ),
              if (board
                      .missionBrief(mission.operationId, mission.missionId)
                      ?.name
                  case final name?)
                Text(name, maxLines: 1, overflow: TextOverflow.ellipsis),
            ],
          ),
        ),
        OoColumn(
          id: 'operation',
          label: 'Operation',
          flex: 2,
          sortKey: (mission) => mission.operationId,
          cell: (context, mission) => OoLink(
            mission.operationId,
            route: OoLinkTo.operation(mission.operationId),
          ),
        ),
        OoColumn(
          id: 'state',
          label: 'State',
          flex: 3,
          sortKey: (mission) => boardStates.indexOf(mission.state),
          cell: (context, mission) => Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                children: [
                  OoStateMark(mission.state, reason: mission.reason),
                  const SizedBox(width: 6),
                  Flexible(
                    child: Text(
                      stateLine(mission.state, mission.reason),
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ],
              ),
              if (mission.pending case final pending?)
                MissionPendingNote(pending: pending, compact: true),
            ],
          ),
        ),
        OoColumn(
          id: 'agent',
          label: 'Agent and role',
          flex: 2,
          cell: (context, mission) {
            final brief = board.missionBrief(
              mission.operationId,
              mission.missionId,
            );
            final agent = brief?.agentId ?? mission.latest?.agent;
            return Text(
              [?agent, ?brief?.roleId].join(' · '),
              overflow: TextOverflow.ellipsis,
            );
          },
        ),
        OoColumn(
          id: 'station',
          label: 'Station and Theater',
          flex: 2,
          cell: (context, mission) {
            final attempt = board.currentAttempt(
              mission.operationId,
              mission.missionId,
            );
            final station = attempt?.station ?? mission.latest?.stationId;
            return Text(
              [?station, ?attempt?.theater].join(' / '),
              style: muted,
              overflow: TextOverflow.ellipsis,
            );
          },
        ),
        OoColumn(
          id: 'attempts',
          label: 'Attempts',
          width: 80,
          cell: (context, mission) => Text(
            '${board.attempts(mission.operationId, mission.missionId).length}',
          ),
        ),
        OoColumn(
          id: 'updated',
          label: 'Updated',
          width: 130,
          sortKey: (mission) => mission.updatedAt,
          cell: (context, mission) => mission.updatedAt == null
              ? const Text('-')
              : OoTime(mission.updatedAt!),
        ),
        OoColumn(
          id: 'actions',
          label: 'Actions',
          width: 56,
          cell: (context, mission) => MissionActionsMenu(
            board: board,
            operationId: mission.operationId,
            missionId: mission.missionId,
            sessions: mission.sessions,
            state: mission.state,
          ),
        ),
      ],
    );
  }

  bool _allows(MissionGroup mission, String action) {
    final option = missionControls(
      state: mission.state,
      permitted: board.canControlMission(mission.operationId),
    ).firstWhere((option) => option.action == action);
    return option.enabled &&
        !board.requestInProgress(mission.operationId, mission.missionId);
  }

  /// Asks [action] of each Mission its state allows, after the person confirms the count; the
  /// Stations make the same checks and have the final say.
  Future<void> _bulk(
    BuildContext context,
    String action,
    String label,
    List<MissionGroup> rows,
  ) async {
    final eligible = [
      for (final row in rows)
        if (_allows(row, action)) row,
    ];
    final messenger = ScaffoldMessenger.maybeOf(context);
    if (eligible.isEmpty) {
      messenger?.showSnackBar(
        SnackBar(content: Text('None of the selected Missions can be $label.')),
      );
      return;
    }
    final skipped = rows.length - eligible.length;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text('$label ${eligible.length} Missions?'),
        content: Text(
          skipped == 0
              ? 'Each Mission is asked, and its Station decides.'
              : '$skipped selected Missions cannot be asked and are left out.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext, false),
            child: const Text('Not now'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext, true),
            child: Text(label),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    final said = <String>[];
    for (final mission in eligible) {
      try {
        final status = await board.requestControl(
          operationId: mission.operationId,
          missionId: mission.missionId,
          action: action,
        );
        said.add(status.describe());
      } on StateError catch (error) {
        said.add('${mission.missionId}: ${error.message}');
      }
    }
    messenger?.showSnackBar(SnackBar(content: Text(said.join('\n'))));
  }
}
