import 'package:material_ui/material_ui.dart';

import '../controls/mission_actions.dart';
import '../main.dart';
import '../routes/oo_route.dart';
import '../theme/oo_theme.dart';
import '../theme/oo_icon.dart';
import 'missions_board.dart';
import 'oo_freshness.dart';
import 'oo_shell.dart';

/// One Mission, read-only: its state and the Session that sets it, every attempt with its
/// Station and Theater (each a link), the brief and timeline, the actions the person has, and
/// for a queued Mission the placement its Operation would get now.
///
/// Links go to routes; nothing here changes anything but the person's own click on an action.
class MissionDetail extends StatelessWidget {
  const MissionDetail({
    required this.board,
    required this.mission,
    required this.pages,
    required this.onRoute,
    required this.onOpenSession,
    required this.onClose,
    super.key,
  });

  final MissionBoard board;
  final MissionGroup mission;

  /// The pages this person may open; a link goes only to one of these.
  final List<OoPage> pages;
  final ValueChanged<OoRoute> onRoute;
  final ValueChanged<SessionSummary> onOpenSession;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final operationId = mission.operationId;
    final attempts = board.attempts(operationId, mission.missionId);
    final current = board.currentAttempt(operationId, mission.missionId);
    final brief = board.missionBrief(operationId, mission.missionId);
    final latest = mission.latest;
    final held = {for (final attempt in attempts) attempt.station};
    final unattached = [
      for (final session in mission.sessions)
        if (!held.contains(session.stationId)) session,
    ];
    final control = board.controls[(operationId, mission.missionId)];
    return FocusTraversalGroup(
      child: ListView(
        padding: const EdgeInsets.all(24),
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton.icon(
              onPressed: onClose,
              icon: const Icon(Icons.arrow_back, size: 18),
              label: const Text('Back to the queue'),
            ),
          ),
          Row(
            children: [
              IconTheme.merge(
                data: IconThemeData(color: tokens.accent, size: 22),
                child: const OoIcon(OoIconName.missions),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Semantics(
                  header: true,
                  child: Text(
                    '$operationId › ${mission.missionId}',
                    style: text.titleLarge,
                  ),
                ),
              ),
              MissionActionsMenu(
                board: board,
                operationId: operationId,
                missionId: mission.missionId,
                sessions: mission.sessions,
              ),
            ],
          ),
          const SizedBox(height: 10),
          Wrap(
            spacing: 14,
            runSpacing: 4,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  OoStateMark(mission.state, reason: mission.reason),
                  const SizedBox(width: 6),
                  Text(
                    [
                      mission.state,
                      ?mission.reason?.replaceAll('-', ' '),
                    ].join(' · '),
                    style: text.titleSmall,
                  ),
                ],
              ),
              OoTime(mission.updatedAt, prefix: 'Updated '),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            [
              'State from',
              if (current != null) 'attempt ${current.attempt}',
              'Session ${latest.sessionId}',
            ].join(' · '),
            style: text.bodySmall?.copyWith(color: tokens.muted),
          ),
          if (control != null) ...[
            const SizedBox(height: 6),
            Text(control.describe(), style: text.labelMedium),
          ],
          const SizedBox(height: 20),
          Text('ATTEMPTS', style: ooSectionLabel(context)),
          const SizedBox(height: 8),
          if (attempts.isEmpty)
            Text(
              'No attempt is recorded for this Mission.',
              style: TextStyle(color: tokens.muted),
            ),
          for (final attempt in attempts)
            _AttemptCard(
              attempt: attempt,
              sessions: [
                for (final session in mission.sessions)
                  if (session.stationId == attempt.station) session,
              ],
              pages: pages,
              onRoute: onRoute,
              onOpenSession: onOpenSession,
            ),
          if (unattached.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(
              attempts.isEmpty ? 'SESSIONS' : 'OTHER SESSIONS',
              style: ooSectionLabel(context),
            ),
            Wrap(
              spacing: 8,
              children: [
                for (final session in unattached)
                  TextButton(
                    onPressed: () => onOpenSession(session),
                    child: Text('Open Session ${session.sessionId}'),
                  ),
              ],
            ),
          ],
          const SizedBox(height: 16),
          Text('LINKS', style: ooSectionLabel(context)),
          Wrap(
            spacing: 8,
            children: [
              if (brief != null)
                TextButton.icon(
                  onPressed: () => Navigator.of(context).push(
                    MaterialPageRoute<void>(
                      builder: (_) => MissionBriefPage(brief: brief),
                    ),
                  ),
                  icon: const Icon(Icons.description_outlined, size: 18),
                  label: const Text('Mission brief'),
                ),
              if (pages.contains(OoPage.timeline) &&
                  board.timelineOperations.contains(operationId))
                TextButton.icon(
                  onPressed: () => onRoute(
                    OoRoute(OoPage.timeline)
                        .withField('operation_id', operationId)
                        .withField('mission_id', mission.missionId),
                  ),
                  icon: const OoIcon(OoIconName.timeline),
                  label: const Text('Timeline'),
                ),
            ],
          ),
          if (mission.state == 'queued') ...[
            const SizedBox(height: 20),
            WhyItMayWait(board: board, operationId: operationId),
          ],
        ],
      ),
    );
  }
}

class _AttemptCard extends StatelessWidget {
  const _AttemptCard({
    required this.attempt,
    required this.sessions,
    required this.pages,
    required this.onRoute,
    required this.onOpenSession,
  });

  final MissionAttemptSummary attempt;

  /// The Sessions the board holds on this attempt's Station.
  final List<SessionSummary> sessions;
  final List<OoPage> pages;
  final ValueChanged<OoRoute> onRoute;
  final ValueChanged<SessionSummary> onOpenSession;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final stations = OoRoute(OoPage.stations)
        .withField('station_id', attempt.station);
    final canLink = pages.contains(OoPage.stations);
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Attempt ${attempt.attempt} · ${attempt.status}',
              style: text.titleSmall,
            ),
            Wrap(
              spacing: 8,
              children: [
                TextButton(
                  onPressed: canLink ? () => onRoute(stations) : null,
                  child: Text('Station ${attempt.station}'),
                ),
                if (attempt.theater case final theater?)
                  TextButton(
                    onPressed: canLink
                        ? () => onRoute(
                            stations
                                .withField('tab', 'theaters')
                                .withField('theater_id', theater),
                          )
                        : null,
                    child: Text('Theater $theater'),
                  ),
              ],
            ),
            if (attempt.lost case final lost?)
              Text(lost, style: text.bodySmall?.copyWith(color: tokens.muted)),
            if (attempt.checkpointBranch case final branch?)
              Text(
                'Checkpointed to branch $branch',
                style: text.bodySmall?.copyWith(color: tokens.muted),
              ),
            if (sessions.isEmpty)
              Text(
                'No Session of this Mission is recorded on this Station.',
                style: text.bodySmall?.copyWith(color: tokens.muted),
              )
            else
              Wrap(
                spacing: 8,
                children: [
                  for (final session in sessions)
                    TextButton(
                      onPressed: () => onOpenSession(session),
                      child: Text('Open Session ${session.sessionId}'),
                    ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

/// Why a queued Mission may not have started: what the Operation's placement preview rules
/// out now. The preview is per Operation, so it is titled as the Operation's and never
/// claims to be this Mission's; it is left out when the gateway answers that the person may
/// not see it.
class WhyItMayWait extends StatefulWidget {
  const WhyItMayWait({
    required this.board,
    required this.operationId,
    super.key,
  });

  final MissionBoard board;
  final String operationId;

  @override
  State<WhyItMayWait> createState() => _WhyItMayWaitState();
}

class _WhyItMayWaitState extends State<WhyItMayWait> {
  late Future<PlacementPreview> _preview = _load();

  Future<PlacementPreview> _load() =>
      widget.board.loadPlacementPreview(widget.operationId);

  @override
  void didUpdateWidget(WhyItMayWait oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.operationId != widget.operationId) _preview = _load();
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    return FutureBuilder<PlacementPreview>(
      future: _preview,
      builder: (context, snapshot) {
        if (snapshot.hasError) {
          if (snapshot.error is PlacementForbidden) {
            return const SizedBox.shrink();
          }
          return Text(
            'Placement could not be loaded.',
            style: TextStyle(color: tokens.muted),
          );
        }
        final preview = snapshot.data;
        if (preview == null) return const LinearProgressIndicator();
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('WHY IT MAY WAIT', style: ooSectionLabel(context)),
            const SizedBox(height: 4),
            Text(
              'Placement for Operation ${widget.operationId} now',
              style: text.titleSmall,
            ),
            const SizedBox(height: 6),
            if (preview.rejections.isEmpty)
              Text(
                'No Station, Theater or Agent is ruled out for this Operation now.',
                style: TextStyle(color: tokens.muted),
              ),
            for (final rejection in preview.rejections)
              Padding(
                padding: const EdgeInsets.only(bottom: 4),
                child: Text(
                  '${rejection.subject.isEmpty ? 'Any' : rejection.subject}: '
                  '${rejection.reason}',
                  style: text.bodySmall,
                ),
              ),
          ],
        );
      },
    );
  }
}
