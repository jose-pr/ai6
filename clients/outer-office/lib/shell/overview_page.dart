import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../assistant/assistant_controller.dart';
import '../assistant/assistant_panel.dart'
    show MoneypennyAvatar, moneypennyPrompts;
import '../held/held_briefs_page.dart' show revealInvisible;
import '../settings/oo_config.dart';
import '../main.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_decor.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_theme.dart';
import '../timeline/timeline_page.dart';
import '../routes/oo_route.dart';
import 'missions_board.dart';
import 'oo_freshness.dart';
import 'oo_link.dart';
import 'oo_shell.dart';
import 'top_bar.dart';

/// The command overview, all from what the board has loaded: the theme's hero art, the
/// person's Operations with their progress, counts of Operations, Stations, active Missions
/// and waiting Sessions, recent activity, the Missions waiting to run, Moneypenny, and what
/// needs the person's attention.
class OverviewPage extends StatelessWidget {
  const OverviewPage({
    required this.board,
    required this.pages,
    required this.onOpen,
    required this.onRoute,
    this.assistant,
    this.onAsk,
    this.activityEvery = RefreshSettings.defaultOverview,
    super.key,
  });

  final MissionBoard board;

  /// The pages this person may open; links go only to these.
  final List<OoPage> pages;
  final ValueChanged<OoPage> onOpen;

  /// Opens the page an attention item points to, with its filters.
  final ValueChanged<OoRoute> onRoute;

  /// The conversation with Moneypenny; null where she is not served, and her card goes too.
  final AssistantController? assistant;

  /// Opens Moneypenny's chat and asks her the text.
  final ValueChanged<String>? onAsk;

  /// How often Recent Activity is fetched again while shown; `null` only once.
  final Duration? activityEvery;

  /// The narrowest a stat tile reads well: its icon, count and a short label on one row.
  static const double statTileWidth = 230;

  // Sized by the width the page has, not the window's: a docked chat takes part of it.
  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) => _page(context, constraints.maxWidth),
  );

  Widget _page(BuildContext context, double width) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final phone = width < OoBreakpoints.phone;
    final missions = groupMissions(board.sessions);
    final active = missions.where((mission) => !mission.finished).toList();
    final working = active.where((mission) => mission.state == 'working');
    final stations = board.stationBoard.stations;
    final online = stations.where((station) => station.healthy).length;
    final waiting = board.sessions
        .where((session) => session.state == 'blocked')
        .toList();
    final operations = board.userSession?.operations ?? const [];
    final busyOperations = active.map((mission) => mission.operationId).toSet();
    final attention = attentionItems(board, tokens);
    final gaps = attentionGaps(board);
    final muted = TextStyle(color: tokens.muted);

    final heroText = Padding(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'AI6 / UNIVERSAL EXPORTS',
            style: ooSectionLabel(context).copyWith(fontSize: 11.5),
          ),
          const SizedBox(height: 6),
          Text(
            'Command overview',
            style: (phone ? text.titleLarge : text.headlineSmall)?.copyWith(
              fontWeight: FontWeight.w600,
            ),
          ),
          if (!phone) ...[
            const SizedBox(height: 4),
            Text(
              'Operations and field activity across the Stations you can see',
              style: text.bodyMedium?.copyWith(color: tokens.muted),
            ),
          ],
        ],
      ),
    );
    final operationsPanel = _ActiveOperations(
      board: board,
      missions: missions,
      onOpen: pages.contains(OoPage.operations)
          ? () => onOpen(OoPage.operations)
          : null,
    );
    final stats = [
      _Stat(
        compact: phone,
        icon: OoIconName.operations,
        label: 'Operations',
        count: operations.length,
        route: const OoRoute(OoPage.operations),
        detail: TextSpan(
          text:
              '${operations.where((operation) => busyOperations.contains(operation.operationId)).length} with active Missions',
          style: TextStyle(color: tokens.accent),
        ),
      ),
      _Stat(
        compact: phone,
        icon: OoIconName.stations,
        label: 'Stations',
        count: stations.length,
        route: const OoRoute(OoPage.stations),
        detail: TextSpan(
          children: [
            TextSpan(
              text: '$online online',
              style: TextStyle(color: tokens.success),
            ),
            TextSpan(text: ' · ', style: muted),
            TextSpan(
              text: '${stations.length - online} dark',
              style: TextStyle(
                color: stations.length > online ? tokens.danger : tokens.muted,
              ),
            ),
          ],
        ),
      ),
      _Stat(
        compact: phone,
        icon: OoIconName.missions,
        label: 'Missions',
        count: active.length,
        route: OoLinkTo.missions(states: activeMissionStates),
        detail: TextSpan(
          children: [
            TextSpan(
              text: '${working.length} working',
              style: TextStyle(color: tokens.accent),
            ),
            TextSpan(text: ' · ${missions.length} on the board', style: muted),
          ],
        ),
      ),
      _Stat(
        compact: phone,
        icon: OoIconName.sessions,
        label: 'Sessions waiting',
        count: waiting.length,
        route: OoLinkTo.sessions(state: 'blocked'),
        detail: TextSpan(
          text: 'of ${board.sessions.length} Sessions',
          style: TextStyle(
            color: waiting.isEmpty ? tokens.muted : tokens.warning,
          ),
        ),
      ),
    ];
    final activity = _RecentActivity(
      board: board,
      every: activityEvery,
      onOpen: pages.contains(OoPage.timeline)
          ? () => onOpen(OoPage.timeline)
          : null,
    );
    final upcoming = _Upcoming(
      board: board,
      missions: missions,
      onOpen: (page) => pages.contains(page) ? () => onOpen(page) : null,
    );
    final moneypenny = switch ((assistant, onAsk)) {
      (final assistant?, final onAsk?) => _MoneypennyCard(
        board: board,
        assistant: assistant,
        onAsk: onAsk,
        waiting: attention.length,
        covered: gaps == null,
      ),
      _ => null,
    };
    final gap = phone ? 12.0 : 16.0;
    final side = width >= 1000;
    final threeColumns = width >= 1080;
    // Four tiles in a row, two, or one: never three and an orphan.
    final fit = ((width - gap) / (statTileWidth + gap)).floor();
    final statColumns = phone || fit < 2
        ? 1
        : fit >= 4
        ? 4
        : 2;
    final statWidth = (width - 2 * gap - gap * (statColumns - 1)) / statColumns;
    // A column, not a lazy list: Overview is a few panels, and every one of them is
    // built so search and screen readers see the whole page.
    return SingleChildScrollView(
      padding: EdgeInsets.fromLTRB(gap, gap, gap, 32),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (side)
            IntrinsicHeight(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(minHeight: 270),
                      child: OoHero(height: null, child: heroText),
                    ),
                  ),
                  SizedBox(width: gap),
                  SizedBox(width: 340, child: operationsPanel),
                ],
              ),
            )
          else ...[
            OoHero(height: phone ? 170 : 220, child: heroText),
            SizedBox(height: gap),
            operationsPanel,
          ],
          SizedBox(height: gap),
          Wrap(
            spacing: gap,
            runSpacing: gap,
            children: [
              for (final stat in stats) SizedBox(width: statWidth, child: stat),
            ],
          ),
          SizedBox(height: gap),
          if (threeColumns)
            IntrinsicHeight(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(child: activity),
                  SizedBox(width: gap),
                  Expanded(child: upcoming),
                  if (moneypenny != null) ...[
                    SizedBox(width: gap),
                    Expanded(child: moneypenny),
                  ],
                ],
              ),
            )
          else ...[
            if (moneypenny != null) ...[moneypenny, SizedBox(height: gap)],
            activity,
            SizedBox(height: gap),
            upcoming,
          ],
          SizedBox(height: gap),
          if (attention.isEmpty)
            OoBackdrop(
              padding: EdgeInsets.zero,
              child: Padding(
                padding: const EdgeInsets.all(20),
                child: gaps == null
                    ? const Text('Nothing needs your attention.')
                    : Text('Nothing found in what loaded. $gaps'),
              ),
            )
          else
            OoPanel(
              title: 'NEEDS ATTENTION',
              trailing: Text('${attention.length}', style: text.titleMedium),
              child: Column(
                children: [
                  for (final item in attention)
                    ListTile(
                      contentPadding: EdgeInsets.zero,
                      leading: IconTheme.merge(
                        data: IconThemeData(color: item.color),
                        child: OoIcon(item.icon),
                      ),
                      title: Text(
                        item.text,
                        maxLines: phone ? 3 : 2,
                        overflow: TextOverflow.ellipsis,
                      ),
                      subtitle: OoTime(item.at),
                      trailing: pages.contains(item.route.page) && !phone
                          ? TextButton(
                              onPressed: () => onRoute(item.route),
                              child: Text('Open ${item.route.page.label}'),
                            )
                          : null,
                      onTap: pages.contains(item.route.page) && phone
                          ? () => onRoute(item.route)
                          : null,
                    ),
                  if (gaps != null) Text(gaps, style: muted),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// The states of a Mission that has not stopped, which the Missions tile counts.
const activeMissionStates = [
  'queued',
  'working',
  'blocked',
  'stalled',
  'parked',
  'quarantined',
  'verifying',
];

/// A count that opens [route], the page filtered to what it counted.
class _Stat extends StatelessWidget {
  const _Stat({
    required this.compact,
    required this.icon,
    required this.label,
    required this.count,
    required this.detail,
    required this.route,
  });

  final OoRoute route;

  /// A phone-sized page: a smaller tile.
  final bool compact;
  final OoIconName icon;
  final String label;
  final int count;
  final TextSpan detail;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final open = OoLinks.opener(context, route);
    return Card(
      clipBehavior: Clip.antiAlias,
      child: Semantics(
        link: open != null,
        label: 'Open $label: $count',
        child: InkWell(
          onTap: open,
          focusColor: tokens.accent.withValues(alpha: 0.2),
          child: Padding(
            padding: EdgeInsets.all(compact ? 12 : 16),
            child: Row(
              children: [
                OoTile(icon, size: compact ? 48 : 60),
                const SizedBox(width: 16),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        label,
                        style: text.labelLarge?.copyWith(color: tokens.muted),
                      ),
                      Text(
                        '$count',
                        style: text.headlineMedium?.copyWith(
                          fontWeight: FontWeight.w600,
                          height: 1.25,
                        ),
                      ),
                      Text.rich(
                        detail,
                        style: text.bodySmall,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Each of the person's Operations with a status dot and the share of its Missions done.
class _ActiveOperations extends StatelessWidget {
  const _ActiveOperations({
    required this.board,
    required this.missions,
    required this.onOpen,
  });

  final MissionBoard board;
  final List<MissionGroup> missions;
  final VoidCallback? onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final operations = board.userSession?.operations ?? const [];
    final stations = board.stationBoard.stations;
    return OoPanel(
      title: 'OPERATIONS',
      trailing: Text('${operations.length}', style: text.titleMedium),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (operations.isEmpty)
            Text(
              'No Operations you can see. A controller grants access to Operations.',
              style: TextStyle(color: tokens.muted),
            ),
          for (final operation in operations.take(board.limits.pages.summary))
            _operationRow(context, operation.operationId),
          if (operations.length > board.limits.pages.summary && onOpen != null)
            Align(
              alignment: Alignment.centerRight,
              child: TextButton(
                onPressed: onOpen,
                child: Text('All ${operations.length} Operations'),
              ),
            ),
          if (stations.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text(
              'STATIONS',
              style: ooSectionLabel(context).copyWith(fontSize: 11),
            ),
            const SizedBox(height: 10),
            for (final station in stations.take(board.limits.pages.summary))
              Padding(
                padding: const EdgeInsets.only(bottom: 10),
                child: Row(
                  children: [
                    OoStatusDot(
                      station.healthy ? tokens.success : tokens.danger,
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Align(
                        alignment: Alignment.centerLeft,
                        child: OoLink(
                          station.stationId,
                          route: OoLinkTo.station(station.stationId),
                          semanticLabel: 'Open Station ${station.stationId}',
                          maxLines: 1,
                          style: text.bodyMedium,
                        ),
                      ),
                    ),
                    const SizedBox(width: 8),
                    Flexible(
                      child: Text(
                        [
                          if (station.headquarters) 'HQS',
                          station.healthy
                              ? '${station.running} of ${station.capacity} running'
                              : 'dark',
                        ].join(' · '),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: text.bodySmall?.copyWith(
                          color: station.healthy ? tokens.muted : tokens.danger,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
          ],
        ],
      ),
    );
  }

  Widget _operationRow(BuildContext context, String operationId) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final own = missions.where((mission) => mission.operationId == operationId);
    final total = own.length;
    final done = own.where((mission) => mission.state == 'done').length;
    final active = own.where((mission) => !mission.finished).toList();
    final (color, status) =
        active.any(
          (mission) => const {
            'blocked',
            'stalled',
            'quarantined',
            'held',
            'verifying',
          }.contains(mission.state),
        )
        ? (tokens.warning, 'needs attention')
        : active.isNotEmpty
        ? (tokens.success, 'active')
        : (tokens.muted, 'idle');
    return Padding(
      padding: const EdgeInsets.only(bottom: 14),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            // Beside the name, which a phone's tap target sets lower.
            padding: EdgeInsets.only(
              top: MediaQuery.sizeOf(context).width < OoBreakpoints.phone
                  ? 20
                  : 6,
            ),
            child: OoStatusDot(color),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Align(
                  alignment: Alignment.centerLeft,
                  child: OoLink(
                    'Operation $operationId',
                    route: OoLinkTo.operation(operationId),
                    style: text.titleSmall,
                  ),
                ),
                Text(
                  total == 0
                      ? '$status · no Missions yet'
                      : '$status · $done of $total Missions done',
                  style: text.bodySmall?.copyWith(color: tokens.muted),
                ),
                const SizedBox(height: 6),
                ClipRRect(
                  borderRadius: BorderRadius.circular(3),
                  child: LinearProgressIndicator(
                    value: total == 0 ? 0 : done / total,
                    minHeight: 5,
                    color: color == tokens.muted ? tokens.accent : color,
                    backgroundColor: tokens.border,
                    semanticsLabel:
                        'Operation $operationId: $done of $total Missions done',
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// The newest activity across the Operations whose timeline the person may read.
class _RecentActivity extends StatefulWidget {
  const _RecentActivity({
    required this.board,
    required this.every,
    required this.onOpen,
  });

  final MissionBoard board;
  final Duration? every;
  final VoidCallback? onOpen;

  @override
  State<_RecentActivity> createState() => _RecentActivityState();
}

class _RecentActivityState extends State<_RecentActivity> {
  int get _shown => widget.board.limits.pages.summary;
  List<TimelineItemSummary>? _items;
  bool _failed = false;
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
    if (widget.every case final every?) {
      _timer = Timer.periodic(every, (_) {
        if (widget.board.visible) unawaited(_load());
      });
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    final operations = widget.board.timelineOperations;
    if (operations.isEmpty) return;
    var failed = false;
    final pages = await Future.wait([
      for (final operation in operations)
        widget.board
            .loadTimeline(operationId: operation, limit: _shown)
            .then<List<TimelineItemSummary>>((page) => page.items)
            .catchError((Object _) {
              failed = true;
              return const <TimelineItemSummary>[];
            }),
    ]);
    if (!mounted) return;
    final items = [for (final page in pages) ...page]
      ..sort((left, right) => right.at.compareTo(left.at));
    setState(() {
      _items = items.take(_shown).toList();
      _failed = failed && items.isEmpty;
    });
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final items = _items;
    final Widget body;
    if (widget.board.timelineOperations.isEmpty) {
      body = Text(
        'Your account does not read any Operation\'s timeline.',
        style: TextStyle(color: tokens.muted),
      );
    } else if (_failed) {
      body = Text(
        'Recent activity could not be loaded.',
        style: TextStyle(color: tokens.muted),
      );
    } else if (items == null) {
      body = const LinearProgressIndicator();
    } else if (items.isEmpty) {
      body = Text(
        'No activity yet. It shows here once a Mission runs.',
        style: TextStyle(color: tokens.muted),
      );
    } else {
      body = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final item in items)
            Padding(
              padding: const EdgeInsets.only(bottom: 12),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(
                    width: 84,
                    child: OoTime(
                      item.at,
                      style: text.labelMedium?.copyWith(color: tokens.muted),
                    ),
                  ),
                  OoDuotone(timelineKindIcon(item.kind), size: 26),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          timelineSentence(item.action),
                          style: text.bodyMedium,
                        ),
                        OoLinkLine(item.names),
                      ],
                    ),
                  ),
                ],
              ),
            ),
        ],
      );
    }
    return OoPanel(
      title: 'RECENT ACTIVITY',
      trailing: widget.onOpen == null
          ? null
          : TextButton(onPressed: widget.onOpen, child: const Text('View all')),
      child: body,
    );
  }
}

/// The icon of a timeline item of [kind].
OoIconName timelineKindIcon(String kind) => switch (kind) {
  'lifecycle' => OoIconName.missions,
  'control' => OoIconName.resume,
  'cable' => OoIconName.cable,
  'sleeper' => OoIconName.sleeper,
  _ => OoIconName.audit,
};

const _months = [
  'Jan',
  'Feb',
  'Mar',
  'Apr',
  'May',
  'Jun',
  'Jul',
  'Aug',
  'Sep',
  'Oct',
  'Nov',
  'Dec',
];

DateTime _local(double seconds) =>
    DateTime.fromMillisecondsSinceEpoch((seconds * 1000).round()).toLocal();

/// The Missions waiting to run: queued Missions on the board, oldest first, then briefs
/// vetting holds for review. Briefs no Station has placed yet are not on the board.
class _Upcoming extends StatelessWidget {
  const _Upcoming({
    required this.board,
    required this.missions,
    required this.onOpen,
  });

  final MissionBoard board;
  final List<MissionGroup> missions;
  final VoidCallback? Function(OoPage page) onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    double? queuedAt(MissionGroup mission) => mission.sessions
        .map((session) => session.updatedAt)
        .whereType<double>()
        .fold<double?>(
          null,
          (latest, at) => latest == null || at > latest ? at : latest,
        );
    final queued =
        missions.where((mission) => mission.state == 'queued').toList()
          ..sort((left, right) {
            final a = queuedAt(left) ?? double.infinity;
            final b = queuedAt(right) ?? double.infinity;
            return a.compareTo(b);
          });
    final rows = <Widget>[
      for (final mission in queued)
        _UpcomingRow(
          kind: 'Queued Mission',
          at: queuedAt(mission),
          color: tokens.accent,
          title:
              'Mission ${board.handleOf(mission.operationId, mission.missionId)}',
          route: OoLinkTo.mission(mission.operationId, mission.missionId),
          detail: [
            ?board.missionBrief(mission.operationId, mission.missionId)?.name,
            'Operation ${mission.operationId}',
            'queued',
            ?board
                .missionBrief(mission.operationId, mission.missionId)
                ?.agentId,
            ?board.missionBrief(mission.operationId, mission.missionId)?.roleId,
            if (board
                    .missionBrief(mission.operationId, mission.missionId)
                    ?.sourceIssue
                case final issue?)
              'from $issue',
          ].join(' · '),
        ),
      for (final brief in board.heldBriefs)
        _UpcomingRow(
          kind: 'Held brief',
          at: brief.heldAt,
          color: tokens.warning,
          title: revealInvisible(brief.title),
          route: OoLinkTo.heldBrief(brief.operationId, brief.key),
          detail: 'Operation ${brief.operationId} · held for review',
        ),
    ];
    final briefings = onOpen(OoPage.briefings);
    return OoPanel(
      title: 'UPCOMING',
      trailing: board.heldBriefs.isNotEmpty && briefings != null
          ? TextButton(onPressed: briefings, child: const Text('Review'))
          : null,
      child: rows.isEmpty
          ? Text(
              'No Mission is waiting to run. Queued Missions and held briefs show here.',
              style: text.bodyMedium?.copyWith(color: tokens.muted),
            )
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: rows,
            ),
    );
  }
}

class _UpcomingRow extends StatelessWidget {
  const _UpcomingRow({
    required this.kind,
    required this.at,
    required this.color,
    required this.title,
    required this.detail,
    required this.route,
  });

  final OoRoute route;

  /// What the row is: `Queued Mission` or `Held brief`.
  final String kind;

  /// When it was queued or held, as Unix seconds, when the data says.
  final double? at;
  final Color color;
  final String title;
  final String detail;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final date = at == null ? null : _local(at!);
    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: IntrinsicHeight(
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              width: 50,
              padding: const EdgeInsets.symmetric(vertical: 6),
              decoration: BoxDecoration(
                color: tokens.raised,
                borderRadius: BorderRadius.circular(8),
                border: Border.all(color: tokens.border),
              ),
              child: date == null
                  ? Center(
                      child: Text(
                        '—',
                        style: TextStyle(color: tokens.muted),
                        semanticsLabel: 'no date',
                      ),
                    )
                  : Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        Text(
                          _months[date.month - 1].toUpperCase(),
                          style: text.labelSmall?.copyWith(color: tokens.muted),
                        ),
                        Text('${date.day}', style: text.titleMedium),
                      ],
                    ),
            ),
            const SizedBox(width: 10),
            Container(
              width: 3,
              decoration: BoxDecoration(
                color: color,
                borderRadius: BorderRadius.circular(2),
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    kind,
                    style: text.labelSmall?.copyWith(color: tokens.muted),
                  ),
                  OoLink(
                    title,
                    route: route,
                    maxLines: 1,
                    style: text.bodyMedium,
                  ),
                  Text(
                    detail,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: text.bodySmall?.copyWith(color: tokens.muted),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Moneypenny on Overview: her portrait, a greeting with what needs the person, quick
/// questions and a field; each opens her chat and asks. She greets only once her socket says
/// she is there; until then the card says it is connecting, or that it failed, with a retry.
class _MoneypennyCard extends StatefulWidget {
  const _MoneypennyCard({
    required this.board,
    required this.assistant,
    required this.onAsk,
    required this.waiting,
    required this.covered,
  });

  final MissionBoard board;
  final AssistantController assistant;
  final ValueChanged<String> onAsk;

  /// Whether every attention source was read: only then may she say nothing needs them.
  final bool covered;

  /// How many things need the person's attention.
  final int waiting;

  @override
  State<_MoneypennyCard> createState() => _MoneypennyCardState();
}

class _MoneypennyCardState extends State<_MoneypennyCard> {
  final _message = TextEditingController();

  @override
  void initState() {
    super.initState();
    widget.assistant.open();
  }

  @override
  void didUpdateWidget(_MoneypennyCard old) {
    super.didUpdateWidget(old);
    if (old.assistant != widget.assistant) widget.assistant.open();
  }

  @override
  void dispose() {
    _message.dispose();
    super.dispose();
  }

  void _ask(String text) {
    if (text.trim().isEmpty) return;
    widget.onAsk(text.trim());
    _message.clear();
  }

  @override
  Widget build(BuildContext context) =>
      ListenableBuilder(listenable: widget.assistant, builder: _card);

  Widget _card(BuildContext context, Widget? _) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final assistant = widget.assistant;
    final enabled = assistant.connected;
    final hour = DateTime.now().hour;
    final part = hour < 12
        ? 'morning'
        : hour < 18
        ? 'afternoon'
        : 'evening';
    final session = widget.board.userSession;
    final name =
        (session?.displayName?.trim().isNotEmpty == true
                ? session!.displayName!
                : session?.name ?? '')
            .trim()
            .split(RegExp(r'\s+'))
            .first;
    final greeting = !enabled
        ? assistant.connecting
              ? 'Connecting to Moneypenny…'
              : assistant.problem ?? 'Moneypenny is not connected.'
        : [
            'Good $part${name.isEmpty ? '' : ', $name'}.',
            switch (widget.waiting) {
              0 when !widget.covered => 'Nothing found in what loaded.',
              0 => 'Nothing needs your attention right now.',
              1 => 'One item needs your attention.',
              final count => '$count items need your attention.',
            },
            'What would you like to know?',
          ].join(' ');
    return OoPanel(
      title: 'MONEYPENNY',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              DecoratedBox(
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(16),
                  border: Border.all(color: tokens.border),
                ),
                child: const MoneypennyAvatar(size: 64),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Container(
                  padding: const EdgeInsets.all(12),
                  decoration: BoxDecoration(
                    color: tokens.raised,
                    border: Border.all(color: tokens.border),
                    borderRadius: const BorderRadius.only(
                      topLeft: Radius.circular(4),
                      topRight: Radius.circular(14),
                      bottomLeft: Radius.circular(14),
                      bottomRight: Radius.circular(14),
                    ),
                  ),
                  child: Text(greeting, style: text.bodyMedium),
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final prompt in moneypennyPrompts)
                ActionChip(
                  label: Text(prompt),
                  onPressed: enabled ? () => _ask(prompt) : null,
                ),
            ],
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _message,
            enabled: enabled,
            textInputAction: TextInputAction.send,
            onSubmitted: _ask,
            decoration: InputDecoration(
              hintText: 'Ask Moneypenny…',
              isDense: true,
              filled: true,
              fillColor: tokens.raised,
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(10),
              ),
              suffixIcon: IconButton(
                tooltip: 'Send to Moneypenny',
                onPressed: enabled ? () => _ask(_message.text) : null,
                icon: const Icon(Icons.send),
              ),
            ),
          ),
          if (!enabled && !assistant.connecting)
            Align(
              alignment: Alignment.centerRight,
              child: TextButton(
                onPressed: assistant.retry,
                child: const Text('Try again'),
              ),
            ),
        ],
      ),
    );
  }
}
