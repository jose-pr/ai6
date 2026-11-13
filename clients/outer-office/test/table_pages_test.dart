import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/widgets/oo_data_table.dart';

import 'access_test.dart' as access;
import 'pools_test.dart' as pools;
import 'timeline_test.dart' as timeline;
import 'work_views_test.dart' show demoBoard, openAt;

/// The ids of the rows the table shows, top to bottom.
List<String> _rows(WidgetTester tester) => [
  for (final material in tester.widgetList<Material>(
    find.byWidgetPredicate(
      (widget) =>
          widget is Material &&
          widget.key is ValueKey &&
          '${(widget.key! as ValueKey).value}'.startsWith('row-'),
    ),
  ))
    '${(material.key! as ValueKey).value}'.substring('row-'.length),
];

http.Response _json(Object? body) => http.Response(
  jsonEncode(body),
  200,
  headers: {'content-type': 'application/json'},
);

MissionBoard _signedIn(
  http.Response Function(http.Request request) answer, {
  required List<String> actions,
  List<String> operations = const ['G', 'L'],
  List<String> global = const [],
}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async => answer(request)),
  );
  board.accessToken = 'token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'p-1',
    name: 'p-1',
    displayName: null,
    groups: const [],
    expiresAt: 1e12,
    operations: [
      for (final id in operations)
        UserOperationAccess(operationId: id, actions: actions),
    ],
    roster: const [],
    globalActions: global,
  );
  addTearDown(board.dispose);
  return board;
}

void main() {
  testWidgets('Missions: a wide queue is a table that hides reviewer turns', (
    tester,
  ) async {
    OoTablePreferences.missionsAsTable.value = true;
    addTearDown(() => OoTablePreferences.missionsAsTable.value = false);
    final board = demoBoard();
    board.runs = [
      ...board.runs,
      RunSummary(
        operationId: 'G',
        missionId: 'mission-review',
        runId: 'review-run',
        state: 'done',
        sequence: 1,
        updatedAt: 1,
      ),
    ];
    board.missionBriefs = [
      ...board.missionBriefs,
      const MissionBriefSummary(
        operationId: 'G',
        missionId: 'mission-review',
        prompt: 'Review the output of Mission mission-007',
        handlerProvider: 'claude',
        terminalProvider: 'pipe',
        workingDirectory: '/work',
        environmentKeys: [],
        agentId: null,
        roleId: null,
        model: null,
        effort: null,
        origin: BriefOrigin(origin: 'review', receivedBy: 'hq'),
      ),
    ];
    await openAt(tester, board, '/missions', size: const Size(1800, 1200));
    expect(
      _rows(tester),
      unorderedEquals(['G/mission-007', 'L/mission-021']),
      reason: 'the reviewer turn is not a Mission of its own on the board',
    );

    await tester.tap(find.byKey(const ValueKey('header-operation')));
    await tester.pumpAndSettle();
    expect(_rows(tester), ['G/mission-007', 'L/mission-021']);
    await tester.tap(find.byKey(const ValueKey('header-operation')));
    await tester.pumpAndSettle();
    expect(_rows(tester), ['L/mission-021', 'G/mission-007']);

    await tester.tap(find.byKey(const ValueKey('select-all')));
    await tester.pumpAndSettle();
    expect(find.text('2 selected'), findsOneWidget);
    for (final label in ['Cancel', 'Retry', 'Resume']) {
      expect(find.widgetWithText(OutlinedButton, label), findsOneWidget);
    }
  });

  testWidgets('Missions: a link can ask for the reviewer turns', (
    tester,
  ) async {
    OoTablePreferences.missionsAsTable.value = true;
    addTearDown(() => OoTablePreferences.missionsAsTable.value = false);
    final board = demoBoard();
    board.runs = [
      ...board.runs,
      RunSummary(
        operationId: 'G',
        missionId: 'mission-review',
        runId: 'review-run',
        state: 'done',
        sequence: 1,
        updatedAt: 1,
      ),
    ];
    board.missionBriefs = [
      ...board.missionBriefs,
      const MissionBriefSummary(
        operationId: 'G',
        missionId: 'mission-review',
        prompt: 'Review',
        handlerProvider: 'claude',
        terminalProvider: 'pipe',
        workingDirectory: '/work',
        environmentKeys: [],
        agentId: null,
        roleId: null,
        model: null,
        effort: null,
        origin: BriefOrigin(origin: 'review', receivedBy: 'hq'),
      ),
    ];
    await openAt(
      tester,
      board,
      '/missions?reviews=show',
      size: const Size(1800, 1200),
    );
    expect(_rows(tester), contains('G/mission-review'));
  });

  testWidgets('Runs: search narrows the table and goes to the link', (
    tester,
  ) async {
    final reported = await openAt(
      tester,
      demoBoard(),
      '/runs',
      size: const Size(1800, 1200),
    );
    expect(_rows(tester).length, 3);
    await tester.enterText(
      find.byKey(const ValueKey('table-search')),
      'run-021',
    );
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pumpAndSettle();
    expect(_rows(tester), ['preview-run-021']);
    expect(
      reported.any((call) => '${call.arguments}'.contains('text=run-021')),
      isTrue,
    );
  });

  testWidgets('Agents: with All Operations one Agent is one row', (
    tester,
  ) async {
    final board = _signedIn((request) {
      if (request.url.path.endsWith('/agents')) {
        return _json([
          {
            'agent': '007',
            'posted': true,
            'operation': {'done': 3},
            'every_operation': <String, Object?>{},
            'by_clearance': <String, Object?>{},
            'clearance': request.url.path.contains('/G/') ? 'field' : 'desk',
            'routes': [
              {
                'route': {'handler': 'claude', 'model': 'sonnet'},
              },
            ],
          },
        ]);
      }
      return http.Response('[]', 200);
    }, actions: ['agent.read']);
    await openAt(tester, board, '/agents', size: const Size(1800, 1200));
    expect(_rows(tester), ['007'], reason: 'once, not once per Operation');
    expect(find.text('field'), findsOneWidget);
    expect(find.text('desk'), findsOneWidget);
    expect(find.text('G'), findsWidgets);
    expect(find.text('L'), findsWidgets);
  });

  testWidgets('Pools: one account is one row naming its Operations', (
    tester,
  ) async {
    final board = _signedIn((request) {
      if (request.url.path.endsWith('/pools')) {
        return _json({
          'operation_id': request.url.pathSegments[3],
          'accounts': [
            pools.account('alpha', used: 400, tokens: 12),
            pools.account('beta'),
          ],
        });
      }
      return http.Response('[]', 200);
    }, actions: ['pool.read']);
    await openAt(tester, board, '/pools', size: const Size(1800, 1200));
    expect(_rows(tester), unorderedEquals(['team/alpha', 'team/beta']));
    await tester.tap(find.byKey(const ValueKey('header-pool')));
    await tester.pumpAndSettle();
    expect(_rows(tester), ['team/beta', 'team/alpha']);
  });

  testWidgets('Timeline: events sort by time and narrow by search', (
    tester,
  ) async {
    final fake = timeline.FakeTimeline();
    final board = await timeline.signedIn(fake);
    addTearDown(board.dispose);
    await openAt(tester, board, '/timeline', size: const Size(1800, 1200));
    expect(_rows(tester), ['event:e5', 'event:e4'], reason: 'newest first');
    await tester.tap(find.byKey(const ValueKey('header-when')));
    await tester.pumpAndSettle();
    expect(_rows(tester), ['event:e4', 'event:e5']);
    await tester.enterText(
      find.byKey(const ValueKey('table-search')),
      'resume',
    );
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pumpAndSettle();
    expect(_rows(tester), ['event:e5']);
  });

  testWidgets('Access: rules sort by priority and accounts list people seen', (
    tester,
  ) async {
    final board = access.board(
      [],
      ['access.read', 'access.manage', 'account.manage'],
      (request) {
        if (request.url.path == '/v1/user/accounts') {
          return access.json([
            {'name': 'svc', 'groups': <String>[], 'disabled': false},
          ]);
        }
        return access.json({
          'revision': 'r1',
          'writable': true,
          'roles': [
            {
              'name': 'chief',
              'built_in': true,
              'editable': false,
              'actions': ['access.read'],
            },
            {
              'name': 'operator',
              'built_in': false,
              'editable': true,
              'actions': ['mission.read'],
            },
          ],
          'rules': [
            {
              'subject': 'group:ops',
              'scope': 'operation:G',
              'role': 'operator',
            },
            {'subject': 'person:1000', 'scope': '*', 'role': 'operator'},
          ],
          'delegable': {
            'actions': ['mission.read'],
            'default': true,
            'weakenings': <String>[],
          },
          'chief': {'stored': <String>[], 'host': <String>[]},
        });
      },
    );
    await openAt(tester, board, '/access?tab=rules');
    expect(_rows(tester), ['0', '1']);
    await tester.tap(find.byKey(const ValueKey('header-subject')));
    await tester.pumpAndSettle();
    expect(find.text('group:ops'), findsOneWidget);
    expect(find.text('1000'), findsOneWidget, reason: 'never seen: the id');
    expect(find.byTooltip('Move rule 1 up'), findsOneWidget);
  });

  testWidgets(
    'Access: accounts list local accounts and the people who signed in',
    (tester) async {
      final board = _signedIn(
        (request) {
          if (request.url.path == '/v1/user/accounts') {
            return _json([
              {'name': 'svc', 'groups': <String>[], 'disabled': false},
            ]);
          }
          if (request.url.path == '/v1/user/principals') {
            return _json({
              'people': [
                {
                  'id': 'person:1000',
                  'name': 'ada',
                  'label': 'Ada Lovelace',
                  'groups': ['analysts'],
                  'last_seen': 1.0,
                },
              ],
              'groups': <String>[],
            });
          }
          return http.Response('{}', 404);
        },
        actions: [],
        operations: const [],
        global: ['access.manage', 'account.manage'],
      );
      await openAt(tester, board, '/access?tab=accounts');
      expect(
        _rows(tester),
        unorderedEquals(['account:svc', 'person:person:1000']),
      );
      expect(find.text('Ada Lovelace'), findsOneWidget);
      expect(find.text('analysts'), findsOneWidget);
      expect(find.text('Directory'), findsOneWidget);
      expect(find.text('Local, enabled'), findsOneWidget);
    },
  );
}
