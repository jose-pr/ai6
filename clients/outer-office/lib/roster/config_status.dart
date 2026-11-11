import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_theme.dart';

/// Where the Stations stand with a configuration revision: how many run it, which wait for a
/// restart to take a changed file, and which refused it, with the reason each gave.
///
/// With a [station], only that Station is looked at: the records shown are the ones it has of
/// its own.
class ConfigStatusBanner extends StatelessWidget {
  const ConfigStatusBanner({
    required this.revision,
    required this.writable,
    required this.stations,
    this.station,
    super.key,
  });

  /// The revision of the records shown, in decimal.
  final String revision;
  final bool writable;
  final List<StationSummary> stations;

  /// The one Station asked about; null asks about every Station.
  final String? station;

  @override
  Widget build(BuildContext context) {
    final reporting = [
      for (final summary in stations)
        if ((station == null || summary.stationId == station) &&
            summary.config != null)
          (summary.stationId, summary.config!),
    ];
    final current = reporting
        .where((entry) => entry.$2.revision == revision)
        .length;
    final small = Theme.of(context).textTheme.bodySmall;
    final tokens = OoTokens.of(context);
    return Column(
      key: const Key('config-status'),
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          [
            'Roster revision $revision',
            if (reporting.isNotEmpty)
              'running on $current of ${reporting.length} '
                  '${reporting.length == 1 && station != null ? 'Station' : 'Stations'}',
            if (!writable) 'read-only: edit the configuration files',
          ].join(' · '),
        ),
        for (final (id, config) in reporting) ...[
          for (final reason in config.refused)
            Text(
              '$id refused: $reason',
              key: Key('config-refused-$id'),
              style: small?.copyWith(color: tokens.danger),
            ),
          for (final file in config.needsRestart)
            Text(
              '$id: $file changed; restart it to apply',
              key: Key('config-restart-$id'),
              style: small?.copyWith(color: tokens.warning),
            ),
        ],
      ],
    );
  }
}
