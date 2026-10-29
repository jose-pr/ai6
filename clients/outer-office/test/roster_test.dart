import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart' show FilledButton, Key;
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

    expect(requests.single.url.path, '/v1/user/roster/agents');
    expect(find.text('007'), findsOneWidget);
    expect(find.textContaining('"priority":5'), findsOneWidget);
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

  testWidgets(
    'an edit writes through the gateway and a refused one shows the reason',
    (tester) async {
      final requests = <http.Request>[];
      var refuse = true;
      final board = signedIn(requests, ['roster.read', 'roster.manage'], (
        request,
      ) {
        if (request.method == 'GET') return agents(request);
        if (refuse) {
          return http.Response(
            jsonEncode({'refused': 'Agent "007": clearance "root" is unknown'}),
            422,
          );
        }
        return http.Response(jsonEncode({'revision': '13'}), 200);
      });
      await openRoster(tester, board);

      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('roster-definition')),
        '{"handler": "claude", "clearance": "root"}',
      );
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(
        find.text('Refused: Agent "007": clearance "root" is unknown'),
        findsOneWidget,
      );
      expect(
        find.text('Save'),
        findsOneWidget,
        reason: 'the editor stays open',
      );

      await tester.enterText(
        find.byKey(const Key('roster-definition')),
        'not json',
      );
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(find.textContaining('Not JSON'), findsOneWidget);
      final sent = requests.where((request) => request.method == 'PUT').length;
      expect(sent, 1, reason: 'invalid JSON is not sent');

      refuse = false;
      await tester.enterText(
        find.byKey(const Key('roster-definition')),
        '{"handler": "claude", "priority": 9}',
      );
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      final put = requests.lastWhere((request) => request.method == 'PUT');
      expect(put.url.path, '/v1/user/roster/agents/007');
      expect(jsonDecode(put.body), {'handler': 'claude', 'priority': 9});
      expect(find.textContaining('roster revision 13'), findsOneWidget);
      expect(find.text('Save'), findsNothing, reason: 'the editor closed');
    },
  );

  testWidgets("Moneypenny's settings are listed, edited under their fixed key, removed, and a "
      'weakening the gateway returns is shown', (tester) async {
    final requests = <http.Request>[];
    var stored = true;
    final board = signedIn(requests, ['roster.read', 'roster.manage'], (
      request,
    ) {
      if (request.url.path == '/v1/user/roster/agents') {
        return agents(request);
      }
      return switch (request.method) {
        'GET' => http.Response(
          jsonEncode({
            'kind': 'assistant',
            'revision': '12',
            'writable': true,
            'records': [
              if (stored)
                {
                  'key': 'settings',
                  'definition': {
                    'personal_repo': '/srv/moneypenny/{person_digest}.git',
                  },
                },
            ],
          }),
          200,
          headers: {'content-type': 'application/json'},
        ),
        'PUT' => http.Response(
          jsonEncode({
            'revision': '13',
            'weakened': [
              "assistant.allow_elevated = true: Moneypenny's harness runs on "
                  'a Station running elevated or as root',
            ],
          }),
          200,
          headers: {'content-type': 'application/json'},
        ),
        _ => () {
          stored = false;
          return http.Response(
            jsonEncode({'revision': '14'}),
            200,
            headers: {'content-type': 'application/json'},
          );
        }(),
      };
    });
    await openRoster(tester, board);

    await tester.tap(find.text('Agents'));
    await tester.pumpAndSettle();
    await tester.tap(find.text("Moneypenny's settings").last);
    await tester.pumpAndSettle();
    expect(requests.last.url.path, '/v1/user/roster/assistant');
    expect(find.text('settings'), findsOneWidget);
    expect(find.textContaining('person_digest'), findsOneWidget);
    expect(
      find.text('Add'),
      findsNothing,
      reason: 'the one record exists, so it is edited, not added',
    );

    await tester.tap(find.byTooltip('Edit settings'));
    await tester.pumpAndSettle();
    expect(
      find.byKey(const Key('roster-key')),
      findsNothing,
      reason: 'the key is fixed',
    );
    expect(find.textContaining('Key: assistant'), findsOneWidget);
    await tester.enterText(
      find.byKey(const Key('roster-definition')),
      '{"personal_repo": "/srv/moneypenny/{person_digest}.git", '
      '"allow_elevated": true}',
    );
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final put = requests.lastWhere((request) => request.method == 'PUT');
    expect(put.url.path, '/v1/user/roster/assistant/settings');
    expect(jsonDecode(put.body)['allow_elevated'], true);
    expect(find.textContaining('roster revision 13'), findsOneWidget);
    expect(
      find.text("Moneypenny's settings now weaken these defaults:"),
      findsOneWidget,
    );
    expect(
      find.textContaining('assistant.allow_elevated = true'),
      findsOneWidget,
    );

    await tester.tap(find.byTooltip('Remove settings'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Remove'));
    await tester.pumpAndSettle();
    final delete = requests.lastWhere((request) => request.method == 'DELETE');
    expect(delete.url.path, '/v1/user/roster/assistant/settings');
    expect(
      find.textContaining('assistant.allow_elevated = true'),
      findsNothing,
      reason: 'the defaults weaken nothing',
    );
    expect(find.text('No records of this kind.'), findsOneWidget);

    await tester.tap(find.text('Add'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('roster-key')), findsNothing);
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(
      requests.lastWhere((request) => request.method == 'PUT').url.path,
      '/v1/user/roster/assistant/settings',
      reason: 'a new record of this kind is keyed assistant',
    );
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
