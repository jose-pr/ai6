import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';

/// The Operations open to the person, each with its Missions on the board counted by state.
class OperationsPage extends StatelessWidget {
  const OperationsPage({required this.board, super.key});

  final MissionBoard board;

  @override
  Widget build(BuildContext context) {
    final operations = board.userSession?.operations ?? const [];
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
      itemCount: operations.length,
      separatorBuilder: (_, _) => const SizedBox(height: 12),
      itemBuilder: (context, index) {
        final operation = operations[index];
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
                  for (final state in missionStates)
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
