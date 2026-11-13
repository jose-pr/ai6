import 'package:material_ui/material_ui.dart';

import '../controls/mission_actions.dart';
import '../main.dart';
import '../routes/oo_route.dart';
import '../theme/oo_theme.dart';
import 'missions_board.dart';
import 'oo_freshness.dart';
import 'oo_link.dart';
import 'oo_shell.dart';

/// The Stations as cards. With [detailed], the route named one Station: its card is the
/// Station Watch, wider, with the Runs running there.
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

  /// A Station's card on a wide page, in logical pixels; narrower pages give it their width.
  static const double cardWidth = 300;
  static const double watchWidth = 720;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 12, 24, 8),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (stations.stations.isEmpty)
          Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Text(empty),
            ),
          )
        else
          LayoutBuilder(
            builder: (context, box) => Wrap(
              spacing: 12,
              runSpacing: 12,
              children: [
                for (final station in stations.stations)
                  SizedBox(
                    width: box.maxWidth < OoBreakpoints.phone
                        ? box.maxWidth
                        : detailed
                        ? watchWidth
                        : cardWidth,
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
          ),
      ],
    ),
  );
}

/// The Missions with an active run on [stationId]: each with an active run there, and the
/// with the Run of it the board holds there, by Operation then Mission.
List<(MissionGroup, RunSummary?)> runsHere(
  MissionBoard board,
  String stationId,
) {
  final here = <(MissionGroup, RunSummary?)>[];
  for (final mission in board.missionGroups()) {
    final attempts = board.attempts(mission.operationId, mission.missionId);
    if (!attempts.any(
      (attempt) => attempt.station == stationId && attempt.status == 'active',
    )) {
      continue;
    }
    final onStation = mission.runs.where(
      (run) => run.stationId == stationId,
    );
    here.add((mission, currentRun(onStation)));
  }
  return here;
}

/// One Station: whether it is online, when it last checked in, its roles, its load, its
/// resources as last sampled and its Theaters, with its Head of Station's report and a button
/// to start a Mission in each Operation it serves. [detailed] adds the Runs running there,
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
  /// times the board's own interval (`[refresh] silent_intervals`); `null` before then, and always when the board does not
  /// poll.
  int? get _silent {
    final every = board.pollInterval;
    if (every == null || station.seen <= 0 || board.demo) return null;
    final seconds = ooClock()
        .difference(
          DateTime.fromMillisecondsSinceEpoch((station.seen * 1000).round()),
        )
        .inSeconds;
    return seconds > board.limits.refresh.silentIntervals * every.inSeconds
        ? seconds
        : null;
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
                  child: Align(
                    alignment: Alignment.centerLeft,
                    child: OoLink(
                      station.stationId,
                      route: OoLinkTo.station(station.stationId),
                      semanticLabel: 'Open Station ${station.stationId}',
                      maxLines: 1,
                      style: text.titleMedium,
                    ),
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
            _StationStatus(station: station),
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
              'Running ${station.running} of ${station.capacity} Runs',
              style: text.bodySmall,
            ),
            const SizedBox(height: 2),
            Text(
              station.resources?.description ?? 'Resources: Not reported',
              style: text.bodySmall,
            ),
            if (detailed)
              _RunsHere(station: station, board: board, onRoute: onRoute),
            if (station.theaters.isNotEmpty) ...[
              const SizedBox(height: 10),
              Wrap(
                spacing: 6,
                runSpacing: 4,
                children: [
                  for (final theater in station.theaters)
                    Tooltip(
                      message: theater.description,
                      child: ActionChip(
                        onPressed: OoLinks.opener(
                          context,
                          OoLinkTo.theater(station.stationId, theater.id),
                        ),
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
                  for (final operation in station.operations) ...[
                    ActionChip(
                      label: Text('Operation $operation'),
                      tooltip: 'Open Operation $operation',
                      onPressed: OoLinks.opener(
                        context,
                        OoLinkTo.operation(operation),
                      ),
                      visualDensity: VisualDensity.compact,
                    ),
                    if (canCreateMission(operation))
                      ActionChip(
                        label: Text('New Mission in $operation'),
                        avatar: const Icon(Icons.add, size: 16),
                        tooltip: 'Start a Mission in $operation',
                        onPressed: () => onCreateMission(operation),
                        visualDensity: VisualDensity.compact,
                      ),
                  ],
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// What a Station says about its configuration and its posture: the roster revision it runs, a
/// revision it refused (with the reason), files that wait for a restart, and the Theaters that
/// report a setting looser than its default.
class _StationStatus extends StatelessWidget {
  const _StationStatus({required this.station});

  final StationSummary station;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final small = Theme.of(context).textTheme.bodySmall;
    final config = station.config;
    final weakened = [
      for (final theater in station.theaters)
        for (final setting in theater.weakened) '${theater.id}: $setting',
    ];
    final refused = config?.refused ?? const <String>[];
    final restart = config?.needsRestart ?? const <String>[];
    if (config == null && weakened.isEmpty) return const SizedBox.shrink();
    Widget chip(
      Key key,
      IconData icon,
      String label,
      Color color,
      String tip,
    ) => Tooltip(
      message: tip,
      child: Chip(
        key: key,
        avatar: Icon(icon, size: 15, color: color),
        label: Text(label),
        visualDensity: VisualDensity.compact,
        side: BorderSide(color: color),
      ),
    );
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            spacing: 6,
            runSpacing: 4,
            children: [
              if (refused.isNotEmpty)
                chip(
                  Key('station-config-refused-${station.stationId}'),
                  Icons.block,
                  'Config refused',
                  tokens.danger,
                  refused.join('\n'),
                )
              else if (restart.isNotEmpty)
                chip(
                  Key('station-config-restart-${station.stationId}'),
                  Icons.restart_alt,
                  'Restart needed',
                  tokens.warning,
                  restart.join('\n'),
                )
              else if (config?.revision case final revision?)
                chip(
                  Key('station-config-${station.stationId}'),
                  Icons.rule,
                  'Config rev $revision',
                  tokens.muted,
                  'The roster revision this Station runs.',
                ),
              if (weakened.isNotEmpty)
                chip(
                  Key('station-posture-${station.stationId}'),
                  Icons.warning_amber,
                  'Weakened (${weakened.length})',
                  tokens.warning,
                  weakened.join('\n'),
                ),
            ],
          ),
          for (final reason in refused)
            Text(
              'Refused: $reason',
              key: Key('station-refused-${station.stationId}'),
              style: small?.copyWith(color: tokens.danger),
            ),
          for (final file in restart)
            Text(
              '$file changed; restart to apply',
              style: small?.copyWith(color: tokens.warning),
            ),
          for (final line in weakened)
            Text(
              'Weakened: $line',
              style: small?.copyWith(color: tokens.warning),
            ),
        ],
      ),
    );
  }
}

/// The Missions with an active run on a Station, each opening its detail.
class _RunsHere extends StatelessWidget {
  const _RunsHere({
    required this.station,
    required this.board,
    required this.onRoute,
  });

  final StationSummary station;
  final MissionBoard board;
  final ValueChanged<OoRoute> onRoute;

  @override
  Widget build(BuildContext context) {
    final here = runsHere(board, station.stationId);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 14),
        Text('RUNS HERE', style: ooSectionLabel(context)),
        if (here.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              'No active attempt is on this Station.',
              style: TextStyle(color: OoTokens.of(context).muted),
            ),
          ),
        for (final (mission, run) in here)
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton(
              onPressed: () => onRoute(
                OoRoute(OoPage.missions)
                    .withField('operation_id', mission.operationId)
                    .withField('mission_id', mission.missionId),
              ),
              child: Text(
                '${mission.operationId} › '
                '${board.handleOf(mission.operationId, mission.missionId)}'
                '${run == null ? '' : ' · ${run.runId}'}',
              ),
            ),
          ),
      ],
    );
  }
}
