import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/agents/agents_page.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/roster/roster_page.dart';
import 'package:outer_office/shell/oo_link.dart';
import 'package:outer_office/shell/operation_summary.dart';

import 'shell_navigation.dart';

/// A board signed in with `actions` on the roster, whose gateway answers roster calls with
/// `respond` and records every request in `requests`.
MissionBoard signedIn(
  List<http.Request> requests,
  List<String> actions,
  http.Response Function(http.Request) respond, {
  List<UserOperationAccess> operations = const [],
}) {
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
    operations: operations,
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

/// Narrows the roster to the kind called [label].
Future<void> pickKind(WidgetTester tester, String label) async {
  await tester.tap(find.byType(DropdownButton<String>));
  await tester.pumpAndSettle();
  await tester.tap(find.text(label).last);
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('a roster reader sees the records and which Stations run them', (
    tester,
  ) async {
    final requests = <http.Request>[];
    final board = signedIn(requests, ['roster.read'], agents);
    await openRoster(tester, board);
    await pickKind(tester, 'Agents');

    expect(
      requests.map((request) => request.url.path),
      contains('/v1/user/roster/agents'),
    );
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
    await pickKind(tester, 'Agents');
    expect(find.textContaining('read-only'), findsOneWidget);
    expect(find.text('Add'), findsNothing);
    expect(find.byTooltip('Edit 007'), findsNothing);
  });

  testWidgets('All records is the default, each kind a section of links', (
    tester,
  ) async {
    final requests = <http.Request>[];
    final board = signedIn(requests, ['roster.read'], agents);
    await openRoster(tester, board);

    expect(find.text('All records'), findsOneWidget);
    expect(
      requests.map((request) => request.url.pathSegments.last).toSet(),
      containsAll(['operations', 'agents', 'roles', 'covers', 'assistant']),
    );
    expect(find.text('Agents (1)'), findsOneWidget);
    await tester.tap(find.text('007').first);
    await tester.pumpAndSettle();
    final page = tester.widget<RosterPage>(find.byType(RosterPage));
    expect(page.route['kind'], 'operation');
    expect(page.route['key'], '007');
  });

  group('a row opens what it names', () {
    /// The roster with one record of each kind, open at the kind called [label], for a person
    /// holding [actions] in Operation G.
    Future<void> open(
      WidgetTester tester,
      String label, {
      List<String> actions = const ['agent.read', 'mission.read'],
    }) async {
      final board = signedIn(
        [],
        ['roster.read'],
        (request) => http.Response(
          jsonEncode({
            'kind': request.url.pathSegments.last,
            'revision': '12',
            'writable': false,
            'records': [
              {
                'key': switch (request.url.pathSegments.last) {
                  'operations' => 'G',
                  'roles' => 'reviewer',
                  'covers' => 'night',
                  _ => '007',
                },
                'definition': <String, Object>{},
              },
            ],
          }),
          200,
        ),
        operations: [UserOperationAccess(operationId: 'G', actions: actions)],
      );
      await openRoster(tester, board);
      await pickKind(tester, label);
    }

    testWidgets('an Agent opens the Agents page filtered to it', (
      tester,
    ) async {
      await open(tester, 'Agents');
      await tester.tap(find.text('007'));
      await tester.pumpAndSettle();
      final page = tester.widget<AgentsPage>(find.byType(AgentsPage));
      expect(page.route['agent'], '007');
      expect(find.text('Agent: 007'), findsOneWidget);
    });

    testWidgets('an Operation opens that Operation', (tester) async {
      await open(tester, 'Operations');
      await tester.tap(find.text('G').last);
      await tester.pumpAndSettle();
      final summary = tester.widget<OperationSummary>(
        find.byType(OperationSummary),
      );
      expect(summary.operation.operationId, 'G');
    });

    testWidgets('a role opens the Roles tab', (tester) async {
      await open(tester, 'Agent roles');
      await tester.tap(find.text('reviewer'));
      await tester.pumpAndSettle();
      final page = tester.widget<AgentRolesPage>(find.byType(AgentRolesPage));
      expect(page.route['tab'], 'roles');
      expect(page.route['role'], 'reviewer');
    });

    testWidgets('a cover policy has no page to open', (tester) async {
      await open(tester, 'Cover policies');
      expect(find.text('night'), findsOneWidget);
      expect(find.byType(OoLink), findsNothing);
    });

    testWidgets('where the person may not open the page it is plain text', (
      tester,
    ) async {
      await open(tester, 'Agents', actions: const ['mission.read']);
      await tester.tap(find.text('007'));
      await tester.pumpAndSettle();
      expect(find.byType(RosterPage), findsOneWidget);
      expect(find.byType(AgentsPage), findsNothing);
    });
  });
}
