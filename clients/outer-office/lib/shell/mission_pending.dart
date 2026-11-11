import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_tokens.dart';
import 'oo_freshness.dart';

/// Where a Mission with no Session stands, in words: what it is waiting for or retrying, and
/// why, as the Registry and its Station said. [compact] is one line of each, for a table cell.
class MissionPendingNote extends StatelessWidget {
  const MissionPendingNote({
    required this.pending,
    this.compact = false,
    super.key,
  });

  final PendingMission pending;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final text = Theme.of(context).textTheme;
    final muted = text.bodySmall?.copyWith(color: OoTokens.of(context).muted);
    final now = ooClock().millisecondsSinceEpoch / 1000;
    final holder = [
      if (pending.agent != null) 'Agent ${pending.agent}',
      if (pending.station != null) 'Station ${pending.station}',
    ].join(' · ');
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(
          pending.describe(now),
          style: text.labelMedium,
          maxLines: compact ? 1 : null,
          overflow: compact ? TextOverflow.ellipsis : null,
        ),
        if (pending.reason case final reason?)
          Text(
            reason,
            style: muted,
            maxLines: compact ? 1 : null,
            overflow: compact ? TextOverflow.ellipsis : null,
          ),
        if (!compact && holder.isNotEmpty) Text(holder, style: muted),
      ],
    );
  }
}
