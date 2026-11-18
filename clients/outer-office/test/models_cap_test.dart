import 'package:flutter_test/flutter_test.dart';
import 'package:outer_office/pools/models_section.dart';

void main() {
  test('a route with a token cap says so and one without says nothing', () {
    final capped = DiscoveredModels.fromJson({
      'station': 's',
      'source': '007/claude/a',
      'handler': 'claude',
      'token_cap': 300000,
      'listing': {'known': true, 'models': <dynamic>[]},
    });
    final open = DiscoveredModels.fromJson({
      'station': 's',
      'source': '007/claude/b',
      'handler': 'claude',
      'listing': {'known': true, 'models': <dynamic>[]},
    });
    expect(capped.capLabel, 'Capped at 300000 tokens a request');
    expect(open.tokenCap, isNull);
    expect(open.capLabel, isEmpty);
  });
}
