import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:outer_office/main.dart';

import 'shell_navigation.dart';

/// A board signed in with `actions` on the roster, whose gateway answers roster calls with
/// `respond` and records every request in `requests`.
MissionBoard signedIn(
  List<http.Request> requests,
  List<String> actions,
  http.Response Function(http.Request) respond,
) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      requests.add(request);
      if (request.url.path.endsWith('/schema')) {
        return http.Response('{}', 404);
      }
      if (request.url.path.startsWith('/v1/user/roster/')) {
        return respond(request);
      }
      return http.Response('[]', 200);
    }),
  );
  board.accessToken = 'operator-token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'operator-007',
    name: 'operator-007',
    displayName: null,
    groups: const [],
    expiresAt: 1e12,
    operations: const [],
    roster: actions,
  );
  board.stationBoard = StationBoardSummary.fromJson({
    'headquarters_station': 'station-a',
    'stations': [
      for (final (id, config) in [
        ('station-a', {'revision': '12'}),
        (
          'station-b',
          {
            'revision': '11',
            'refused': ['roster revision 12: Agent "009": clearance "root"'],
            'needs_restart': ['theaters.toml'],
          },
        ),
      ])
        {
          'station_id': id,
          'healthy': true,
          'headquarters': id == 'station-a',
          'seen': 1000,
          'roles': ['control', 'execution'],
          'running': 0,
          'capacity': 4,
          'operations': <String>[],
          'config': config,
        },
    ],
  });
  return board;
}

http.Response agents(http.Request request, {bool writable = true}) =>
    http.Response(
      jsonEncode({
        'kind': 'agents',
        'revision': '12',
        'writable': writable,
        'records': [
          {
            'key': '007',
            'definition': {'handler': 'claude', 'priority': 5},
          },
        ],
      }),
      200,
    );

Future<void> openRoster(WidgetTester tester, MissionBoard board) async {
  tester.view.physicalSize = const Size(800, 1200);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await openPage(tester, 'Roster');
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('a roster reader sees the records and which Stations run them', (
    tester,
  ) async {
    final requests = <http.Request>[];
    final board = signedIn(requests, ['roster.read'], agents);
    await openRoster(tester, board);

    expect(requests.first.url.path, '/v1/user/roster/agents');
    expect(find.text('007'), findsOneWidget);
    expect(
      find.text('Roster revision 12 · running on 1 of 2 Stations'),
      findsOneWidget,
    );
    expect(
      find.textContaining('station-b refused: roster revision 12'),
      findsOneWidget,
    );
    expect(
      find.text('station-b: theaters.toml changed; restart it to apply'),
      findsOneWidget,
    );
    expect(find.text('Add'), findsNothing, reason: 'roster.read edits nothing');
    expect(find.byTooltip('Edit 007'), findsNothing);
  });

  testWidgets('a read-only roster offers no edits', (tester) async {
    final requests = <http.Request>[];
    final board = signedIn(requests, [
      'roster.read',
      'roster.manage',
    ], (request) => agents(request, writable: false));
    await openRoster(tester, board);
    expect(find.textContaining('read-only'), findsOneWidget);
    expect(find.text('Add'), findsNothing);
    expect(find.byTooltip('Edit 007'), findsNothing);
  });
}
