import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/mission_detail.dart';
import 'package:outer_office/shell/missions_board.dart';
import 'package:outer_office/shell/missions_table.dart';
import 'package:outer_office/shell/oo_freshness.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/theme/oo_theme.dart';

final _now = DateTime.fromMillisecondsSinceEpoch(1_800_000_000 * 1000);

Map<String, Object?> _pending(
  String id,
  Map<String, Object?> pending, {
  String operation = 'G',
}) => {
  'operation_id': operation,
  'mission_id': id,
  'handle': 'ai6-$id',
  'runs': const <Object>[],
  'attempts': const <Object>[],
  'pending': pending,
};

final _missions = [
  _pending('8', {
    'state': 'queued',
    'kind': 'retrying',
    'reason': "Agent 7777's credentials could not be leased",
    'failures': 2,
    'retry_at': 1_800_000_160.0,
    'agent': '7777',
    'station': 'station-a',
    'at': 1_799_999_990.0,
  }),
  _pending('5', {
    'state': 'queued',
    'kind': 'waiting',
    'reason':
        'every place that fits is taken: the Operation or Agent (009) is at '
        'its max_concurrent',
    'failures': 0,
    'at': 1_799_999_900.0,
  }),
  _pending('1', {
    'state': 'failed',
    'kind': 'failed',
    'reason': 'no PATH',
    'failures': 0,
    'station': 'station-b',
    'at': 1_799_999_000.0,
  }),
  {
    'operation_id': 'G',
    'mission_id': '4',
    'handle': 'ai6-4',
    'runs': const <Object>[],
    'attempts': const <Object>[],
  },
];

Future<MissionBoard> _board() async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      http.Response json(Object body, [int status = 200]) => http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json'},
      );
      return switch (request.url.path) {
        '/v1/auth/login' => json({'token': 'user-token'}, 201),
        '/v1/board' => json([
          {
            'operation_id': 'G',
            'mission_id': '4',
            'run_id': 's4',
            'state': 'working',
            'sequence': 1,
            'updated_at': 1_799_999_999.0,
          },
        ]),
        '/v1/missions' => json(_missions),
        '/v1/stations' => json({'headquarters_station': null, 'stations': []}),
        '/v1/user/profile' => json({
          'person_id': 'operator-007',
          'name': 'operator',
          'display_name': null,
          'groups': [],
          'expires_at': 1_900_000_000,
          'operations': [
            {
              'operation_id': 'G',
              'actions': ['mission.read'],
            },
          ],
          'stations': [],
        }),
        _ => json(const <Object>[]),
      };
    }),
  );
  await board.login('operator', 'secret');
  addTearDown(board.dispose);
  return board;
}

Future<void> _show(WidgetTester tester, Widget child) async {
  tester.view.physicalSize = const Size(1400, 1000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    MaterialApp(
      theme: ooTheme(OoThemeId.values.first),
      home: Scaffold(body: child),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  final clock = ooClock;
  setUp(() => ooClock = () => _now);
  tearDown(() => ooClock = clock);

  test(
    'Missions with no Run are listed beside those that have one',
    () async {
      final board = await _board();
      final groups = board.missionGroups();
      expect(groups.map((group) => group.missionId), ['1', '4', '5', '8']);
      final byId = {for (final group in groups) group.missionId: group};
      expect(byId['4']!.pending, isNull);
      expect(byId['4']!.state, 'working');
      expect(byId['8']!.state, 'queued');
      expect(byId['8']!.runs, isEmpty);
      expect(byId['8']!.updatedAt, 1_799_999_990.0);
      expect(byId['1']!.state, 'failed');
      expect(byId['1']!.finished, isTrue);
      // A filter on a state sees them as it sees the rest.
      expect(
        groups
            .where((group) => group.state == 'queued')
            .map((g) => g.missionId),
        ['5', '8'],
      );
    },
  );

  test('a pending Mission says what it is doing and when it tries again', () {
    final now = _now.millisecondsSinceEpoch / 1000;
    PendingMission of(String kind, {int failures = 0, double? at}) =>
        PendingMission(
          state: 'queued',
          kind: kind,
          failures: failures,
          retryAt: at,
        );
    expect(
      of('retrying', failures: 3, at: now + 160).describe(now),
      'Retrying: 3 starts failed, next try in 160 s',
    );
    expect(
      of('retrying', failures: 1, at: now - 5).describe(now),
      'Retrying: 1 start failed, next try due',
    );
    expect(of('unplaced').describe(now), 'Waiting for a Station to take it');
    expect(of('queued').describe(now), 'Queued');
    expect(of('failed').describe(now), 'Failed before it started');
  });

  testWidgets('the table gives each its state, the retry and the reason', (
    tester,
  ) async {
    final board = await _board();
    await _show(
      tester,
      MissionsTable(
        board: board,
        missions: board.missionGroups(),
        route: OoRoute(OoPage.missions),
        onRoute: (_) {},
        onDetails: (_) {},
      ),
    );
    expect(
      find.text('Retrying: 2 starts failed, next try in 160 s'),
      findsOneWidget,
    );
    expect(
      find.text("Agent 7777's credentials could not be leased"),
      findsOneWidget,
    );
    expect(
      find.textContaining('every place that fits is taken'),
      findsOneWidget,
    );
    expect(find.text('Failed before it started'), findsOneWidget);
    expect(find.text('no PATH'), findsOneWidget);
  });

  test(
    'the board sorts Missions with and without Runs by either time',
    () async {
      final board = await _board();
      for (final sort in ['updated', 'created']) {
        for (final order in ['asc', 'desc']) {
          final route = OoRoute(OoPage.missions)
              .withField('sort', sort)
              .withField('order', order);
          final sorted = routeMissions(board, route);
          expect(
            sorted.length,
            board.missionGroups().length,
            reason: '$sort $order',
          );
          expect(sorted.any((group) => group.runs.isEmpty), isTrue);
          expect(sorted.any((group) => group.runs.isNotEmpty), isTrue);
        }
      }
    },
  );

  testWidgets('a Mission that failed before it started opens its detail', (
    tester,
  ) async {
    final board = await _board();
    final failed = board.missionGroups().firstWhere(
      (group) => group.missionId == '1',
    );
    await _show(
      tester,
      MissionDetail(
        board: board,
        mission: failed,
        pages: const [OoPage.missions],
        onRoute: (_) {},
        onOpenRun: (_) {},
        onClose: () {},
      ),
    );
    expect(find.text('Failed before it started'), findsOneWidget);
    expect(find.text('no PATH'), findsOneWidget);
    expect(find.textContaining('Station station-b'), findsWidgets);
  });
}
