import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../theme/oo_tokens.dart';
import 'mission_detail.dart';
import 'missions_board.dart';
import 'oo_shell.dart';

/// The Mission queue: the Missions a Missions route selects, newest update first unless the
/// route sorts, each row saying its state, where it runs and when the board last heard of it.
///
/// A route naming `mission_id` opens that Mission's detail beside the queue where the page is
/// wide enough, and in place of it below that; closing it returns focus to the row that
/// opened it. Briefs vetting holds and Mission proposals are not Missions: when the queue
/// shows queued Missions, a strip counts them and links to Briefings.
class MissionsPage extends StatefulWidget {
  const MissionsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    required this.desktopWindows,
    required this.pages,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;
  final bool desktopWindows;

  /// The pages this person may open.
  final List<OoPage> pages;

  @override
  State<MissionsPage> createState() => _MissionsPageState();
}

class _MissionsPageState extends State<MissionsPage> {
  SessionSummary? _selectedWebSession;

  /// The "Mission details" button of each row, by (Operation, Mission).
  final _focus = <(String, String), FocusNode>{};

  MissionBoard get board => widget.board;
  OoRoute get route => widget.route;

  FocusNode _nodeFor(String operationId, String missionId) =>
      _focus.putIfAbsent((operationId, missionId), FocusNode.new);

  @override
  void didUpdateWidget(MissionsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    final was = oldWidget.route['mission_id'];
    if (was != null && route['mission_id'] == null) {
      final node = _focus[(oldWidget.route['operation_id']!, was)];
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) node?.requestFocus();
      });
    }
  }

  @override
  void dispose() {
    for (final node in _focus.values) {
      node.dispose();
    }
    super.dispose();
  }

  void _details(MissionGroup mission) => widget.onRoute(
    route
        .withField('operation_id', mission.operationId)
        .withField('mission_id', mission.missionId),
  );

  void _openSession(BuildContext context, SessionSummary session) {
    if (widget.desktopWindows) {
      openSessionWindow(context, session, board);
    } else {
      openSessionPage(context, session, board);
    }
  }

  // Sized by the width the page has, not the window's: a docked chat takes part of it.
  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) => _page(context, constraints.maxWidth),
  );

  Widget _page(BuildContext context, double width) {
    final missionId = route['mission_id'];
    // The page's width beside the labelled rail at the wide breakpoint.
    final beside = width >= OoBreakpoints.wide - OoRail.width - 1;
    final showSessionPane = !widget.desktopWindows && beside;
    final listRoute = route.without('mission_id');
    final selected = missionId == null
        ? null
        : routeMissions(board, route).firstOrNull;
    final queue = _queue(context, listRoute, showSessionPane, selected);
    if (missionId != null && selected == null) {
      return EmptyFilter(
        text:
            'Mission $missionId of Operation ${route['operation_id']} is not on this '
            'board.',
      );
    }
    final detail = selected == null
        ? null
        : MissionDetail(
            key: ValueKey((selected.operationId, selected.missionId)),
            board: board,
            mission: selected,
            pages: widget.pages,
            onRoute: widget.onRoute,
            onOpenSession: (session) => _openSession(context, session),
            onClose: () => widget.onRoute(route.without('mission_id')),
          );
    if (!beside) return detail ?? queue;
    final session = _selectedWebSession;
    final pane = session != null && showSessionPane
        ? SessionDetailPane(
            key: ValueKey(session.sessionId),
            session: session,
            board: board,
            onClose: () => setState(() => _selectedWebSession = null),
          )
        : detail;
    if (pane == null) return queue;
    return Row(
      children: [
        Expanded(child: queue),
        const VerticalDivider(width: 1),
        SizedBox(width: 440, child: pane),
      ],
    );
  }

  Widget _queue(
    BuildContext context,
    OoRoute listRoute,
    bool showSessionPane,
    MissionGroup? open,
  ) {
    final missions = routeMissions(board, listRoute);
    final waiting = _alsoWaiting(context, listRoute);
    final boardView = CustomScrollView(
      slivers: [
        SliverToBoxAdapter(
          child: BoardHeader(
            sessions: board.sessions,
            selectedStates: listRoute.states,
            onSelectStates: (states) =>
                widget.onRoute(listRoute.withField('state', states)),
            sort: OoSortMenu(
              route: listRoute,
              onRoute: widget.onRoute,
              defaultLabel: 'Last updated, newest first',
            ),
          ),
        ),
        SliverToBoxAdapter(
          child: OoFilterBar(route: listRoute, onRoute: widget.onRoute),
        ),
        if (waiting != null) SliverToBoxAdapter(child: waiting),
        if (board.demo) const SliverToBoxAdapter(child: DemoBanner()),
        if (board.error != null)
          SliverToBoxAdapter(
            child: ErrorBanner(
              apiOrigin: board.apiOrigin,
              onRetry: board.refresh,
            ),
          ),
        if (board.loading && board.sessions.isEmpty)
          const SliverFillRemaining(
            hasScrollBody: false,
            child: Center(child: CircularProgressIndicator()),
          )
        else if (missions.isEmpty && listRoute.filtered)
          const SliverFillRemaining(
            hasScrollBody: false,
            child: EmptyFilter(text: 'No Missions match these filters.'),
          )
        else if (missions.isEmpty && board.error == null)
          const SliverFillRemaining(hasScrollBody: false, child: EmptyBoard())
        else if (missions.isEmpty && board.error != null)
          const SliverFillRemaining(
            hasScrollBody: false,
            child: GatewayUnavailable(),
          )
        else
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 36),
            sliver: SliverList.separated(
              itemCount: missions.length,
              separatorBuilder: (_, _) => const SizedBox(height: 12),
              itemBuilder: (context, index) {
                final mission = missions[index];
                return MissionCard(
                  mission: mission,
                  board: board,
                  desktopWindows: widget.desktopWindows,
                  selected:
                      open != null &&
                      open.operationId == mission.operationId &&
                      open.missionId == mission.missionId,
                  detailsFocus: _nodeFor(
                    mission.operationId,
                    mission.missionId,
                  ),
                  onDetails: () => _details(mission),
                  onSelectSession: showSessionPane
                      ? (session) =>
                            setState(() => _selectedWebSession = session)
                      : null,
                );
              },
            ),
          ),
      ],
    );
    return boardView;
  }

  /// "Also waiting: N held briefs · N Mission proposals" with a link to each, when the queue
  /// shows queued Missions and either is not zero; `null` otherwise.
  Widget? _alsoWaiting(BuildContext context, OoRoute listRoute) {
    if (!listRoute.states.contains('queued')) return null;
    final operation = listRoute['operation_id'];
    final held = board.heldBriefs
        .where((brief) => operation == null || brief.operationId == operation)
        .length;
    final proposals = board.missionProposals
        .where(
          (proposal) => operation == null || proposal.operationId == operation,
        )
        .length;
    if (held == 0 && proposals == 0) return null;
    final briefings = OoRoute(OoPage.briefings);
    OoRoute link(String tab) => operation == null
        ? briefings.withField('tab', tab)
        : briefings.withField('tab', tab).withField('operation_id', operation);
    final canOpen = widget.pages.contains(OoPage.briefings);
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 0, 24, 8),
      child: Wrap(
        crossAxisAlignment: WrapCrossAlignment.center,
        spacing: 8,
        children: [
          Text(
            'Also waiting:',
            style: Theme.of(context).textTheme.bodyMedium
                ?.copyWith(color: OoTokens.of(context).muted),
          ),
          TextButton(
            onPressed: canOpen ? () => widget.onRoute(link('held')) : null,
            child: Text('$held held ${held == 1 ? 'brief' : 'briefs'}'),
          ),
          const Text('·'),
          TextButton(
            onPressed: canOpen ? () => widget.onRoute(link('proposals')) : null,
            child: Text(
              '$proposals Mission ${proposals == 1 ? 'proposal' : 'proposals'}',
            ),
          ),
        ],
      ),
    );
  }
}
