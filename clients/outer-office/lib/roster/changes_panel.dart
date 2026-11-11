import 'package:material_ui/material_ui.dart';

import '../forms/form_tools.dart';

/// What a save would change in the record, key by key against the record as stored, or the
/// record as the configuration files would hold it.
class ChangesPanel extends StatefulWidget {
  const ChangesPanel({
    required this.changes,
    required this.configuration,
    super.key,
  });

  final List<RecordChange> changes;

  /// The record as configuration text; asked only while the raw view is open.
  final String Function() configuration;

  @override
  State<ChangesPanel> createState() => _ChangesPanelState();
}

class _ChangesPanelState extends State<ChangesPanel> {
  bool _raw = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final mono = theme.textTheme.bodySmall?.copyWith(fontFamily: 'monospace');
    final count = widget.changes.length;
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: ExpansionTile(
        key: const Key('record-changes'),
        tilePadding: EdgeInsets.zero,
        title: Text(count == 0 ? 'No changes yet' : 'Review changes ($count)'),
        childrenPadding: const EdgeInsets.only(bottom: 8),
        expandedCrossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              TextButton(
                key: const Key('record-changes-diff'),
                onPressed: _raw ? () => setState(() => _raw = false) : null,
                child: const Text('Changes'),
              ),
              TextButton(
                key: const Key('record-changes-raw'),
                onPressed: _raw ? null : () => setState(() => _raw = true),
                child: const Text('As configuration'),
              ),
            ],
          ),
          if (_raw)
            SelectableText(widget.configuration(), style: mono)
          else if (count == 0)
            Text('Nothing differs from the stored record.', style: mono)
          else
            for (final change in widget.changes)
              Text(
                change.line,
                key: Key('record-change-${change.path}'),
                style: mono?.copyWith(
                  color: switch (change.kind) {
                    ChangeKind.added => theme.colorScheme.primary,
                    ChangeKind.removed => theme.colorScheme.error,
                    ChangeKind.changed => null,
                  },
                ),
              ),
        ],
      ),
    );
  }
}
