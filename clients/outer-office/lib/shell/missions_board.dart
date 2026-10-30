import 'package:material_ui/material_ui.dart';

import '../controls/mission_actions.dart';
import '../main.dart';
import '../operations/operation_tree.dart';
import '../routes/oo_route.dart';
import '../terminal/session_connection.dart';
import '../terminal/session_terminal.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'app_window.dart';
import 'oo_freshness.dart';

/// The Missions a Missions [route] selects from the board, in its order. A state filter keeps
/// the Sessions in those states (a Mission shows when one of its Sessions does); a Station or
/// Agent filter keeps the Missions with a Session on that Station or for that Agent.
List<MissionGroup> routeMissions(MissionBoard board, OoRoute route) {
  final states = route.states;
  final sessions = [
    for (final session in board.sessions)
      if (states.isEmpty || states.contains(session.state ?? 'working'))
        session,
  ];
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
    if (route['operation_id'] case final operation?
        when subtree != null
            ? !subtree.contains(mission.operationId)
            : mission.operationId != operation) {
      return false;
    }
    if (route['mission_id'] case final id? when mission.missionId != id) {
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
        for (final session in mission.sessions) session.sessionId,
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
    'mission' => (mission) => mission.missionId,
    _ => updated,
  };
  // By update time the newest comes first unless the route says `asc`; otherwise `desc`
  // reverses. Ties keep the board's order (Operation, then Mission ID) either way, so rows
  // never swap between refreshes.
  final descending = sort == 'updated'
      ? route['order'] != 'asc'
      : route['order'] == 'desc';
  final missions = groupMissions(sessions).where(matches).indexed.toList()
    ..sort((a, b) {
      final order = key(a.$2).compareTo(key(b.$2));
      return order != 0 ? (descending ? -order : order) : a.$1.compareTo(b.$1);
    });
  return [for (final (_, mission) in missions) mission];
}

class BoardHeader extends StatelessWidget {
  const BoardHeader({
    required this.sessions,
    required this.selectedStates,
    required this.onSelectStates,
    required this.sort,
    super.key,
  });

  final List<SessionSummary> sessions;

  /// The states shown; none means every state.
  final List<String> selectedStates;
  final ValueChanged<List<String>> onSelectStates;

  /// The sort control beside the heading.
  final Widget sort;

  @override
  Widget build(BuildContext context) {
    final counts = <String, int>{};
    for (final session in sessions) {
      final state = session.state ?? 'working';
      counts.update(state, (value) => value + 1, ifAbsent: () => 1);
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'MISSIONS',
            style: Theme.of(context).textTheme.labelMedium?.copyWith(
              letterSpacing: 2.4,
              color: OoTokens.of(context).accent,
            ),
          ),
          const SizedBox(height: 4),
          Row(
            children: [
              Expanded(
                child: Text(
                  'Mission board',
                  style: Theme.of(context).textTheme.headlineMedium
                      ?.copyWith(fontWeight: FontWeight.w600),
                ),
              ),
              sort,
            ],
          ),
          const SizedBox(height: 16),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _CountChip(
                label: 'Sessions',
                count: sessions.length,
                selected: selectedStates.isEmpty,
                onPressed: () => onSelectStates(const []),
              ),
              for (final state in boardStates)
                _CountChip(
                  label: state,
                  count: counts[state] ?? 0,
                  state: state,
                  selected: selectedStates.contains(state),
                  // One state at a time from the chips; a link may name several.
                  onPressed: () => onSelectStates(
                    selectedStates.length == 1 && selectedStates.single == state
                        ? const []
                        : [state],
                  ),
                ),
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

  /// The state this chip filters by; none for all Sessions.
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
    tooltip: label == 'Sessions' ? 'Show all Sessions' : 'Filter by $label',
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

  /// The Session that sets the Mission's state: the one with the latest event.
  SessionSummary get latest => sessions.reduce(
    (left, right) => right.sequence > left.sequence ? right : left,
  );

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
    return station != 0 ? station : left.missionId.compareTo(right.missionId);
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
                child: Text(
                  mission.operationId,
                  style: Theme.of(context).textTheme.titleMedium,
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
          Padding(
            padding: const EdgeInsets.only(left: 27, top: 4, bottom: 8),
            child: Text(
              'Mission ${mission.missionId}',
              style: Theme.of(context).textTheme.bodySmall
                  ?.copyWith(color: OoTokens.of(context).muted),
            ),
          ),
          Padding(
            padding: const EdgeInsets.only(left: 27, bottom: 12),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: MissionFacts(mission: mission, board: board),
                ),
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
            Text(
              [
                mission.state,
                ?mission.reason?.replaceAll('-', ' '),
              ].join(' · '),
              style: Theme.of(context).textTheme.labelMedium,
            ),
          ],
        ),
        if (station != null)
          Text(
            [
              'Station $station',
              if (attempt?.theater case final theater?) 'Theater $theater',
            ].join(' · '),
            style: muted,
          ),
        OoTime(mission.updatedAt, prefix: 'Updated '),
        if (agent != null || role != null)
          Text(
            [
              ?agent == null ? null : 'Agent $agent',
              ?role == null ? null : 'Role $role',
            ].join(' · '),
            style: muted,
          ),
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
            Text(
              attempt.describe(),
              style: Theme.of(context).textTheme.labelMedium,
            ),
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
    appBar: AppBar(title: Text('Mission ${brief.missionId}')),
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
              child: Column(
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
            ),
            if (session.exposure.isNotEmpty) ...[
              Tooltip(
                message: 'Read untrusted input: ${session.exposure.join(', ')}',
                child: const Chip(
                  label: Text('EXPOSED'),
                  visualDensity: VisualDensity.compact,
                ),
              ),
              const SizedBox(width: 6),
            ],
            Chip(
              label: Text(
                [
                  state,
                  ?session.reason?.replaceAll('-', ' '),
                ].join(' · ').toUpperCase(),
              ),
              visualDensity: VisualDensity.compact,
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
              'Mission ${current.missionId} · ${current.state ?? 'awaiting lifecycle'}',
            ),
            const SizedBox(height: 24),
            Expanded(
              child: board.demo
                  ? const _SessionPreviewPanel(
                      message: 'Demo Sessions are sample data. No terminal connection is available.',
                    )
                  : board.accessToken == null
                  ? const _SessionPreviewPanel(
                      message: 'Sign in to Universal Exports to attach to this Session.',
                    )
                  : _LiveSessionTerminal(session: current, board: board),
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
  void didUpdateWidget(_LiveSessionTerminal oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.session.sequence != widget.session.sequence) {
      _connection.advanceCursor(widget.session.sequence);
    }
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

class EmptyBoard extends StatelessWidget {
  const EmptyBoard({super.key});

  @override
  Widget build(BuildContext context) => Center(
    child: OoBackdrop(
      alignment: Alignment.center,
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            IconTheme.merge(
              data: IconThemeData(size: 42, color: OoTokens.of(context).muted),
              child: const OoIcon(OoIconName.missions),
            ),
            const SizedBox(height: 12),
            const Text('No active Missions'),
            const SizedBox(height: 4),
            const Text('Registered agent Sessions will appear here.'),
          ],
        ),
      ),
    ),
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
                'Demo preview · sign-in is unavailable. Open the authenticated Outer Office site.',
              ),
            ),
          ],
        ),
      ),
    ),
  );
}

class GatewayUnavailable extends StatelessWidget {
  const GatewayUnavailable({super.key});

  @override
  Widget build(BuildContext context) => Center(
    child: OoBackdrop(
      alignment: Alignment.center,
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.cloud_off, size: 42, color: OoTokens.of(context).muted),
            const SizedBox(height: 12),
            const Text('Universal Exports is offline'),
            const SizedBox(height: 4),
            const Text(
              'Start the gateway, or launch with OO_DEMO=true to preview.',
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
