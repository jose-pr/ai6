import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';

/// The Operations open to the person, each with its Missions on the board counted by state;
/// the route's `operation_id` shows only that one.
class OperationsPage extends StatelessWidget {
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
    return ListView.separated(
      padding: const EdgeInsets.all(24),
      itemCount: operations.length + 1,
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
        final operation = operations[index - 1];
        final counts = <String, int>{};
        for (final mission in missions.where(
          (mission) => mission.operationId == operation.operationId,
        )) {
          counts.update(mission.state, (count) => count + 1, ifAbsent: () => 1);
        }
        return _OperationCard(operation: operation, counts: counts);
      },
    );
  }
}

class _OperationCard extends StatelessWidget {
  const _OperationCard({required this.operation, required this.counts});

  final UserOperationAccess operation;

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
