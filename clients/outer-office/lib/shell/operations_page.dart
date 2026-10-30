import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../operations/operation_tree.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../theme/oo_backdrop.dart';
import 'oo_shell.dart' show OoPage;
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';

/// The Operations open to the person, each with its Missions on the board counted by state;
/// the route's `operation_id` shows only that one.
///
/// Operations stand under their visible parent, collapsible; a parent's card also counts the
/// working and queued Missions of its whole subtree.
class OperationsPage extends StatefulWidget {
  const OperationsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<OperationsPage> createState() => _OperationsPageState();
}

class _OperationsPageState extends State<OperationsPage> {
  /// The Operations whose subordinates are folded away.
  final _collapsed = <String>{};

  MissionBoard get board => widget.board;
  OoRoute get route => widget.route;
  ValueChanged<OoRoute> get onRoute => widget.onRoute;

  @override
  Widget build(BuildContext context) {
    final open = board.userSession?.operations ?? const [];
    final selected = route['operation_id'];
    final operations = [
      for (final operation in open)
        if (selected == null || operation.operationId == selected) operation,
    ];
    if (selected != null && operations.isEmpty) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          OoFilterBar(
            route: route,
            onRoute: onRoute,
            padding: const EdgeInsets.fromLTRB(24, 16, 24, 0),
          ),
          Expanded(
            child: Center(
              child: Text('Operation $selected is not open to this account.'),
            ),
          ),
        ],
      );
    }
    if (operations.isEmpty) {
      return const Center(
        child: OoBackdrop(
          child: Padding(
            padding: EdgeInsets.all(20),
            child: Text('No Operations are open to this account.'),
          ),
        ),
      );
    }
    final missions = groupMissions(board.sessions);
    final rows = <(OperationNode, int)>[];
    void walk(List<OperationNode> nodes, int depth) {
      for (final node in nodes) {
        rows.add((node, depth));
        if (!_collapsed.contains(node.operationId)) {
          walk(node.children, depth + 1);
        }
      }
    }

    walk(operationTree(operations), 0);
    return ListView.separated(
      padding: const EdgeInsets.all(24),
      itemCount: rows.length + 1,
      separatorBuilder: (_, index) =>
          SizedBox(height: index == 0 && !route.filtered ? 0 : 12),
      itemBuilder: (context, index) {
        if (index == 0) {
          return OoFilterBar(
            route: route,
            onRoute: onRoute,
            padding: EdgeInsets.zero,
          );
        }
        final (node, depth) = rows[index - 1];
        final operation = node.access;
        final counts = <String, int>{};
        final below = {...node.subtree};
        var working = 0;
        var queued = 0;
        for (final mission in missions) {
          if (!below.contains(mission.operationId)) continue;
          if (mission.operationId == operation.operationId) {
            counts.update(
              mission.state,
              (count) => count + 1,
              ifAbsent: () => 1,
            );
          }
          if (mission.state == 'working') working++;
          if (mission.state == 'queued') queued++;
        }
        final folded = _collapsed.contains(operation.operationId);
        return Padding(
          padding: EdgeInsets.only(left: 28.0 * depth),
          child: _OperationCard(
            operation: operation,
            counts: counts,
            subordinates: node.children.length,
            subtreeWorking: working,
            subtreeQueued: queued,
            folded: node.children.isNotEmpty ? folded : null,
            onToggle: () => setState(() {
              if (!_collapsed.remove(operation.operationId)) {
                _collapsed.add(operation.operationId);
              }
            }),
            onSubtree: () => onRoute(
              OoRoute(OoPage.missions)
                  .withField('operation_id', operation.operationId)
                  .withField('subtree', 'true'),
            ),
          ),
        );
      },
    );
  }
}

class _OperationCard extends StatelessWidget {
  const _OperationCard({
    required this.operation,
    required this.counts,
    required this.subordinates,
    required this.subtreeWorking,
    required this.subtreeQueued,
    required this.folded,
    required this.onToggle,
    required this.onSubtree,
  });

  final UserOperationAccess operation;

  /// The Operations directly under it, and the working and queued Missions of it and all
  /// beneath it.
  final int subordinates;
  final int subtreeWorking;
  final int subtreeQueued;

  /// Whether its subordinates are folded away; `null` when it has none.
  final bool? folded;
  final VoidCallback onToggle;
  final VoidCallback onSubtree;

  /// Missions by state.
  final Map<String, int> counts;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final total = counts.values.fold(0, (sum, count) => sum + count);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(18),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                if (folded case final folded?)
                  IconButton(
                    tooltip: folded
                        ? "Show ${operation.operationId}'s subordinates"
                        : "Hide ${operation.operationId}'s subordinates",
                    onPressed: onToggle,
                    icon: Icon(
                      folded ? Icons.chevron_right : Icons.expand_more,
                    ),
                  ),
                IconTheme.merge(
                  data: IconThemeData(color: tokens.accent, size: 20),
                  child: const OoIcon(OoIconName.operations),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(
                    'Operation ${operation.operationId}',
                    style: text.titleMedium,
                  ),
                ),
                Text(
                  '$total Mission${total == 1 ? '' : 's'}',
                  style: text.labelMedium,
                ),
              ],
            ),
            if (subordinates > 0) ...[
              const SizedBox(height: 8),
              Row(
                children: [
                  Expanded(
                    child: Text(
                      '$subordinates subordinate Operation'
                      '${subordinates == 1 ? '' : 's'} · whole subtree: '
                      '$subtreeWorking working, $subtreeQueued queued',
                      style: text.bodySmall,
                    ),
                  ),
                  TextButton(
                    onPressed: onSubtree,
                    child: const Text('Subtree Missions'),
                  ),
                ],
              ),
            ],
            if (counts.isNotEmpty) ...[
              const SizedBox(height: 12),
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  for (final state in boardStates)
                    if (counts[state] case final count?)
                      Chip(
                        avatar: OoStateMark(state),
                        label: Text('$state $count'),
                        visualDensity: VisualDensity.compact,
                      ),
                ],
              ),
            ],
            const SizedBox(height: 10),
            Text(
              operation.actions.isEmpty
                  ? 'No actions granted'
                  : 'Your access: ${operation.actions.join(', ')}',
              style: text.bodySmall?.copyWith(color: tokens.muted),
            ),
          ],
        ),
      ),
    );
  }
}
