import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'oo_shell.dart';

/// The command overview: counts of Operations, Stations, active Missions and waiting
/// Sessions, and what needs the person's attention, all from what the board has loaded.
class OverviewPage extends StatelessWidget {
  const OverviewPage({
    required this.board,
    required this.pages,
    required this.onOpen,
    super.key,
  });

  final MissionBoard board;

  /// The pages this person may open; attention items link only to these.
  final List<OoPage> pages;
  final ValueChanged<OoPage> onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final missions = groupMissions(board.sessions);
    final active = missions.where((mission) => !mission.finished).toList();
    final stations = board.stationBoard.stations;
    final online = stations.where((station) => station.healthy).length;
    final waiting = board.sessions
        .where((session) => session.state == 'blocked')
        .toList();
    final operations = board.userSession?.operations ?? const [];
    final busyOperations = active.map((mission) => mission.operationId).toSet();
    final attention = <_Attention>[
      for (final session in waiting)
        session.reason == 'green-ink'
            ? _Attention(
                icon: OoIconName.greenInk,
                color: tokens.warning,
                text:
                    'Session ${session.sessionId} of Mission '
                    '${session.missionId} waits for green ink',
                page: OoPage.greenInk,
              )
            : _Attention(
                icon: OoIconName.interrupt,
                color: tokens.warning,
                text:
                    'Session ${session.sessionId} of Mission '
                    '${session.missionId} is blocked'
                    '${session.reason == null ? '' : ': ${session.reason!.replaceAll('-', ' ')}'}',
                page: OoPage.missions,
              ),
      for (final station in stations.where((station) => !station.healthy))
        _Attention(
          icon: OoIconName.stations,
          color: tokens.danger,
          text: 'Station ${station.stationId} has gone dark',
          page: OoPage.stations,
        ),
    ];
    return ListView(
      padding: const EdgeInsets.only(bottom: 32),
      children: [
        OoBackdrop(
          child: Padding(
            padding: const EdgeInsets.all(20),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'AI6 / UNIVERSAL EXPORTS',
                  style: text.labelMedium?.copyWith(
                    letterSpacing: 2.5,
                    color: tokens.accent,
                  ),
                ),
                const SizedBox(height: 6),
                Text(
                  'Command overview',
                  style: text.headlineMedium?.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  'Operations and field activity across the Stations you can see',
                  style: text.bodyMedium?.copyWith(color: tokens.muted),
                ),
              ],
            ),
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: LayoutBuilder(
            builder: (context, constraints) {
              final columns = constraints.maxWidth >= 760 ? 4 : 2;
              final width =
                  (constraints.maxWidth - 12 * (columns - 1)) / columns;
              return Wrap(
                spacing: 12,
                runSpacing: 12,
                children: [
                  for (final stat in [
                    _Stat(
                      'OPERATIONS',
                      operations.length,
                      '${operations.where((operation) => busyOperations.contains(operation.operationId)).length} with active Missions',
                    ),
                    _Stat(
                      'STATIONS',
                      stations.length,
                      '$online online · ${stations.length - online} dark',
                    ),
                    _Stat(
                      'MISSIONS',
                      active.length,
                      '${missions.length} on the board',
                    ),
                    _Stat(
                      'SESSIONS WAITING',
                      waiting.length,
                      'of ${board.sessions.length} Sessions',
                    ),
                  ])
                    SizedBox(width: width, child: stat),
                ],
              );
            },
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 24, 24, 8),
          child: Text(
            'NEEDS ATTENTION',
            style: text.labelMedium?.copyWith(
              letterSpacing: 2.4,
              color: tokens.accent,
            ),
          ),
        ),
        if (attention.isEmpty)
          const OoBackdrop(
            child: Padding(
              padding: EdgeInsets.all(20),
              child: Text('Nothing needs your attention.'),
            ),
          )
        else
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 24),
            child: Card(
              child: Column(
                children: [
                  for (final item in attention)
                    ListTile(
                      leading: IconTheme.merge(
                        data: IconThemeData(color: item.color),
                        child: OoIcon(item.icon),
                      ),
                      title: Text(item.text),
                      trailing: pages.contains(item.page)
                          ? TextButton(
                              onPressed: () => onOpen(item.page),
                              child: Text('Open ${item.page.label}'),
                            )
                          : null,
                    ),
                ],
              ),
            ),
          ),
      ],
    );
  }
}

class _Attention {
  const _Attention({
    required this.icon,
    required this.color,
    required this.text,
    required this.page,
  });

  final OoIconName icon;
  final Color color;
  final String text;
  final OoPage page;
}

class _Stat extends StatelessWidget {
  const _Stat(this.label, this.count, this.detail);

  final String label;
  final int count;
  final String detail;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(18),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(label, style: text.labelMedium?.copyWith(color: tokens.muted)),
            const SizedBox(height: 8),
            Text(
              '$count',
              style: text.headlineLarge?.copyWith(fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 4),
            Text(detail, style: text.bodySmall?.copyWith(color: tokens.muted)),
          ],
        ),
      ),
    );
  }
}
