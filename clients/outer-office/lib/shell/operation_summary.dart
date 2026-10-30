import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_route.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_theme.dart';
import '../timeline/timeline_page.dart';
import 'missions_board.dart';
import 'oo_freshness.dart';
import 'oo_shell.dart';
import 'overview_page.dart' show timelineKindIcon, timelineSentence;

/// One Operation at a glance: what the person may do there, its Missions counted by state
/// (each count a link to the Missions with that state), the held briefs and Mission proposals
/// waiting, and its newest activity.
///
/// Only what the board and the Operation's timeline already hold: the share of Missions done,
/// no forecast and no health score.
class OperationSummary extends StatefulWidget {
  const OperationSummary({
    required this.board,
    required this.operation,
    required this.pages,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final UserOperationAccess operation;

  /// The pages this person may open; a link goes only to one of these.
  final List<OoPage> pages;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<OperationSummary> createState() => _OperationSummaryState();
}

class _OperationSummaryState extends State<OperationSummary> {
  late Future<TimelinePageSummary>? _timeline = _load();

  String get _id => widget.operation.operationId;

  /// The newest items of its timeline, only for a person who may read it.
  Future<TimelinePageSummary>? _load() =>
      widget.board.timelineOperations.contains(_id)
      ? widget.board.loadTimeline(
          operationId: _id,
          limit: widget.board.limits.pages.summary,
        )
      : null;

  @override
  void didUpdateWidget(OperationSummary oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.operation.operationId != _id) _timeline = _load();
  }

  OoRoute _missions([List<String> states = const []]) =>
      OoRoute(OoPage.missions)
          .withField('operation_id', _id)
          .withField('state', states);

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final board = widget.board;
    final counts = <String, int>{};
    for (final mission in groupMissions(board.sessions)) {
      if (mission.operationId != _id) continue;
      counts.update(mission.state, (count) => count + 1, ifAbsent: () => 1);
    }
    final total = counts.values.fold(0, (sum, count) => sum + count);
    final done = counts['done'] ?? 0;
    final canWatchBriefs =
        board.proposalOperations.contains(_id) &&
        widget.pages.contains(OoPage.briefings);
    final held = board.heldBriefs.where((b) => b.operationId == _id).length;
    final proposals = board.missionProposals
        .where((proposal) => proposal.operationId == _id)
        .length;
    final briefings = OoRoute(OoPage.briefings);
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Row(
          children: [
            IconTheme.merge(
              data: IconThemeData(color: tokens.accent, size: 22),
              child: const OoIcon(OoIconName.operations),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Semantics(
                header: true,
                child: Text('Operation $_id', style: text.titleLarge),
              ),
            ),
          ],
        ),
        const SizedBox(height: 14),
        Text('YOUR ACCESS', style: ooSectionLabel(context)),
        const SizedBox(height: 6),
        if (widget.operation.actions.isEmpty)
          Text('No actions granted', style: TextStyle(color: tokens.muted))
        else
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (final action in widget.operation.actions)
                Chip(label: Text(action), visualDensity: VisualDensity.compact),
            ],
          ),
        const SizedBox(height: 20),
        Text('MISSIONS', style: ooSectionLabel(context)),
        const SizedBox(height: 6),
        if (total == 0)
          Text(
            'No Missions of this Operation are on the board.',
            style: TextStyle(color: tokens.muted),
          )
        else ...[
          Text(
            '$done of $total ${total == 1 ? 'Mission' : 'Missions'} done',
            style: text.bodyMedium,
          ),
          const SizedBox(height: 6),
          LinearProgressIndicator(value: done / total),
          const SizedBox(height: 10),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final state in boardStates)
                if (counts[state] case final count?)
                  ActionChip(
                    key: ValueKey('count-$state'),
                    avatar: OoStateMark(state),
                    label: Text('$state $count'),
                    tooltip: 'Show the $state Missions of $_id',
                    onPressed: () => widget.onRoute(_missions([state])),
                  ),
            ],
          ),
        ],
        if (canWatchBriefs) ...[
          const SizedBox(height: 20),
          Text('WAITING', style: ooSectionLabel(context)),
          Wrap(
            spacing: 8,
            children: [
              TextButton(
                onPressed: () => widget.onRoute(
                  briefings
                      .withField('tab', 'held')
                      .withField('operation_id', _id),
                ),
                child: Text('$held held ${held == 1 ? 'brief' : 'briefs'}'),
              ),
              TextButton(
                onPressed: () => widget.onRoute(
                  briefings
                      .withField('tab', 'proposals')
                      .withField('operation_id', _id),
                ),
                child: Text(
                  '$proposals Mission '
                  '${proposals == 1 ? 'proposal' : 'proposals'}',
                ),
              ),
            ],
          ),
        ],
        if (_timeline case final timeline?) ...[
          const SizedBox(height: 20),
          Row(
            children: [
              Expanded(
                child: Text('RECENT ACTIVITY', style: ooSectionLabel(context)),
              ),
              if (widget.pages.contains(OoPage.timeline))
                TextButton(
                  onPressed: () => widget.onRoute(
                    OoRoute(OoPage.timeline).withField('operation_id', _id),
                  ),
                  child: const Text('View all'),
                ),
            ],
          ),
          FutureBuilder<TimelinePageSummary>(
            future: timeline,
            builder: (context, snapshot) {
              if (snapshot.hasError) {
                return Text(
                  'Recent activity could not be loaded.',
                  style: TextStyle(color: tokens.muted),
                );
              }
              final items = snapshot.data?.items;
              if (items == null) return const LinearProgressIndicator();
              if (items.isEmpty) {
                return Text(
                  'No activity recorded yet.',
                  style: TextStyle(color: tokens.muted),
                );
              }
              return Column(
                children: [
                  for (final item in items.take(
                    widget.board.limits.pages.summary,
                  ))
                    ListTile(
                      dense: true,
                      contentPadding: EdgeInsets.zero,
                      leading: OoDuotone(timelineKindIcon(item.kind), size: 26),
                      title: Text(timelineSentence(item.action)),
                      subtitle: item.context.isEmpty
                          ? null
                          : Text(item.context),
                      trailing: OoTime(item.at),
                    ),
                ],
              );
            },
          ),
        ],
      ],
    );
  }
}
