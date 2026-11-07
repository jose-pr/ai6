import 'package:material_ui/material_ui.dart';

import '../assistant/pages.dart' show launchPage;
import '../assistant/transcripts.dart' show SessionContextPanel;
import '../controls/mission_actions.dart';
import '../main.dart';
import '../operations/operation_tree.dart';
import '../routes/oo_route.dart';
import 'oo_shell.dart' show OoPage;
import '../terminal/session_connection.dart';
import '../terminal/session_terminal.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'app_window.dart';
import 'oo_empty.dart';
import 'oo_freshness.dart';
import 'oo_link.dart';

/// The Missions a Missions [route] selects from the board, in its order. A state filter looks at
/// each Mission's state, that of its latest Session, as Overview and an Operation's counts do; a
/// Station or Agent filter keeps the Missions with a Session on that Station or for that Agent.
List<MissionGroup> routeMissions(MissionBoard board, OoRoute route) {
  final states = route.states;
  final text = route['text']?.toLowerCase();
  // With `subtree`, the Operation and those under it as the person's tree shows them.
  final subtree = switch (route['operation_id']) {
    final operation? when route['subtree'] == 'true' => subtreeIds({
      for (final access in board.userSession?.operations ?? const [])
        access.operationId: access.parent,
    }, operation),
    _ => null,
  };
  bool matches(MissionGroup mission) {
    if (states.isNotEmpty && !states.contains(mission.state)) return false;
    if (route['operation_id'] case final operation?
        when subtree != null
            ? !subtree.contains(mission.operationId)
            : mission.operationId != operation) {
      return false;
    }
    if (route['mission_id'] case final id?
        when !board.names(mission.operationId, mission.missionId, id)) {
      return false;
    }
    if (route['station_id'] case final station?
        when !mission.sessions.any((session) => session.stationId == station)) {
      return false;
    }
    if (route['agent'] case final agent?
        when !mission.sessions.any((session) => session.agent == agent)) {
      return false;
    }
    if (text != null) {
      final brief = board.missionBrief(mission.operationId, mission.missionId);
      final haystack = [
        mission.operationId,
        mission.missionId,
        board.handleOf(mission.operationId, mission.missionId),
        for (final session in mission.sessions) session.sessionId,
        for (final attempt in board.attempts(
          mission.operationId,
          mission.missionId,
        )) ...[?attempt.runId, ?attempt.handle],
        ?brief?.name,
        ?brief?.prompt,
      ];
      if (!haystack.any((item) => item.toLowerCase().contains(text))) {
        return false;
      }
    }
    return true;
  }

  double updated(MissionGroup mission) => mission.sessions
      .map((session) => session.updatedAt ?? 0)
      .reduce((a, b) => a > b ? a : b);
  // The board holds no creation time: the earliest update it holds stands in for one.
  double created(MissionGroup mission) => mission.sessions
      .map((session) => session.updatedAt ?? 0)
      .reduce((a, b) => a < b ? a : b);
  final sort = route['sort'] ?? 'updated';
  final Comparable<Object> Function(MissionGroup) key = switch (sort) {
    'created' => created,
    'state' => (mission) => boardStates.indexOf(mission.state),
    'operation' => (mission) => mission.operationId,
    'mission' => (mission) => missionOrder(mission.missionId),
    _ => updated,
  };
  // By update time the newest comes first unless the route says `asc`; otherwise `desc`
  // reverses. Ties keep the board's order (Operation, then Mission ID) either way, so rows
  // never swap between refreshes.
  final descending = sort == 'updated'
      ? route['order'] != 'asc'
      : route['order'] == 'desc';
  final missions = groupMissions(board.sessions).where(matches).indexed.toList()
    ..sort((a, b) {
      final order = key(a.$2).compareTo(key(b.$2));
      return order != 0 ? (descending ? -order : order) : a.$1.compareTo(b.$1);
    });
  return [for (final (_, mission) in missions) mission];
}

class BoardHeader extends StatelessWidget {
  const BoardHeader({
    required this.missions,
    required this.selectedStates,
    required this.onSelectStates,
    required this.sort,
    super.key,
  });

  final List<MissionGroup> missions;

  /// The states shown; none means every state.
  final List<String> selectedStates;
  final ValueChanged<List<String>> onSelectStates;

  /// The sort control beside the heading.
  final Widget sort;

  @override
  Widget build(BuildContext context) {
    final counts = <String, int>{};
    for (final mission in missions) {
      counts.update(mission.state, (value) => value + 1, ifAbsent: () => 1);
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 12, 24, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    _CountChip(
                      label: 'All',
                      count: missions.length,
                      selected: selectedStates.isEmpty,
                      onPressed: () => onSelectStates(const []),
                    ),
                    for (final state in boardStates)
                      _CountChip(
                        label: state == 'verifying'
                            ? 'verifying (waiting for a decision)'
                            : state,
                        count: counts[state] ?? 0,
                        state: state,
                        selected: selectedStates.contains(state),
                        // One state at a time from the chips; a link may name several.
                        onPressed: () => onSelectStates(
                          selectedStates.length == 1 &&
                                  selectedStates.single == state
                              ? const []
                              : [state],
                        ),
                      ),
                  ],
                ),
              ),
              sort,
            ],
          ),
        ],
      ),
    );
  }
}

class _CountChip extends StatelessWidget {
  const _CountChip({
    required this.label,
    required this.count,
    required this.selected,
    required this.onPressed,
    this.state,
  });

  final String label;
  final int count;

  /// The state this chip filters by; none for all Missions.
  final String? state;
  final bool selected;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) => ChoiceChip(
    avatar: switch (state) {
      final state? => OoStateMark(state),
      null => null,
    },
    label: Text('$label  $count'),
    selected: selected,
    onSelected: (_) => onPressed(),
    visualDensity: VisualDensity.compact,
    tooltip: switch (state) {
      null => 'Show every Mission',
      'verifying' => 'Missions waiting for a decision',
      _ => 'Missions that are $label',
    },
  );
}

class EmptyFilter extends StatelessWidget {
  const EmptyFilter({required this.text, super.key});

  final String text;

  @override
  Widget build(BuildContext context) => Center(child: Text(text));
}

/// One Mission on the board and its Sessions.
class MissionGroup {
  const MissionGroup({
    required this.operationId,
    required this.missionId,
    required this.sessions,
  });

  final String operationId;
  final String missionId;
  final List<SessionSummary> sessions;

  /// The Session that sets the Mission's state, by [currentSession]'s rule. A group always has
  /// a Session.
  SessionSummary get latest => currentSession(sessions)!;

  /// The state of its latest Session; a Session not yet reporting one is working.
  String get state => latest.state ?? 'working';

  /// Why the latest Session holds that state, when it says.
  String? get reason => latest.reason;

  /// When the board last heard of any of its Sessions, as Unix seconds; `null` before it
  /// has heard of one.
  double? get updatedAt => sessions
      .map((session) => session.updatedAt)
      .whereType<double>()
      .fold<double?>(
        null,
        (latest, at) => latest == null || at > latest ? at : latest,
      );

  /// Whether it has stopped for good: done, failed or abandoned.
  bool get finished => const {'done', 'failed', 'abandoned'}.contains(state);
}

/// A Mission ID as it sorts for a person: `ai6-9` before `ai6-10`. Only an order; nothing is
/// read out of the ID.
String missionOrder(String id) =>
    id.replaceAllMapped(RegExp(r'\d+'), (run) => run[0]!.padLeft(20, '0'));

/// The board's Sessions grouped by Mission, by Operation then Mission ID.
List<MissionGroup> groupMissions(List<SessionSummary> sessions) {
  final groups = <(String, String), List<SessionSummary>>{};
  for (final session in sessions) {
    groups
        .putIfAbsent((session.operationId, session.missionId), () => [])
        .add(session);
  }
  final result = groups.entries
      .map(
        (entry) => MissionGroup(
          operationId: entry.key.$1,
          missionId: entry.key.$2,
          sessions: List.unmodifiable(entry.value),
        ),
      )
      .toList();
  result.sort((left, right) {
    final station = left.operationId.compareTo(right.operationId);
    return station != 0
        ? station
        : missionOrder(left.missionId).compareTo(missionOrder(right.missionId));
  });
  return result;
}

class MissionCard extends StatelessWidget {
  const MissionCard({
    required this.mission,
    required this.board,
    required this.desktopWindows,
    required this.onSelectSession,
    this.onDetails,
    this.detailsFocus,
    this.selected = false,
    super.key,
  });

  final MissionGroup mission;
  final MissionBoard board;
  final bool desktopWindows;
  final ValueChanged<SessionSummary>? onSelectSession;

  /// Opens the Mission's detail; the card has no such button without it.
  final VoidCallback? onDetails;

  /// The button's focus, so closing the detail can return focus to it.
  final FocusNode? detailsFocus;

  /// Whether the Mission's detail is open.
  final bool selected;

  @override
  Widget build(BuildContext context) => Card(
    shape: selected
        ? RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
            side: BorderSide(color: OoTokens.of(context).accent, width: 1.5),
          )
        : null,
    child: Padding(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              IconTheme.merge(
                data: IconThemeData(
                  color: OoTokens.of(context).accent,
                  size: 19,
                ),
                child: const OoIcon(OoIconName.missions),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: OoLink(
                    'Mission ${board.handleOf(mission.operationId, mission.missionId)}',
                    route: OoLinkTo.mission(
                      mission.operationId,
                      mission.missionId,
                    ),
                    onTap: onDetails,
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                ),
              ),
              Text(
                '${mission.sessions.length} session${mission.sessions.length == 1 ? '' : 's'}',
                style: Theme.of(context).textTheme.labelMedium,
              ),
              if (board.missionBrief(mission.operationId, mission.missionId)
                  case final brief?)
                IconButton(
                  tooltip: 'View Mission brief and launch context',
                  onPressed: () => Navigator.of(context).push(
                    MaterialPageRoute<void>(
                      builder: (_) => MissionBriefPage(brief: brief),
                    ),
                  ),
                  icon: const Icon(Icons.description_outlined),
                ),
              MissionActionsMenu(
                board: board,
                operationId: mission.operationId,
                missionId: mission.missionId,
                sessions: mission.sessions,
              ),
            ],
          ),
          if (board.missionBrief(mission.operationId, mission.missionId)
              case final brief?)
            MissionNameLine(brief: brief),
          Padding(
            padding: const EdgeInsets.only(left: 27, top: 4, bottom: 8),
            child: OoLink(
              'Operation ${mission.operationId}',
              route: OoLinkTo.operation(mission.operationId),
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ),
          Padding(
            padding: const EdgeInsets.only(left: 27, bottom: 12),
            child: Wrap(
              alignment: WrapAlignment.spaceBetween,
              crossAxisAlignment: WrapCrossAlignment.center,
              spacing: 12,
              children: [
                MissionFacts(mission: mission, board: board),
                if (onDetails case final onDetails?)
                  TextButton(
                    focusNode: detailsFocus,
                    onPressed: onDetails,
                    child: const Text('Mission details'),
                  ),
              ],
            ),
          ),
          if (board.attempts(mission.operationId, mission.missionId)
              case final attempts
              when attempts.length > 1 ||
                  attempts.any((attempt) => attempt.status == 'lost'))
            _AttemptHistory(attempts: attempts),
          if (board.controls[(mission.operationId, mission.missionId)]
              case final control?)
            Padding(
              padding: const EdgeInsets.only(left: 27, bottom: 12),
              child: Text(
                control.describe(),
                style: Theme.of(context).textTheme.labelMedium,
              ),
            ),
          for (final session in mission.sessions)
            _SessionRow(
              session: session,
              board: board,
              desktopWindows: desktopWindows,
              onSelectSession: onSelectSession,
            ),
        ],
      ),
    ),
  );
}

/// The name beside a Mission's ID, and a link to the issue it came from when there is one.
///
/// A name can be an issue's title, written by whoever can open an issue: it is plain text here,
/// cut to two lines, and only a web link from the tracker opens anything.
class MissionNameLine extends StatelessWidget {
  const MissionNameLine({required this.brief, super.key});

  final MissionBriefSummary brief;

  @override
  Widget build(BuildContext context) {
    final name = brief.name;
    final url = switch (brief.origin?.itemUrl) {
      final String link => Uri.tryParse(link),
      null => null,
    };
    final web = url != null && const {'http', 'https'}.contains(url.scheme);
    if (name == null && !web) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(left: 27, top: 2),
      child: Wrap(
        spacing: 8,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          if (name != null)
            Text(
              name,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.bodyMedium,
            ),
          if (web)
            TextButton.icon(
              onPressed: () => launchPage(url),
              icon: const Icon(Icons.open_in_new, size: 16),
              label: Text(brief.sourceIssue ?? 'Open the issue'),
            ),
        ],
      ),
    );
  }
}

/// What the queue says of a Mission in a line: its state as a word with its icon and the
/// reason, the Station and Theater of its current attempt, when the board last heard of it,
/// and the Agent and role its brief names.
class MissionFacts extends StatelessWidget {
  const MissionFacts({required this.mission, required this.board, super.key});

  final MissionGroup mission;
  final MissionBoard board;

  @override
  Widget build(BuildContext context) {
    final muted = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: OoTokens.of(context).muted);
    final attempt = board.currentAttempt(
      mission.operationId,
      mission.missionId,
    );
    final brief = board.missionBrief(mission.operationId, mission.missionId);
    final station = attempt?.station ?? mission.latest.stationId;
    final agent = brief?.agentId ?? mission.latest.agent;
    final role = brief?.roleId;
    return Wrap(
      spacing: 14,
      runSpacing: 4,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            OoStateMark(mission.state, reason: mission.reason),
            const SizedBox(width: 6),
            Flexible(
              child: Text(
                stateLine(mission.state, mission.reason),
                style: Theme.of(context).textTheme.labelMedium,
              ),
            ),
          ],
        ),
        if (station != null)
          OoLinkLine([
            ('Station $station', OoLinkTo.station(station)),
            if (attempt?.theater case final theater?)
              ('Theater $theater', OoLinkTo.theater(station, theater)),
            if (attempt?.route case final route?) ('Route $route', null),
          ], style: muted),
        OoTime(mission.updatedAt, prefix: 'Updated '),
        if (agent != null || role != null)
          OoLinkLine([
            if (agent != null)
              (
                'Agent $agent',
                OoLinkTo.agent(agent, operation: mission.operationId),
              ),
            if (role != null)
              (
                'Role $role',
                OoLinkTo.role(role, operation: mission.operationId),
              ),
          ], style: muted),
      ],
    );
  }
}

/// Every attempt at a Mission, first first, with why a lost one was lost.
class _AttemptHistory extends StatelessWidget {
  const _AttemptHistory({required this.attempts});

  final List<MissionAttemptSummary> attempts;

  @override
  Widget build(BuildContext context) {
    final muted = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: OoTokens.of(context).muted);
    return Padding(
      padding: const EdgeInsets.only(left: 27, bottom: 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (final attempt in attempts) ...[
            OoLinkLine([
              ('Attempt ${attempt.attempt}', null),
              if (attempt.runId case final run?) ('Run $run', null),
              (attempt.status, null),
              ('Station ${attempt.station}', OoLinkTo.station(attempt.station)),
              if (attempt.theater case final theater?)
                (
                  'Theater $theater',
                  OoLinkTo.theater(attempt.station, theater),
                ),
              if (attempt.route case final route?) ('Route $route', null),
            ], style: Theme.of(context).textTheme.labelMedium),
            if (attempt.lost case final lost?) Text(lost, style: muted),
            if (attempt.checkpointBranch case final branch?)
              Text('Checkpointed to branch $branch', style: muted),
          ],
        ],
      ),
    );
  }
}

class MissionBriefPage extends StatelessWidget {
  const MissionBriefPage({required this.brief, super.key});

  final MissionBriefSummary brief;

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: Text('Mission ${brief.handle ?? brief.missionId}')),
    body: ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Text(brief.operationId, style: Theme.of(context).textTheme.titleMedium),
        const SizedBox(height: 20),
        Text('Brief', style: Theme.of(context).textTheme.titleLarge),
        const SizedBox(height: 8),
        SelectableText(brief.prompt),
        const SizedBox(height: 24),
        Text('Launch context', style: Theme.of(context).textTheme.titleLarge),
        const SizedBox(height: 8),
        _manifestRow('Origin', brief.origin?.describe() ?? 'Not recorded'),
        if (brief.origin?.itemUrl case final url?) _manifestRow('Item', url),
        _manifestRow('Agent', brief.agentId ?? 'Unassigned'),
        _manifestRow('Role', brief.roleId ?? 'Unspecified'),
        _manifestRow('Station', brief.stationId ?? 'Not recorded'),
        _manifestRow('Theater', brief.theaterId ?? 'Not recorded'),
        if (brief.mode != null) _manifestRow('Mode', brief.mode!),
        _manifestRow('Handler', brief.handlerProvider),
        _manifestRow('Terminal', brief.terminalProvider),
        _manifestRow('Working directory', brief.workingDirectory),
        _manifestRow('Model', brief.model ?? 'Configured by handler'),
        _manifestRow('Effort', brief.effort ?? 'Configured by handler'),
        _manifestRow(
          'Environment names',
          brief.environmentKeys.isEmpty
              ? 'None'
              : brief.environmentKeys.join(', '),
        ),
        const SizedBox(height: 8),
        Text(
          'Environment values are withheld.',
          style: Theme.of(context).textTheme.bodySmall,
        ),
      ],
    ),
  );

  Widget _manifestRow(String label, String value) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 5),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(width: 160, child: Text(label)),
        Expanded(child: SelectableText(value)),
      ],
    ),
  );
}

class _SessionRow extends StatelessWidget {
  const _SessionRow({
    required this.session,
    required this.board,
    required this.desktopWindows,
    required this.onSelectSession,
  });

  final SessionSummary session;
  final MissionBoard board;
  final bool desktopWindows;
  final ValueChanged<SessionSummary>? onSelectSession;

  void _open(BuildContext context) {
    if (onSelectSession case final select?) {
      select(session);
    } else if (desktopWindows) {
      openSessionWindow(context, session, board);
    } else {
      openSessionPage(context, session, board);
    }
  }

  @override
  Widget build(BuildContext context) {
    final state = session.state ?? 'working';
    final tokens = OoTokens.of(context);
    final marks = Wrap(
      spacing: 6,
      runSpacing: 4,
      children: [
        if (session.exposure.isNotEmpty)
          Tooltip(
            message: 'Read untrusted input: ${session.exposure.join(', ')}',
            child: const Chip(
              label: Text('EXPOSED'),
              visualDensity: VisualDensity.compact,
            ),
          ),
        Chip(
          label: Text(
            [
              state,
              if (session.reason case final reason?) reasonWords(reason),
            ].join(' · ').toUpperCase(),
          ),
          visualDensity: VisualDensity.compact,
        ),
      ],
    );
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: tokens.raised,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: tokens.border),
      ),
      child: InkWell(
        onTap: () => _open(context),
        borderRadius: BorderRadius.circular(8),
        child: Row(
          children: [
            OoStateMark(state, reason: session.reason),
            const SizedBox(width: 10),
            Expanded(
              // The state chips stay beside the ID when there is room and go under it when not.
              child: Wrap(
                alignment: WrapAlignment.spaceBetween,
                crossAxisAlignment: WrapCrossAlignment.center,
                spacing: 8,
                runSpacing: 4,
                children: [
                  Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        session.sessionId,
                        style: const TextStyle(fontFamily: 'monospace'),
                      ),
                      Text(
                        'Updated at event ${session.sequence}',
                        style: Theme.of(context).textTheme.labelSmall
                            ?.copyWith(color: tokens.muted),
                      ),
                    ],
                  ),
                  marks,
                ],
              ),
            ),
            if (desktopWindows) ...[
              const SizedBox(width: 8),
              IconButton(
                tooltip: 'Open Session window',
                onPressed: () => _open(context),
                icon: const Icon(Icons.open_in_new),
              ),
            ] else ...[
              const SizedBox(width: 8),
              IconButton(
                tooltip: 'View Session details',
                onPressed: () => _open(context),
                icon: const Icon(Icons.chevron_right),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

void openSessionPage(
  BuildContext context,
  SessionSummary session,
  MissionBoard board,
) {
  Navigator.of(context).push(
    MaterialPageRoute<void>(
      builder: (_) => SessionWindow(session: session, board: board),
    ),
  );
}

void openSessionWindow(
  BuildContext context,
  SessionSummary session,
  MissionBoard board,
) => openAppWindow(
  context,
  size: const Size(900, 680),
  title: 'Session ${session.sessionId} | Outer Office',
  builder: (_) => boardThemedApp(
    board,
    title: 'Session ${session.sessionId}',
    home: SessionWindow(session: session, board: board),
  ),
);

class SessionWindow extends StatelessWidget {
  const SessionWindow({required this.session, required this.board, super.key});

  final SessionSummary session;
  final MissionBoard board;

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: Text('Session ${session.sessionId}')),
    body: _SessionDetailContent(session: session, board: board),
  );
}

class SessionDetailPane extends StatelessWidget {
  const SessionDetailPane({
    required this.session,
    required this.board,
    required this.onClose,
    super.key,
  });

  final SessionSummary session;
  final MissionBoard board;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) => Column(
    children: [
      ListTile(
        title: Text(
          'Session ${session.sessionId}',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        trailing: IconButton(
          tooltip: 'Close Session details',
          onPressed: onClose,
          icon: const Icon(Icons.close),
        ),
      ),
      const Divider(height: 1),
      Expanded(
        child: _SessionDetailContent(session: session, board: board),
      ),
    ],
  );
}

class _SessionDetailContent extends StatelessWidget {
  const _SessionDetailContent({required this.session, required this.board});

  final SessionSummary session;
  final MissionBoard board;

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: board,
    builder: (context, _) {
      final current =
          board.sessions
              .where((item) => item.sessionId == session.sessionId)
              .firstOrNull ??
          session;
      return Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              current.operationId,
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 4),
            Text(
              'Mission ${board.handleOf(current.operationId, current.missionId)} · '
              '${current.state ?? 'awaiting lifecycle'}',
            ),
            const SizedBox(height: 12),
            Expanded(
              child: DefaultTabController(
                length: 2,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const TabBar(
                      tabs: [
                        Tab(text: 'Terminal'),
                        Tab(text: 'Context'),
                      ],
                    ),
                    const SizedBox(height: 12),
                    Expanded(
                      child: TabBarView(
                        children: [
                          board.demo
                              ? const _SessionPreviewPanel(
                                  message: 'Demo Sessions are sample data. No terminal connection is available.',
                                )
                              : board.accessToken == null
                              ? const _SessionPreviewPanel(
                                  message: 'Sign in to Universal Exports to attach to this Session.',
                                )
                              : _LiveSessionTerminal(
                                  session: current,
                                  board: board,
                                ),
                          board.demo || board.accessToken == null
                              ? const _SessionPreviewPanel(
                                  message:
                                      "A Session's context is read from Universal Exports "
                                      'once you are signed in.',
                                )
                              : SessionContextPanel(
                                  reads: board,
                                  operation: current.operationId,
                                  session: current.sessionId,
                                ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      );
    },
  );
}

class _SessionPreviewPanel extends StatelessWidget {
  const _SessionPreviewPanel({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) => Card(
    child: Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.terminal, size: 36, color: OoTokens.of(context).accent),
            const SizedBox(height: 12),
            const Text('Session preview'),
            const SizedBox(height: 8),
            Text(message, textAlign: TextAlign.center),
          ],
        ),
      ),
    ),
  );
}

class _LiveSessionTerminal extends StatefulWidget {
  const _LiveSessionTerminal({required this.session, required this.board});

  final SessionSummary session;
  final MissionBoard board;

  @override
  State<_LiveSessionTerminal> createState() => _LiveSessionTerminalState();
}

class _LiveSessionTerminalState extends State<_LiveSessionTerminal> {
  late final SessionConnection _connection;

  @override
  void initState() {
    super.initState();
    _connection = SessionConnection(
      board: widget.board,
      session: widget.session,
    )..addListener(_changed);
    _connection.start();
  }

  @override
  void dispose() {
    _connection.removeListener(_changed);
    _connection.dispose();
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  @override
  Widget build(BuildContext context) => Card(
    clipBehavior: Clip.antiAlias,
    child: Column(
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
          child: Row(
            children: [
              Expanded(child: Text(_connection.status)),
              TextButton.icon(
                onPressed: _connection.isConnected
                    ? () => _connection.setWriteLease(
                        !_connection.writeLeaseRequested,
                      )
                    : null,
                icon: Icon(
                  _connection.writeLeaseRequested
                      ? Icons.lock_open
                      : Icons.lock_outline,
                ),
                label: Text(
                  _connection.writeLeaseRequested
                      ? 'Release input lease'
                      : 'Request input lease',
                ),
              ),
            ],
          ),
        ),
        const Divider(height: 1),
        Expanded(
          child: _connection.runtimeError == null
              ? SessionTerminal(
                  output: _connection.output,
                  onInput: _connection.sendInput,
                  onResize: _connection.sendResize,
                  writeLeaseHeld: _connection.writeLeaseHeld,
                  maxLines: widget.board.limits.terminal.scrollbackLines,
                )
              : Center(
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: Text(_connection.runtimeError!),
                  ),
                ),
        ),
      ],
    ),
  );
}

/// The board with no Mission on it: what a Mission is, and a way to start the first in each
/// Operation the person may, or who can if they may not.
class EmptyBoard extends StatelessWidget {
  const EmptyBoard({this.operations = const [], this.onCreate, super.key});

  /// The Operations the person may start a Mission in.
  final List<String> operations;
  final ValueChanged<String>? onCreate;

  @override
  Widget build(BuildContext context) => OoEmptyState(
    icon: OoIconName.missions,
    backdrop: true,
    title: 'No Missions yet',
    body: operations.isEmpty
        ? 'A Mission is one brief of work under an Operation, and its Sessions show here. Your '
              'account cannot start one; a controller grants mission.create on an Operation.'
        : 'A Mission is one brief of work under an Operation. Start the first, and its '
              'Sessions show here.',
    actions: [
      if (onCreate case final onCreate?)
        for (final operation in operations)
          ('New Mission in $operation', () => onCreate(operation)),
    ],
    link: const ('See Stations', OoRoute(OoPage.stations)),
  );
}

class DemoBanner extends StatelessWidget {
  const DemoBanner({super.key});

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 0, 24, 12),
    child: DecoratedBox(
      decoration: BoxDecoration(
        color: OoTokens.of(context).raised,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: OoTokens.of(context).accent),
      ),
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Row(
          children: [
            Icon(
              Icons.visibility,
              color: OoTokens.of(context).accent,
              size: 18,
            ),
            const SizedBox(width: 10),
            const Expanded(
              child: Text(
                'Demo data: nothing here is connected, and changes are turned off.',
              ),
            ),
          ],
        ),
      ),
    ),
  );
}

class ErrorBanner extends StatelessWidget {
  const ErrorBanner({
    required this.apiOrigin,
    required this.onRetry,
    super.key,
  });

  final Uri apiOrigin;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
    child: MaterialBanner(
      content: Text(
        'Could not reach Universal Exports at ${apiOrigin.host}:${apiOrigin.port}.',
      ),
      leading: const Icon(Icons.cloud_off),
      actions: [TextButton(onPressed: onRetry, child: const Text('Retry'))],
    ),
  );
}
