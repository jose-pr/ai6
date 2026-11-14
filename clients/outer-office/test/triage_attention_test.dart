import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/attention_summary.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'attention_fake.dart';
import 'routes_test.dart' show locations;
import 'work_views_test.dart' show openAt;

/// A board that read the attention summary [items] and whose person may read the roster.
MissionBoard boardWith(
  List<Map<String, Object?>> items,
  List<http.Request> requests,
) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      requests.add(request);
      return http.Response('[]', 404);
    }),
  );
  board.accessToken = 'user-token';
  board.loading = false;
  board.userSession = const UserSessionSummary(
    personId: 'ana',
    name: 'ana',
    displayName: null,
    groups: [],
    expiresAt: 1_900_000_000,
    operations: [
      UserOperationAccess(operationId: 'G', actions: ['mission.read']),
    ],
    roster: ['roster.read'],
    config: {
      'station': ['config.read'],
    },
  );
  board.attention = AttentionSummary.fromJson(
    attentionBody(
      items: items,
      coverage: [
        attentionRead('reports', 'Unassessed Reports', 'checked'),
        attentionRead('charges', 'Charges', 'checked'),
      ],
    ),
  );
  return board;
}

final _items = [
  attentionEntry(
    id: 'report:r1',
    kind: 'report',
    title: 'Report "Key in the CI log" (finding) waits for an assessment in G',
    route: {'page': 'reports', 'report_id': 'r1'},
    scope: 'operation:G',
  ),
  attentionEntry(
    id: 'charge_failed:operation:G',
    kind: 'charge_failed',
    title: 'Charge of Operation G failed its last turn',
    detail: 'The last turn ended failed: no quota.',
    route: {'page': 'charges', 'operation_id': 'G'},
    scope: 'charge:operation:G',
  ),
  attentionEntry(
    id: 'charge_lapsed:station:worker',
    kind: 'charge_lapsed',
    title: 'Charge of Station worker has no Station holding it',
    route: {'page': 'charges', 'station_id': 'worker'},
    scope: 'charge:station:worker',
  ),
  {
    ...attentionEntry(
      id: 'config_refused:worker:78',
      kind: 'config_refused',
      title: 'Station worker refused a configuration (revision 78)',
      detail: 'configuration revision 78: the roster does not parse',
      route: {'page': 'stations', 'station_id': 'worker', 'tab': 'settings'},
      scope: 'station:worker',
    ),
    'revision': '78',
    'editor': {'page': 'roster', 'kind': 'station', 'station_id': 'worker'},
  },
];

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  for (final (name, size) in [
    ('desktop', const Size(1400, 1400)),
    ('phone', const Size(390, 1600)),
  ]) {
    testWidgets('Reports and charges are in the bell and on Overview, and a '
        'refusal offers its Station records, at $name width', (tester) async {
      final requests = <http.Request>[];
      final board = boardWith(_items, requests);
      addTearDown(board.dispose);
      final reported = await openAt(tester, board, '/', size: size);

      expect(find.byTooltip('Needs attention: 4'), findsOneWidget);
      expect(find.textContaining('Key in the CI log'), findsOneWidget);
      expect(
        find.textContaining('Charge of Operation G failed its last turn'),
        findsOneWidget,
      );
      expect(find.textContaining('The last turn ended failed'), findsOneWidget);
      expect(
        find.textContaining('Charge of Station worker has no Station'),
        findsOneWidget,
      );
      expect(find.textContaining('Checked charge:operation:G'), findsOneWidget);

      // The refusal's records open in the record editor, scoped to that Station.
      final editor = find.byKey(const Key('attention-editor-station:worker'));
      await tester.ensureVisible(editor);
      await tester.tap(editor);
      await tester.pumpAndSettle();
      expect(
        locations(reported).last.$1,
        '/roster?kind=station&station_id=worker',
      );
    });
  }

  for (final (name, size) in [
    ('desktop', const Size(1400, 1400)),
    ('phone', const Size(390, 1600)),
  ]) {
    testWidgets('the bell lists them and a charge opens its page, at $name '
        'width', (tester) async {
      final requests = <http.Request>[];
      final board = boardWith(_items, requests);
      addTearDown(board.dispose);
      final reported = await openAt(tester, board, '/', size: size);
      await tester.tap(find.byTooltip('Needs attention: 4'));
      await tester.pumpAndSettle();
      expect(
        find.descendant(
          of: find.byType(MenuItemButton),
          matching: find.textContaining('Key in the CI log'),
        ),
        findsOneWidget,
      );
      await tester.tap(
        find.descendant(
          of: find.byType(MenuItemButton),
          matching: find.textContaining('failed its last turn'),
        ),
      );
      await tester.pumpAndSettle();
      expect(locations(reported).last.$1, '/charges?operation_id=G');
    });
  }

  testWidgets('a Report item opens the Reports page on it', (tester) async {
    final requests = <http.Request>[];
    final board = boardWith(_items, requests);
    addTearDown(board.dispose);
    final reported = await openAt(tester, board, '/');
    await tester.tap(find.text('Open Reports'));
    await tester.pumpAndSettle();
    expect(locations(reported).last.$1, '/reports?report_id=r1');
  });

  testWidgets('the roster opens on the Station the link names', (tester) async {
    final requests = <http.Request>[];
    final board = boardWith(const [], requests);
    addTearDown(board.dispose);
    await openAt(tester, board, '/roster?kind=station&station_id=worker');
    final asked = [
      for (final request in requests)
        if (request.url.path == '/v1/user/roster/station')
          request.url.queryParameters['station'],
    ];
    expect(
      asked,
      contains('worker'),
      reason: 'its own records, not the global',
    );
  });
}
