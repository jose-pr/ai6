import 'package:flutter_test/flutter_test.dart';
import 'package:outer_office/main.dart';

void main() {
  test('a Station that reports a refused library reads as degraded', () {
    final station = StationSummary.fromJson({
      'station_id': 's-1',
      'healthy': true,
      'headquarters': false,
      'seen': 1.0,
      'roles': <String>['execution'],
      'running': 0,
      'capacity': 2,
      'operations': <String>[],
      'degraded': [
        {
          'kind': 'library',
          'subject': 'claude.dll',
          'reason': 'not loaded, refused: digest differs (ai6_agent_handler)',
          'depends': ['Agents without a loaded handler: 007'],
        },
      ],
    });
    expect(station.degraded, hasLength(1));
    expect(
      station.degraded.single.description,
      'library claude.dll: not loaded, refused: digest differs '
      '(ai6_agent_handler) (needed by Agents without a loaded handler: 007)',
    );
  });

  test('a Station that reports nothing is whole', () {
    final station = StationSummary.fromJson({
      'station_id': 's-1',
      'healthy': true,
      'headquarters': false,
      'seen': 1.0,
      'roles': <String>[],
      'running': 0,
      'capacity': 0,
      'operations': <String>[],
    });
    expect(station.degraded, isEmpty);
  });
}
