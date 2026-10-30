import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_route.dart';
import '../theme/oo_theme.dart';
import 'missions_board.dart';
import 'oo_freshness.dart';
import 'oo_shell.dart';

/// The Stations as cards. With [detailed], the route named one Station: its card is the
/// Station Watch, wider, with the Sessions running there.
class StationOverview extends StatelessWidget {
  const StationOverview({
    required this.board,
    required this.stations,
    required this.empty,
    required this.onCreateMission,
    required this.canCreateMission,
    required this.onRoute,
    this.detailed = false,
    super.key,
  });

  final MissionBoard board;

  /// The Stations to show.
  final StationBoardSummary stations;

  /// What shows when [stations] has none.
  final String empty;
  final ValueChanged<String> onCreateMission;
  final bool Function(String operationId) canCreateMission;
  final ValueChanged<OoRoute> onRoute;
  final bool detailed;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          'STATIONS',
          style: Theme.of(context).textTheme.labelMedium?.copyWith(
            letterSpacing: 2.4,
            color: OoTokens.of(context).accent,
          ),
        ),
        const SizedBox(height: 10),
        if (stations.stations.isEmpty)
          Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Text(empty),
            ),
          )
        else
          Wrap(
            spacing: 12,
            runSpacing: 12,
            children: [
              for (final station in stations.stations)
                SizedBox(
                  width: detailed ? 720 : 300,
                  child: StationCard(
                    station: station,
                    board: board,
                    onCreateMission: onCreateMission,
                    canCreateMission: canCreateMission,
                    onRoute: onRoute,
                    detailed: detailed,
                  ),
                ),
            ],
          ),
      ],
    ),
  );
}

/// The Missions with an active attempt on [stationId]: each with an active attempt there, and the
/// with the Session of it the board holds there, by Operation then Mission.
List<(MissionGroup, SessionSummary?)> sessionsHere(
  MissionBoard board,
  String stationId,
) {
  final here = <(MissionGroup, SessionSummary?)>[];
  for (final mission in groupMissions(board.sessions)) {
    final attempts = board.attempts(mission.operationId, mission.missionId);
    if (!attempts.any(
      (attempt) => attempt.station == stationId && attempt.status == 'active',
    )) {
      continue;
    }
    final onStation = mission.sessions.where(
      (session) => session.stationId == stationId,
    );
    here.add((
      mission,
      onStation.isEmpty
          ? null
          : onStation.reduce(
              (left, right) => right.sequence > left.sequence ? right : left,
            ),
    ));
  }
  return here;
}

/// One Station: whether it is online, when it last checked in, its roles, its load, its
/// resources as last sampled and its Theaters, with its Head of Station's report and a button
/// to start a Mission in each Operation it serves. [detailed] adds the Sessions running there,
/// each a link to its Mission.
class StationCard extends StatelessWidget {
  const StationCard({
    required this.station,
    required this.board,
    required this.onCreateMission,
    required this.canCreateMission,
    required this.onRoute,
    this.detailed = false,
    super.key,
  });

  final StationSummary station;
  final MissionBoard board;
  final ValueChanged<String> onCreateMission;
  final bool Function(String operationId) canCreateMission;
  final ValueChanged<OoRoute> onRoute;
  final bool detailed;

  /// How many seconds the Station has gone without checking in, once that is more than three
  /// times the board's own interval; `null` before then, and always when the board does not
  /// poll.
  int? get _silent {
    final every = board.pollInterval;
    if (every == null || station.seen <= 0) return null;
    final seconds = ooClock()
        .difference(
          DateTime.fromMillisecondsSinceEpoch((station.seen * 1000).round()),
        )
        .inSeconds;
    return seconds > 3 * every.inSeconds ? seconds : null;
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    station.stationId,
                    style: text.titleMedium,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                if (station.headquarters)
                  const Chip(
                    avatar: Icon(Icons.account_balance, size: 15),
                    label: Text('HQS'),
                    visualDensity: VisualDensity.compact,
                  ),
              ],
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                Icon(
                  station.healthy ? Icons.check_circle : Icons.link_off,
                  size: 16,
                  color: station.healthy ? tokens.success : tokens.danger,
                ),
                const SizedBox(width: 6),
                Text(
                  station.healthy ? 'Online' : 'Dark',
                  style: text.labelLarge,
                ),
                if (_silent case final seconds?) ...[
                  const SizedBox(width: 10),
                  Text(
                    'No check-in for $seconds s',
                    style: text.labelMedium?.copyWith(color: tokens.warning),
                  ),
                ],
              ],
            ),
            const SizedBox(height: 4),
            OoTime(
              station.seen > 0 ? station.seen : null,
              prefix: 'Last check-in ',
            ),
            const SizedBox(height: 2),
            Text(
              'Roles: ${station.roles.isEmpty ? 'Not reported' : station.roles.join(', ')}',
              style: text.bodySmall,
            ),
            const SizedBox(height: 2),
            Text(
              'Running ${station.running}/${station.capacity} sessions',
              style: text.bodySmall,
            ),
            const SizedBox(height: 2),
            Text(
              station.resources?.description ?? 'Resources: Not reported',
              style: text.bodySmall,
            ),
            if (detailed)
              _SessionsHere(station: station, board: board, onRoute: onRoute),
            if (station.theaters.isNotEmpty) ...[
              const SizedBox(height: 10),
              Wrap(
                spacing: 6,
                runSpacing: 4,
                children: [
                  for (final theater in station.theaters)
                    Tooltip(
                      message: theater.description,
                      child: Chip(
                        avatar: Icon(
                          theater.unavailable != null
                              ? Icons.link_off
                              : theater.unfit
                              ? Icons.build_outlined
                              : theater.access == 'local'
                              ? Icons.computer
                              : Icons.cloud_outlined,
                          size: 15,
                        ),
                        label: Text('Theater ${theater.id}'),
                        visualDensity: VisualDensity.compact,
                      ),
                    ),
                ],
              ),
            ],
            if (station.headOfStation case final report?) ...[
              const SizedBox(height: 10),
              Text(report.heading, style: text.labelSmall),
              const SizedBox(height: 2),
              Text(
                report.summary,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: text.bodySmall,
              ),
              for (final finding in report.findings)
                Padding(
                  padding: const EdgeInsets.only(top: 3),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Icon(
                        switch (finding.severity) {
                          'critical' => Icons.error_outline,
                          'warning' => Icons.warning_amber,
                          _ => Icons.info_outline,
                        },
                        size: 14,
                        color: switch (finding.severity) {
                          'critical' => tokens.danger,
                          'warning' => tokens.warning,
                          _ => tokens.muted,
                        },
                      ),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          '${finding.kind}: ${finding.text}',
                          style: text.bodySmall,
                        ),
                      ),
                    ],
                  ),
                ),
            ],
            if (station.operations.isNotEmpty) ...[
              const SizedBox(height: 10),
              Wrap(
                spacing: 6,
                runSpacing: 4,
                children: [
                  for (final operation in station.operations)
                    ActionChip(
                      label: Text(operation),
                      avatar: const Icon(Icons.add, size: 16),
                      tooltip: canCreateMission(operation)
                          ? 'Start a Mission in $operation'
                          : 'You do not have mission.create access to $operation',
                      onPressed: canCreateMission(operation)
                          ? () => onCreateMission(operation)
                          : null,
                      visualDensity: VisualDensity.compact,
                    ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// The Missions with an active attempt on a Station, each opening its detail.
class _SessionsHere extends StatelessWidget {
  const _SessionsHere({
    required this.station,
    required this.board,
    required this.onRoute,
  });

  final StationSummary station;
  final MissionBoard board;
  final ValueChanged<OoRoute> onRoute;

  @override
  Widget build(BuildContext context) {
    final here = sessionsHere(board, station.stationId);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 14),
        Text('SESSIONS HERE', style: ooSectionLabel(context)),
        if (here.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              'No active attempt is on this Station.',
              style: TextStyle(color: OoTokens.of(context).muted),
            ),
          ),
        for (final (mission, session) in here)
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton(
              onPressed: () => onRoute(
                OoRoute(OoPage.missions)
                    .withField('operation_id', mission.operationId)
                    .withField('mission_id', mission.missionId),
              ),
              child: Text(
                '${mission.operationId} › ${mission.missionId}'
                '${session == null ? '' : ' · ${session.sessionId}'}',
              ),
            ),
          ),
      ],
    );
  }
}
