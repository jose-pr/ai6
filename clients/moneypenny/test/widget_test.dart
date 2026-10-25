import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart'
    show FilledButton, MaterialApp, NavigationBar, SwitchListTile, TextField;
import 'package:moneypenny/main.dart';

String stationBoardResponse() => jsonEncode({
  'headquarters_station': 'station-alpha',
  'stations': [
    {
      'station_id': 'station-alpha',
      'healthy': true,
      'headquarters': true,
      'seen': 1000,
      'roles': ['control', 'execution'],
      'running': 2,
      'capacity': 4,
      'operations': ['operation-alpha'],
      'theaters': [
        {'id': 'local', 'access': 'local', 'labels': {}},
        {
          'id': 'build-box',
          'access': 'ssh',
          'host': 'build.example.net',
          'labels': {'os': 'linux'},
          'capabilities': {
            'terminal': true,
            'run': true,
            'files': true,
            'forward': false,
            'os': 'linux',
            'arch': 'x86_64',
          },
          'agents': 'remote-hands',
          'egress': 'none',
        },
      ],
    },
  ],
});

String userProfileResponse({
  String operationId = 'operation-alpha',
  List<String> actions = const [
    'mission.read',
    'mission.create',
    'session.read',
  ],
  List<Map<String, Object>> stations = const [],
}) => jsonEncode({
  'person_id': 'operator-007',
  'name': 'operator',
  'display_name': 'Operator 007',
  'groups': ['field'],
  'expires_at': 1_900_000_000,
  'operations': [
    {'operation_id': operationId, 'actions': actions},
  ],
  'stations': stations,
});

Finder navLabel(String label) =>
    find.descendant(of: find.byType(NavigationBar), matching: find.text(label));

void main() {
  testWidgets('demo board explains that sign-in is unavailable', (
    tester,
  ) async {
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      demo: true,
      pollInterval: null,
    );
    addTearDown(board.dispose);
    await tester.pumpWidget(
      MaterialApp(home: MissionBoardPage(board: board, desktopWindows: false)),
    );

    expect(
      find.text(
        'Demo preview · sign-in is unavailable. Open the authenticated Moneypenny site.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('demo Administration opens with sample Agent roles', (
    tester,
  ) async {
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      demo: true,
      pollInterval: null,
    );
    addTearDown(board.dispose);
    await tester.pumpWidget(
      MaterialApp(home: MissionBoardPage(board: board, desktopWindows: false)),
    );
    await tester.tap(navLabel('Administration'));
    await tester.pumpAndSettle();

    expect(find.text('Administration'), findsNWidgets(2));
    expect(find.text('case-officer'), findsOneWidget);
    expect(find.textContaining('Follow the Operation policy.'), findsOneWidget);
    expect(find.text('Add role'), findsNothing);
  });

  testWidgets(
    'Mission proposal preview shows provenance and disables decisions',
    (tester) async {
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://moneypenny.example'),
        demo: true,
        pollInterval: null,
      );
      addTearDown(board.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: MissionBoardPage(board: board, desktopWindows: false),
        ),
      );
      await tester.tap(navLabel('Administration'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Mission proposals'));
      await tester.pumpAndSettle();

      expect(find.text('Review the role catalog'), findsOneWidget);
      expect(find.textContaining('coordinator-007'), findsOneWidget);
      expect(find.textContaining('mission-007'), findsOneWidget);
      expect(
        find.textContaining('Preview data — decisions disabled'),
        findsOneWidget,
      );
      final approve = tester.widget<FilledButton>(
        find.widgetWithText(FilledButton, 'Approve and queue'),
      );
      expect(approve.onPressed, isNull);
    },
  );

  testWidgets('navigation hides Administration and Audit without access', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      pollInterval: null,
    );
    board.accessToken = 'user-token';
    board.loading = false;
    board.userSession = UserSessionSummary.fromJson(
      jsonDecode(userProfileResponse(actions: ['mission.read', 'session.read']))
          as Map<String, dynamic>,
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));

    expect(navLabel('Stations'), findsOneWidget);
    expect(navLabel('Operations'), findsOneWidget);
    expect(navLabel('Administration'), findsNothing);
    expect(navLabel('Audit log'), findsNothing);
  });

  testWidgets('Station settings update through the authorized gateway API', (
    tester,
  ) async {
    final requests = <http.Request>[];
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      pollInterval: null,
      client: MockClient((request) async {
        requests.add(request);
        if (request.method == 'GET' && request.url.path.endsWith('/settings')) {
          return http.Response(
            jsonEncode({
              'station_id': 'station-alpha',
              'max_concurrent_missions': 4,
              'configured': false,
              'execution_enabled': true,
            }),
            200,
          );
        }
        if (request.method == 'PUT' && request.url.path.endsWith('/settings')) {
          return http.Response('', 204);
        }
        return http.Response('[]', 200);
      }),
    );
    board.accessToken = 'operator-token';
    board.loading = false;
    board.userSession = UserSessionSummary.fromJson(
      jsonDecode(
        userProfileResponse(
          stations: [
            {
              'station_id': 'station-alpha',
              'actions': ['station.read', 'station.configure'],
            },
          ],
        ),
      ) as Map<String, dynamic>,
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    await tester.tap(navLabel('Administration'));
    await tester.pumpAndSettle();
    await tester.pumpAndSettle();
    expect(find.text('Maximum concurrent Missions'), findsOneWidget);
    expect(find.textContaining('Current Station value.'), findsOneWidget);

    await tester.enterText(find.byType(TextField).last, '7');
    await tester.tap(find.text('Save settings'));
    await tester.pumpAndSettle();

    final update = requests.singleWhere((request) => request.method == 'PUT');
    expect(update.url.path, '/v1/user/stations/station-alpha/settings');
    expect(jsonDecode(update.body), {'max_concurrent_missions': 7});
  });

  test(
    'proxy sign-in exchanges the authenticated reverse-proxy identity',
    () async {
      final requests = <http.Request>[];
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://moneypenny.example'),
        pollInterval: null,
        client: MockClient((request) async {
          requests.add(request);
          if (request.url.path == '/v1/auth/login') {
            expect(jsonDecode(request.body), isEmpty);
            return http.Response(
              jsonEncode({'token': 'proxy-user-token'}),
              201,
            );
          }
          expect(request.headers['authorization'], 'Bearer proxy-user-token');
          if (request.url.path == '/v1/missions') {
            return http.Response('[]', 200);
          }
          if (request.url.path == '/v1/stations') {
            return http.Response(stationBoardResponse(), 200);
          }
          if (request.url.path == '/v1/user/profile') {
            return http.Response(userProfileResponse(), 200);
          }
          expect(request.url.path, '/v1/board');
          return http.Response('[]', 200);
        }),
      );
      addTearDown(board.dispose);

      await board.loginViaProxy();

      expect(board.accessToken, 'proxy-user-token');
      expect(board.canCreateMission('operation-alpha'), isTrue);
      expect(requests.map((request) => request.url.path), [
        '/v1/auth/login',
        '/v1/board',
        '/v1/missions',
        '/v1/stations',
        '/v1/user/profile',
      ]);
    },
  );

  test(
    'proxy sign-in rejection explains the required organization proxy',
    () async {
      final board = MissionBoard(
        apiOrigin: Uri.parse('http://127.0.0.1:7357'),
        pollInterval: null,
        client: MockClient((request) async {
          expect(request.url.path, '/v1/auth/login');
          expect(jsonDecode(request.body), isEmpty);
          return http.Response('Not found', 404);
        }),
      );
      addTearDown(board.dispose);

      await board.loginViaProxy();

      expect(board.accessToken, isNull);
      expect(
        board.loginError,
        contains(
          'Open Moneypenny through your authenticated organization proxy',
        ),
      );
      expect(board.loginError, isNot(contains('Check the account')));
    },
  );

  test(
    'Mission creation posts a bounded brief and refreshes the board',
    () async {
      final requests = <http.Request>[];
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://moneypenny.example'),
        pollInterval: null,
        client: MockClient((request) async {
          requests.add(request);
          if (request.url.path ==
              '/v1/user/operations/operation-alpha/missions') {
            expect(request.headers['authorization'], 'Bearer operator-token');
            expect(jsonDecode(request.body), {
              'title': 'Inspect the gateway',
              'prompt': 'Review the current Operation and report findings.',
            });
            return http.Response(
              jsonEncode({
                'operation_id': 'operation-alpha',
                'mission_id': 'moneypenny--mission-123',
                'state': 'queued',
              }),
              202,
            );
          }
          if (request.url.path == '/v1/board') {
            return http.Response('[]', 200);
          }
          if (request.url.path == '/v1/missions') {
            return http.Response('[]', 200);
          }
          if (request.url.path == '/v1/stations') {
            return http.Response(stationBoardResponse(), 200);
          }
          if (request.url.path == '/v1/user/profile') {
            return http.Response(userProfileResponse(), 200);
          }
          fail('Unexpected request ${request.url}');
        }),
      );
      board.accessToken = 'operator-token';
      addTearDown(board.dispose);

      final missionId = await board.createMission(
        operationId: 'operation-alpha',
        title: 'Inspect the gateway',
        prompt: 'Review the current Operation and report findings.',
      );

      expect(missionId, 'moneypenny--mission-123');
      expect(requests.map((request) => request.url.path), [
        '/v1/user/operations/operation-alpha/missions',
        '/v1/board',
        '/v1/missions',
        '/v1/stations',
        '/v1/user/profile',
      ]);
    },
  );

  test(
    'proposal review calls the authorized list and approval endpoints',
    () async {
      final requests = <http.Request>[];
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://moneypenny.example'),
        pollInterval: null,
        client: MockClient((request) async {
          requests.add(request);
          if (request.url.path.endsWith('/mission-proposals')) {
            return http.Response(
              jsonEncode([
                {
                  'id': 'proposal-123',
                  'operation_id': 'operation-alpha',
                  'parent_mission_id': 'parent-1',
                  'proposer_agent_id': 'coordinator-007',
                  'proposer_session_id': 'provider-session-9',
                  'created_at': 1_798_000_000,
                  'brief': {
                    'title': 'Review the gateway',
                    'body': 'Check its permission boundaries.',
                    'role': 'analyst',
                    'tags': ['security'],
                    'priority': 1,
                  },
                },
              ]),
              200,
            );
          }
          if (request.method == 'POST' &&
              request.url.path.endsWith('/approve')) {
            expect(request.headers['authorization'], 'Bearer operator-token');
            return http.Response(
              jsonEncode({
                'operation_id': 'operation-alpha',
                'proposal_id': 'proposal-123',
                'mission_id': 'ai6-agent-proposal--proposal-123',
                'state': 'queued',
              }),
              202,
            );
          }
          if (request.url.path == '/v1/board' ||
              request.url.path == '/v1/missions') {
            return http.Response('[]', 200);
          }
          if (request.url.path == '/v1/stations') {
            return http.Response(stationBoardResponse(), 200);
          }
          if (request.url.path == '/v1/user/profile') {
            return http.Response(userProfileResponse(), 200);
          }
          fail('Unexpected request ${request.url}');
        }),
      );
      board.accessToken = 'operator-token';
      board.userSession = UserSessionSummary.fromJson(
        jsonDecode(userProfileResponse()) as Map<String, dynamic>,
      );
      addTearDown(board.dispose);

      final proposals = await board.loadMissionProposals('operation-alpha');
      expect(proposals.single.proposerSessionId, 'provider-session-9');
      expect(proposals.single.body, 'Check its permission boundaries.');
      await board.decideMissionProposal(
        operationId: 'operation-alpha',
        proposalId: proposals.single.id,
        approve: true,
      );

      expect(
        requests.first.url.path,
        '/v1/user/operations/operation-alpha/mission-proposals',
      );
      expect(requests.first.headers['authorization'], 'Bearer operator-token');
      expect(
        requests[1].url.path,
        '/v1/user/operations/operation-alpha/mission-proposals/proposal-123/approve',
      );
    },
  );

  test(
    'operator menu identity uses the authenticated gateway session',
    () async {
      final requests = <http.Request>[];
      final themeWrites = <bool>[];
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://moneypenny.example'),
        pollInterval: null,
        onThemeChanged: (lightTheme) async => themeWrites.add(lightTheme),
        client: MockClient((request) async {
          requests.add(request);
          return http.Response(
            jsonEncode({
              'person_id': 'operator-007',
              'name': 'operator',
              'display_name': 'Operator 007',
              'groups': ['field'],
              'expires_at': 1_900_000_000,
              'operations': [
                {
                  'operation_id': 'operation-alpha',
                  'actions': ['mission.read', 'session.read'],
                },
              ],
            }),
            200,
          );
        }),
      );
      board.accessToken = 'operator-token';
      addTearDown(board.dispose);

      final identity = await board.loadUserSession();
      expect(identity.personId, 'operator-007');
      expect(identity.expiresAt, 1_900_000_000);
      expect(requests.single.url.path, '/v1/user/profile');
      expect(requests.single.headers['authorization'], 'Bearer operator-token');
      expect(board.canCreateMission('operation-alpha'), isFalse);

      board.toggleTheme();
      expect(board.lightTheme, isTrue);
      board.toggleTheme();
      expect(board.lightTheme, isFalse);
      await Future<void>.delayed(Duration.zero);
      expect(themeWrites, [true, false]);
    },
  );

  test(
    'audit request is bearer-authenticated and keeps metadata fields',
    () async {
      final requests = <http.Request>[];
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://moneypenny.example'),
        pollInterval: null,
        client: MockClient((request) async {
          requests.add(request);
          return http.Response(
            jsonEncode([
              {
                'id': 'audit-001',
                'actor': 'operator-007',
                'action': 'mission.create.requested',
                'operation': 'operation-alpha',
                'target': 'mission-007',
                'at': 1_900_000_000,
              },
            ]),
            200,
          );
        }),
      );
      board.accessToken = 'operator-token';
      addTearDown(board.dispose);

      final records = await board.loadAudit();

      expect(requests.single.url.path, '/v1/user/audit');
      expect(requests.single.url.queryParameters, {'limit': '100'});
      expect(requests.single.headers['authorization'], 'Bearer operator-token');
      expect(records.single.actor, 'operator-007');
      expect(records.single.action, 'mission.create.requested');
      expect(records.single.target, 'mission-007');
    },
  );

  test(
    'Agent role administration uses the authenticated Operation routes',
    () async {
      final requests = <http.Request>[];
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://moneypenny.example'),
        pollInterval: null,
        client: MockClient((request) async {
          requests.add(request);
          if (request.method == 'GET') {
            return http.Response(
              jsonEncode([
                {
                  'id': 'case-officer',
                  'purpose': 'Coordinate Mission work.',
                  'policy': 'Follow the Operation policy.',
                },
              ]),
              200,
            );
          }
          return http.Response('', 204);
        }),
      );
      board.accessToken = 'operator-token';
      addTearDown(board.dispose);

      final roles = await board.loadAgentRoles('operation-alpha');
      expect(roles.single.id, 'case-officer');
      expect(roles.single.policy, 'Follow the Operation policy.');
      await board.saveAgentRole('operation-alpha', roles.single);
      await board.deleteAgentRole('operation-alpha', 'case-officer');

      expect(requests.map((request) => request.method), [
        'GET',
        'PUT',
        'DELETE',
      ]);
      expect(requests.map((request) => request.url.path), [
        '/v1/user/operations/operation-alpha/agent-roles',
        '/v1/user/operations/operation-alpha/agent-roles/case-officer',
        '/v1/user/operations/operation-alpha/agent-roles/case-officer',
      ]);
      expect(
        requests.every(
          (request) =>
              request.headers['authorization'] == 'Bearer operator-token',
        ),
        isTrue,
      );
      expect(
        jsonDecode(requests[1].body)['purpose'],
        'Coordinate Mission work.',
      );
    },
  );

  test('session attachment exchanges a scoped grant for a cookie', () async {
    final requests = <http.Request>[];
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      pollInterval: null,
      client: MockClient((request) async {
        requests.add(request);
        if (request.url.path == '/v1/user/attach-grants') {
          return http.Response(jsonEncode({'token': 'scoped-grant'}), 201);
        }
        expect(request.url.path, '/v1/attach');
        return http.Response(
          '',
          204,
          headers: {'set-cookie': 'ai6_attach=one-use-ticket; Path=/v1/socket'},
        );
      }),
    );
    board.accessToken = 'user-token';
    addTearDown(board.dispose);

    final cookie = await board.prepareSessionSocket(
      const SessionSummary(
        operationId: 'station-a',
        missionId: 'mission-1',
        sessionId: 'agent-session-1',
        state: 'working',
        sequence: 4,
      ),
      write: false,
    );

    expect(cookie, 'ai6_attach=one-use-ticket');
    expect(requests.map((request) => request.url.path), [
      '/v1/user/attach-grants',
      '/v1/attach',
    ]);
    expect(requests.first.headers['authorization'], 'Bearer user-token');
    expect(jsonDecode(requests.first.body), {
      'operation_id': 'station-a',
      'mission_id': 'mission-1',
      'session_id': 'agent-session-1',
      'mode': 'read',
    });
    expect(requests.last.headers['origin'], 'https://moneypenny.example');
    expect(requests.last.headers['x-forwarded-proto'], 'https');
    expect(jsonDecode(requests.last.body), {'token': 'scoped-grant'});
    expect(
      requests.every((request) => request.url.queryParameters.isEmpty),
      isTrue,
    );
  });

  testWidgets('board groups and renders gateway Session snapshots', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      pollInterval: null,
      client: MockClient((request) async {
        if (request.method == 'POST') {
          expect(request.url.path, '/v1/auth/login');
          expect(jsonDecode(request.body), {
            'name': 'operator',
            'secret': 'secret',
          });
          return http.Response(jsonEncode({'token': 'user-token'}), 201);
        }
        expect(request.headers['authorization'], 'Bearer user-token');
        if (request.url.path == '/v1/missions') {
          return http.Response(
            jsonEncode([
              {
                'operation_id': 'operation-alpha',
                'mission_id': 'mission-007',
                'sessions': [],
                'brief': {
                  'prompt': 'Review the requested change.',
                  'agent_id': 'technical-officer',
                  'role_id': 'case-officer',
                  'handler_provider': 'claude',
                  'terminal_provider': 'pty',
                  'working_directory': 'C:/stations/alpha',
                  'model': 'claude-sonnet',
                  'effort': 'high',
                  'environment_keys': ['AI6_MISSION_ID'],
                  'station_id': 'station-alpha',
                  'theater_id': 'local',
                },
              },
            ]),
            200,
          );
        }
        if (request.url.path == '/v1/stations') {
          return http.Response(stationBoardResponse(), 200);
        }
        if (request.url.path == '/v1/user/profile') {
          return http.Response(userProfileResponse(), 200);
        }
        if (request.url.path ==
            '/v1/user/operations/operation-alpha/placement-preview') {
          return http.Response(
            jsonEncode({
              'operation_id': 'operation-alpha',
              'policy_station': 'station-alpha',
              'choices': [
                {
                  'station': 'station-alpha',
                  'theater': 'local',
                  'agent': '007',
                  'handler': 'claude',
                  'mode': 'field',
                },
                {
                  'station': 'station-alpha',
                  'theater': 'build-box',
                  'agent': '007',
                  'handler': 'claude',
                  'mode': 'remote-hands',
                },
              ],
              'rejections': [
                {
                  'station': 'station-beta',
                  'theater': null,
                  'agent': null,
                  'reason': 'it is at capacity (2 of 2 Missions)',
                },
                {
                  'station': null,
                  'theater': null,
                  'agent': '008',
                  'reason': 'it has no handler configured',
                },
              ],
            }),
            200,
          );
        }
        expect(request.url.path, '/v1/board');
        return http.Response(
          jsonEncode([
            {
              'operation_id': 'operation-alpha',
              'mission_id': 'mission-007',
              'session_id': 'claude-session-123',
              'state': 'working',
              'sequence': 4,
            },
            {
              'operation_id': 'operation-alpha',
              'mission_id': 'mission-007',
              'session_id': 'codex-session-456',
              'state': 'blocked',
              'sequence': 9,
            },
            {
              'operation_id': 'operation-alpha',
              'mission_id': 'mission-007',
              'session_id': 'claude-session-789',
              'state': 'parked',
              'sequence': 3,
              'exposure': ['brief:source', 'tool:WebFetch'],
            },
            {
              'operation_id': 'operation-alpha',
              'mission_id': 'mission-007',
              'session_id': 'claude-session-901',
              'state': 'quarantined',
              'sequence': 2,
            },
          ]),
          200,
        );
      }),
    );
    addTearDown(board.dispose);
    await board.login('operator', 'secret');

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));

    expect(find.text('Operations'), findsNWidgets(2));
    expect(find.text('operation-alpha'), findsOneWidget);
    expect(find.text('Mission mission-007'), findsOneWidget);
    expect(find.text('claude-session-123'), findsOneWidget);
    expect(find.text('codex-session-456'), findsOneWidget);
    expect(find.text('WORKING'), findsOneWidget);
    expect(find.text('BLOCKED'), findsOneWidget);
    expect(find.text('PARKED'), findsOneWidget);
    expect(find.text('QUARANTINED'), findsOneWidget);
    expect(find.text('EXPOSED'), findsOneWidget);
    expect(
      find.byTooltip('Read untrusted input: brief:source, tool:WebFetch'),
      findsOneWidget,
    );

    await tester.tap(navLabel('Stations'));
    await tester.pumpAndSettle();
    expect(find.text('HQS'), findsOneWidget);
    expect(find.text('station-alpha'), findsOneWidget);
    expect(find.textContaining('2/4 sessions'), findsOneWidget);
    expect(find.text('Theater local'), findsOneWidget);
    expect(find.text('Theater build-box'), findsOneWidget);
    expect(
      find.byTooltip(
        'ssh build.example.net \u00b7 os=linux\n'
        'terminal, run, files \u00b7 linux/x86_64\n'
        'agents remote-hands \u00b7 egress none',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Preview placement'));
    await tester.pumpAndSettle();
    expect(find.text('station-alpha \u00b7 local \u00b7 007'), findsOneWidget);
    expect(
      find.text('station-alpha \u00b7 build-box \u00b7 007 \u00b7 remote-hands'),
      findsOneWidget,
    );
    expect(
      find.text('station-beta: it is at capacity (2 of 2 Missions)'),
      findsOneWidget,
    );
    expect(
      find.text('Agent 008: it has no handler configured'),
      findsOneWidget,
    );
    expect(find.text('operation-alpha'), findsNWidgets(2));
    expect(find.text('claude-session-123'), findsNothing);
    await tester.tap(navLabel('Operations'));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('View Mission brief and launch context'));
    await tester.pumpAndSettle();
    expect(find.text('Review the requested change.'), findsOneWidget);
    expect(find.text('case-officer'), findsOneWidget);
    expect(find.text('station-alpha'), findsOneWidget);
    expect(find.text('local'), findsOneWidget);
    expect(find.text('AI6_MISSION_ID'), findsOneWidget);
    expect(find.text('Environment values are withheld.'), findsOneWidget);
    await tester.pageBack();
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('Filter by blocked'));
    await tester.pumpAndSettle();
    expect(find.text('claude-session-123'), findsNothing);
    expect(find.text('codex-session-456'), findsOneWidget);

    await tester.tap(find.byTooltip('Filter by parked'));
    await tester.pumpAndSettle();
    expect(find.text('claude-session-789'), findsOneWidget);
    expect(find.text('codex-session-456'), findsNothing);

    await tester.tap(find.byTooltip('Filter by quarantined'));
    await tester.pumpAndSettle();
    expect(find.text('claude-session-901'), findsOneWidget);
    expect(find.text('claude-session-789'), findsNothing);

    await tester.tap(find.byTooltip('Show all Sessions'));
    await tester.pumpAndSettle();
    expect(find.text('claude-session-123'), findsOneWidget);
    expect(find.text('codex-session-456'), findsOneWidget);
    expect(find.text('claude-session-789'), findsOneWidget);

    await tester.tap(find.text('claude-session-123'));
    await tester.pumpAndSettle();
    expect(find.text('Session claude-session-123'), findsOneWidget);
  });

  testWidgets('demo Session details explain that terminal data is offline', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final board = MissionBoard(
      apiOrigin: Uri.parse('http://127.0.0.1:7082'),
      demo: true,
      pollInterval: null,
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    await tester.tap(find.text('preview-session-007'));
    await tester.pumpAndSettle();

    expect(find.text('Session preview-session-007'), findsOneWidget);
    expect(
      find.text(
        'Demo Sessions are sample data. No terminal connection is available.',
      ),
      findsOneWidget,
    );
    expect(
      find.text('Sign in to Universal Exports to attach to this Session.'),
      findsNothing,
    );

    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('preview-session-007'), findsOneWidget);
    expect(
      board.sessions
          .singleWhere((session) => session.sessionId == 'preview-session-007')
          .state,
      'working',
    );
  });

  testWidgets('wide web view keeps the Mission board beside Session detail', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final board = MissionBoard(
      apiOrigin: Uri.parse('http://127.0.0.1:7082'),
      demo: true,
      pollInterval: null,
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    await tester.tap(find.text('preview-session-007'));
    await tester.pumpAndSettle();

    expect(find.text('Mission board'), findsOneWidget);
    expect(find.text('preview-session-007'), findsOneWidget);
    expect(find.text('Session preview'), findsOneWidget);
    expect(find.byTooltip('Close Session details'), findsOneWidget);

    await tester.tap(find.byTooltip('Close Session details'));
    await tester.pumpAndSettle();
    expect(find.text('Mission board'), findsOneWidget);
    expect(find.text('Session preview'), findsNothing);
    expect(find.text('preview-session-007'), findsOneWidget);
  });

  testWidgets('demo audit view renders only sample event metadata', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final board = MissionBoard(
      apiOrigin: Uri.parse('http://127.0.0.1:7082'),
      demo: true,
      pollInterval: null,
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    await tester.tap(navLabel('Audit log'));
    await tester.pumpAndSettle();

    expect(
      find.text('operator-007 · mission.create.requested'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Operation G · moneypenny--mission-007'),
      findsOneWidget,
    );
    expect(find.textContaining('prompt'), findsNothing);
  });

  testWidgets('operator menu shows account identity and theme preference', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      pollInterval: null,
      client: MockClient((request) async {
        if (request.url.path == '/v1/user/profile') {
          return http.Response(
            jsonEncode({
              'person_id': 'operator-007',
              'name': 'operator',
              'display_name': 'Operator 007',
              'groups': ['field'],
              'expires_at': 1_900_000_000,
              'operations': [
                {
                  'operation_id': 'operation-alpha',
                  'actions': ['mission.read', 'session.read'],
                },
              ],
            }),
            200,
          );
        }
        fail('Unexpected request ${request.url}');
      }),
    );
    board.accessToken = 'operator-token';
    board.loading = false;
    board.userSession = UserSessionSummary.fromJson({
      'person_id': 'operator-007',
      'name': 'operator',
      'display_name': 'Operator 007',
      'groups': ['field'],
      'expires_at': 1_900_000_000,
      'operations': [
        {
          'operation_id': 'operation-alpha',
          'actions': ['mission.read', 'session.read'],
        },
      ],
    });
    board.stationBoard = StationBoardSummary.fromJson(
      jsonDecode(stationBoardResponse()) as Map<String, dynamic>,
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    expect(find.text('O'), findsOneWidget);
    await tester.tap(find.byTooltip('Operator account and preferences'));
    await tester.pumpAndSettle();

    expect(find.text('Person ID: operator-007'), findsOneWidget);
    expect(find.text('Effective actions by Operation'), findsOneWidget);
    expect(
      find.text('operation-alpha: mission.read, session.read'),
      findsOneWidget,
    );
    await tester.tap(find.byType(SwitchListTile));
    await tester.pumpAndSettle();
    expect(board.lightTheme, isTrue);
    await tester.tap(find.text('Close'));
    await tester.pumpAndSettle();
  });

  testWidgets('audit log navigation is permission-gated and shows metadata', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      pollInterval: null,
      client: MockClient((request) async {
        expect(request.url.path, '/v1/user/audit');
        expect(request.headers['authorization'], 'Bearer operator-token');
        return http.Response(
          jsonEncode([
            {
              'id': 'audit-001',
              'actor': 'operator-007',
              'action': 'mission.create.requested',
              'operation': 'operation-alpha',
              'target': 'mission-007',
              'at': 1_900_000_000,
            },
          ]),
          200,
        );
      }),
    );
    board.accessToken = 'operator-token';
    board.loading = false;
    board.userSession = UserSessionSummary.fromJson(
      jsonDecode(userProfileResponse(actions: ['mission.read', 'audit.read']))
          as Map<String, dynamic>,
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    await tester.tap(navLabel('Audit log'));
    await tester.pumpAndSettle();

    expect(find.text('Audit log'), findsNWidgets(2));
    expect(
      find.text('operator-007 · mission.create.requested'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Operation operation-alpha · mission-007'),
      findsOneWidget,
    );
    expect(find.textContaining('prompt'), findsNothing);
  });

  testWidgets('sign in loads only the authenticated board', (tester) async {
    final requests = <http.Request>[];
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      pollInterval: null,
      client: MockClient((request) async {
        requests.add(request);
        if (request.url.path == '/v1/auth/login') {
          expect(request.method, 'POST');
          expect(jsonDecode(request.body), {
            'name': 'operator',
            'secret': 'secret',
          });
          return http.Response(jsonEncode({'token': 'user-token'}), 201);
        }
        if (request.url.path == '/v1/missions') {
          expect(request.headers['authorization'], 'Bearer user-token');
          return http.Response('[]', 200);
        }
        if (request.url.path == '/v1/stations') {
          return http.Response(stationBoardResponse(), 200);
        }
        if (request.url.path == '/v1/user/profile') {
          return http.Response(userProfileResponse(), 200);
        }
        expect(request.url.path, '/v1/board');
        expect(request.headers['authorization'], 'Bearer user-token');
        return http.Response(
          jsonEncode([
            {
              'operation_id': 'operation-alpha',
              'mission_id': 'mission-007',
              'session_id': 'claude-session-123',
              'state': 'working',
              'sequence': 4,
            },
          ]),
          200,
        );
      }),
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    expect(find.text('Sign in'), findsOneWidget);

    await tester.enterText(find.byType(TextField).at(0), 'operator');
    await tester.enterText(find.byType(TextField).at(1), 'secret');
    await tester.pump();
    await tester.tap(find.text('Continue'));
    await tester.pumpAndSettle();

    expect(find.text('Operations'), findsNWidgets(2));
    expect(find.text('claude-session-123'), findsOneWidget);
    await tester.tap(navLabel('Stations'));
    await tester.pumpAndSettle();
    expect(find.text('station-alpha'), findsOneWidget);
    expect(find.text('claude-session-123'), findsNothing);
    expect(requests.map((request) => request.url.path), [
      '/v1/auth/login',
      '/v1/board',
      '/v1/missions',
      '/v1/stations',
      '/v1/user/profile',
    ]);
  });

  testWidgets('Continue is enabled only when both credentials are present', (
    tester,
  ) async {
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://moneypenny.example'),
      pollInterval: null,
      client: MockClient((_) async => http.Response('', 500)),
    );
    addTearDown(board.dispose);

    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));

    FilledButton button() =>
        tester.widget<FilledButton>(find.byType(FilledButton));
    expect(button().onPressed, isNull);

    await tester.enterText(find.byType(TextField).at(0), 'operator');
    await tester.pump();
    expect(button().onPressed, isNull);

    await tester.enterText(find.byType(TextField).at(1), 'secret');
    await tester.pump();
    expect(button().onPressed, isNotNull);
  });
}
