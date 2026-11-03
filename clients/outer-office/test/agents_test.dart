import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart'
    show ActionChip, DropdownButton, Tab, TextField;
import 'package:outer_office/agents/agents_page.dart' show AgentSummary;
import 'package:outer_office/main.dart';

import 'shell_navigation.dart';

const _agents = [
  {
    'agent': '007',
    'posted': true,
    'operation': {
      'done': 12,
      'failed': 1,
      'blown': 0,
      'abandoned': 0,
      'stalled': 2,
      'lost': 1,
      'quarantined': 0,
      'budget_stops': 0,
      'green_ink_rejected': 0,
      'last': {},
    },
    'every_operation': {'quarantined': 1},
    'by_clearance': {},
    'last_mission_at': 1_900_000_000.0,
    'model_history': [
      {'model': 'opus', 'from': 1_800_000_000.0, 'to': 1_850_000_000.0},
      {'model': 'sonnet', 'from': 1_850_000_000.0, 'to': null},
    ],
    'retirement': {
      'retire_on': '2030-01-31',
      'on_retire': 'desk',
      'status': 'retiring',
    },
    'clearance': 'field',
    'configured_clearance': 'desk',
    'grant': {
      'agent': '007',
      'operation': 'G',
      'clearance': 'field',
      'signer': 'alice',
      'at': 1_850_000_000.0,
      'approval': 'green-ink-1',
    },
    'promotion': {
      'id': 'green-ink-2',
      'scope': {'kind': 'clearance', 'target': '007:00@G'},
    },
  },
  {
    'agent': '008',
    'posted': true,
    'operation': {'done': 0},
    'holds': [
      {
        'operation': 'G',
        'agent': '008',
        'reason': 'failing',
        'by': 'm',
        'at': 1.0,
      },
      {
        'operation': 'G',
        'agent': '008',
        'route': {'handler': 'claude', 'model': 'opus', 'pool': 'team'},
        'reason': 'slow',
        'by': 'alice',
        'at': 1.0,
      },
    ],
    'model_history': [],
    'clearance': 'desk',
    'configured_clearance': 'desk',
    'grant_unreadable': true,
  },
  {
    'agent': '009',
    'posted': true,
    'operation': {'done': 3},
    'routes': [
      {'handler': 'claude', 'model': 'fable'},
    ],
    'model_history': [
      {'model': 'sonnet', 'from': 1_850_000_000.0, 'to': null},
    ],
  },
];

const _scorecard = {
  'window_days': 30,
  'small_sample': 5,
  'unattributed': 1,
  'agents': [
    {
      'agent': '007',
      'standing': {
        'attempts': {
          'done': [
            {'mission_id': 'm-1', 'session_id': 'm-1.1'},
            {'mission_id': 'm-2', 'session_id': 'm-2.1'},
            {'mission_id': 'm-3', 'session_id': 'm-3.1'},
          ],
          'failed': [
            {'mission_id': 'm-4', 'session_id': 'm-4.1'},
          ],
          'lost': [
            {'mission_id': 'm-5', 'session_id': 'm-5.1'},
          ],
        },
        'judged': 4,
        'failure_rate': 0.25,
        'small_sample': true,
        'refused_tools': 2,
        'usage': {'input': 1000, 'output': 500, 'cost_micro_usd': 1_230_000},
        'wall_secs': 720.0,
      },
      'routes': [
        {
          'route': {'handler': 'claude', 'model': 'sonnet', 'pool': 'team'},
          'standing': {
            'attempts': {
              'done': [
                {'mission_id': 'm-1', 'session_id': 'm-1.1'},
              ],
            },
            'judged': 1,
            'failure_rate': 0.0,
            'small_sample': true,
            'usage': {'input': 0, 'output': 0, 'cost_micro_usd': 0},
            'wall_secs': 0.0,
          },
        },
      ],
    },
  ],
  'models': [
    {
      'model': null,
      'standing': {
        'attempts': {},
        'judged': 0,
        'small_sample': true,
        'usage': {'input': 0, 'output': 0, 'cost_micro_usd': 0},
        'wall_secs': 0.0,
      },
    },
  ],
};

/// A board whose person holds `actions` on Operation G, answering the Agents routes and
/// recording every request in `requests`. With `roster.read` the scorecard answers too.
MissionBoard signedIn(
  List<http.Request> requests,
  List<String> actions, {
  List<String> roster = const [],
}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      requests.add(request);
      final path = request.url.path;
      if (path == '/v1/user/operations/G/agents') {
        return http.Response(
          jsonEncode(_agents),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      if (path == '/v1/user/operations/G/agents/scorecard') {
        return http.Response(
          jsonEncode(_scorecard),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      if (path.endsWith('/clearance')) {
        return http.Response(
          jsonEncode({
            'agent': '007',
            'operation': 'G',
            'clearance': 'desk',
            'signer': 'operator-007',
            'at': 1.0,
          }),
          200,
          headers: {'content-type': 'application/json'},
        );
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
    operations: [UserOperationAccess(operationId: 'G', actions: actions)],
    roster: roster,
  );
  return board;
}

Future<void> openAgents(WidgetTester tester, MissionBoard board) async {
  tester.view.physicalSize = const Size(800, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await openPage(tester, 'Agents');
  await tester.tap(find.widgetWithText(Tab, 'Agents'));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'an Agent reader sees records, clearance, retirement and promotion',
    (tester) async {
      final requests = <http.Request>[];
      final board = signedIn(requests, ['agent.read']);
      await openAgents(tester, board);

      expect(
        requests.map((request) => request.url.path),
        contains('/v1/user/operations/G/agents'),
      );
      expect(
        find.text('12 done · 1 failed · 2 stalled · 1 lost'),
        findsOneWidget,
      );
      expect(
        find.text('Clearance grant unreadable: Stations work it at desk'),
        findsOneWidget,
      );
      expect(find.text('1 quarantined in every Operation'), findsOneWidget);
      expect(
        find.textContaining('Model sonnet since 2028-08-'),
        findsOneWidget,
      );
      expect(
        find.text('Earlier: opus 2027-01-15 to 2028-08-16'),
        findsOneWidget,
      );
      expect(find.textContaining('default'), findsNothing);
      expect(
        find.textContaining('Model fable, handler claude'),
        findsOneWidget,
      );
      expect(find.text('Earlier: sonnet from 2028-08-16'), findsOneWidget);
      expect(
        find.textContaining('Model fable, handler claude'),
        findsOneWidget,
      );
      expect(find.text('Earlier: sonnet from 2028-08-16'), findsOneWidget);
      expect(find.text('field (configured desk)'), findsOneWidget);
      expect(find.text('Desk duty from 2030-01-31'), findsOneWidget);
      expect(find.text('Raised to field by alice'), findsOneWidget);
      expect(find.text('Promotion to 00 awaits green ink'), findsOneWidget);
      expect(find.text('0 done'), findsOneWidget);
      expect(
        find.text('Lower clearance'),
        findsNothing,
        reason: 'agent.configure is needed',
      );
    },
  );

  testWidgets(
    'the scorecard needs the roster and opens the Missions behind a number',
    (tester) async {
      final without = <http.Request>[];
      await openAgents(tester, signedIn(without, ['agent.read']));
      expect(
        find.text('Scorecard'),
        findsNothing,
        reason: 'roster.read is needed',
      );
      expect(
        without.map((request) => request.url.path),
        isNot(contains('/v1/user/operations/G/agents/scorecard')),
      );

      final requests = <http.Request>[];
      await openAgents(
        tester,
        signedIn(requests, ['agent.read'], roster: ['roster.read']),
      );
      await tester.tap(find.text('Scorecard'));
      await tester.pumpAndSettle();
      expect(
        find.text('4 judged · 25 % failed · small sample: not ranked'),
        findsOneWidget,
      );
      expect(find.widgetWithText(ActionChip, '3 done'), findsOneWidget);
      expect(find.text('1 lost with a Station'), findsOneWidget);
      expect(
        find.text('\$1.23 · 1500 tokens · 2 tools refused · 12 min'),
        findsOneWidget,
      );
      expect(
        find.textContaining('claude · sonnet · pool team'),
        findsOneWidget,
      );
      expect(find.textContaining('1 attempts name no Agent'), findsOneWidget);

      await tester.tap(find.widgetWithText(ActionChip, '3 done'));
      await tester.pumpAndSettle();
      expect(find.text('m-1'), findsOneWidget);
      expect(find.text('m-3'), findsOneWidget);
    },
  );

  testWidgets('a hold is shown to a reader and lifted by a configurer', (
    tester,
  ) async {
    final read = <http.Request>[];
    await openAgents(tester, signedIn(read, ['agent.read']));
    expect(find.text('Held by m: failing'), findsOneWidget);
    expect(
      find.text('Route claude · opus · pool team held by alice: slow'),
      findsOneWidget,
    );
    expect(
      find.text('Lift hold'),
      findsNothing,
      reason: 'agent.configure is needed',
    );

    final requests = <http.Request>[];
    await openAgents(
      tester,
      signedIn(requests, ['agent.read', 'agent.configure']),
    );
    await tester.tap(find.text('Lift hold').last);
    await tester.pumpAndSettle();
    final lifted = requests.lastWhere((request) => request.method == 'DELETE');
    expect(lifted.url.path, '/v1/user/operations/G/agents/008/hold');
    expect(lifted.url.queryParameters, {
      'handler': 'claude',
      'model': 'opus',
      'pool': 'team',
    });
  });

  testWidgets(
    'a configurer lowers a clearance and restores the configured one',
    (tester) async {
      final requests = <http.Request>[];
      final board = signedIn(requests, ['agent.read', 'agent.configure']);
      await openAgents(tester, board);

      expect(
        find.text('Lower clearance'),
        findsOneWidget,
        reason: '008 is at desk already',
      );
      await tester.tap(find.text('Lower clearance'));
      await tester.pumpAndSettle();
      expect(find.byType(DropdownButton<String>), findsWidgets);
      await tester.enterText(find.byType(TextField), 'under review');
      await tester.tap(find.text('Lower'));
      await tester.pumpAndSettle();
      final lowered = requests.lastWhere((request) => request.method == 'PUT');
      expect(lowered.url.path, '/v1/user/operations/G/agents/007/clearance');
      expect(jsonDecode(lowered.body), {
        'clearance': 'desk',
        'reason': 'under review',
      });

      await tester.tap(find.text('Restore configured clearance'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Restore'));
      await tester.pumpAndSettle();
      final restored = requests.lastWhere(
        (request) => request.method == 'DELETE',
      );
      expect(restored.url.path, '/v1/user/operations/G/agents/007/clearance');
    },
  );

  testWidgets(
    'All Operations is the default, says which failed, and narrows to one',
    (tester) async {
      final requests = <http.Request>[];
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: MockClient((request) async {
          requests.add(request);
          final path = request.url.path;
          if (path == '/v1/user/operations/G/agents') {
            return http.Response(
              jsonEncode(_agents),
              200,
              headers: {'content-type': 'application/json'},
            );
          }
          if (path == '/v1/user/operations/L/agents') {
            return http.Response('{"error":"down"}', 503);
          }
          if (path.endsWith('/clearance')) {
            return http.Response(
              jsonEncode({
                'agent': '007',
                'operation': 'G',
                'clearance': 'desk',
                'signer': 'operator-007',
                'at': 1.0,
              }),
              200,
              headers: {'content-type': 'application/json'},
            );
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
        operations: [
          UserOperationAccess(
            operationId: 'G',
            actions: const ['agent.read', 'agent.configure'],
          ),
          UserOperationAccess(operationId: 'L', actions: const ['agent.read']),
        ],
        roster: const [],
      );
      await openAgents(tester, board);

      final paths = requests.map((request) => request.url.path);
      expect(paths, contains('/v1/user/operations/G/agents'));
      expect(paths, contains('/v1/user/operations/L/agents'));
      expect(find.text('All Operations'), findsOneWidget);
      expect(find.text('Operation G'), findsOneWidget);
      expect(find.textContaining('Operation L: '), findsOneWidget);
      expect(find.text('field (configured desk)'), findsOneWidget);

      // An action from All goes to the row's Operation.
      await tester.tap(find.text('Lower clearance'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), 'under review');
      await tester.tap(find.text('Lower'));
      await tester.pumpAndSettle();
      expect(
        requests.lastWhere((request) => request.method == 'PUT').url.path,
        '/v1/user/operations/G/agents/007/clearance',
      );

      await tester.tap(find.text('All Operations'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('G').last);
      await tester.pumpAndSettle();
      expect(find.text('All Operations'), findsNothing);
      expect(find.textContaining('Operation L: '), findsNothing);
      expect(find.text('field (configured desk)'), findsOneWidget);
    },
  );

  group('the model an Agent card shows', () {
    AgentSummary summary(Map<String, dynamic> fields) => AgentSummary.fromJson({
      'agent': '007',
      'posted': true,
      'operation': const {'done': 0},
      ...fields,
    });
    const opus = {
      'model': 'opus',
      'from': 1_800_000_000.0,
      'to': 1_850_000_000.0,
    };
    const sonnet = {'model': 'sonnet', 'from': 1_850_000_000.0, 'to': null};

    test('with a model and no Missions, the record says', () {
      final agent = summary({
        'routes': [
          {'handler': 'claude', 'model': 'fable'},
        ],
        'model_history': const [],
      });
      expect(agent.modelLabels, ['Model fable, handler claude']);
      expect(agent.earlierModels, isEmpty);
    });

    test('an Agent with routes shows each, with its pool', () {
      final agent = summary({
        'routes': [
          {'handler': 'acp', 'model': 'grok-4.7', 'pool': 'grok-free'},
          {'handler': 'claude', 'model': 'gemini-free'},
        ],
        'model_history': const [],
      });
      expect(agent.modelLabels, [
        'Model grok-4.7, handler acp, pool grok-free',
        'Model gemini-free, handler claude',
      ]);
    });

    test('a changed model: the record wins and what ran stays as history', () {
      final agent = summary({
        'routes': [
          {'handler': 'claude', 'model': 'fable'},
        ],
        'model_history': const [opus, sonnet],
      });
      expect(agent.modelLabels, ['Model fable, handler claude']);
      expect(agent.earlierModels, [
        'opus 2027-01-15 to 2028-08-16',
        'sonnet from 2028-08-16',
      ]);
    });

    test('the date is of a tenure of that same model', () {
      final agent = summary({
        'routes': [
          {'handler': 'claude', 'model': 'sonnet'},
        ],
        'model_history': const [opus, sonnet],
      });
      expect(agent.modelLabels, [
        'Model sonnet since 2028-08-16, handler claude',
      ]);
      expect(agent.earlierModels, ['opus 2027-01-15 to 2028-08-16']);
    });

    test('no model set: the handler\'s default, and only then', () {
      final agent = summary({
        'routes': [
          {'handler': 'claude', 'model': null},
        ],
        'model_history': const [],
      });
      expect(agent.modelLabels, [
        "The handler's default model, handler claude",
      ]);
    });

    test(
      'a person who may not read the record sees what ran, and no guess',
      () {
        expect(
          summary({
            'routes': const [],
            'model_history': const [sonnet],
          }).modelLabels,
          ['Model sonnet since 2028-08-16'],
        );
        expect(
          summary({'routes': const [], 'model_history': const []}).modelLabels,
          isEmpty,
          reason: 'not "default": nothing here says so',
        );
      },
    );
  });
}
