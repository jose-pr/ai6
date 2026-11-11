import 'package:flutter_test/flutter_test.dart';
import 'package:outer_office/shell/attention_summary.dart';

import 'attention_fake.dart';

void main() {
  test(
    'a summary separates what holds from what has passed and names its gaps',
    () {
      final summary = AttentionSummary.fromJson(
        attentionBody(
          items: [
            attentionEntry(
              id: 'a',
              kind: 'station_dark',
              title: 'Station x has gone dark',
              route: {'page': 'stations', 'station_id': 'x'},
            ),
            attentionEntry(
              id: 'b',
              kind: 'config_refused',
              condition: 'historical',
              title: 'Station x refused a configuration (revision 78)',
              route: {'page': 'stations', 'station_id': 'x', 'tab': 'settings'},
            ),
            attentionEntry(
              id: 'c',
              kind: 'config_refused',
              condition: 'unverified',
              title: 'unsure',
              route: {'page': 'bogus'},
            ),
          ],
          coverage: [
            attentionRead('green_ink', 'Green ink requests', 'not_permitted'),
            attentionRead('held_briefs', 'Held briefs', 'unavailable'),
          ],
        ),
      );
      expect(summary.open.map((item) => item.id), ['a', 'c']);
      expect(summary.past.map((item) => item.id), ['b']);
      expect(
        summary.items[2].route,
        isNull,
        reason: 'an unknown page is no route',
      );
      expect(summary.complete, isFalse);
      expect(summary.gaps, 'Could not be read: Held briefs.');
      expect(
        summary.hidden,
        'Not visible to you, so not included: Green ink requests.',
      );
    },
  );
}
