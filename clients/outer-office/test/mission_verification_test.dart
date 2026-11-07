import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/mission_detail.dart';
import 'package:outer_office/shell/mission_outcome.dart';
import 'package:outer_office/shell/missions_board.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;
import 'package:outer_office/theme/oo_theme.dart';

const _session = SessionSummary(
  operationId: 'G',
  missionId: 'm1',
  sessionId: 's1',
  state: 'verifying',
  sequence: 5,
  stationId: 'hq',
);

/// What a probe of `branch` found, as the gateway's Mission detail spells it.
Map<String, Object?> _evidence(String result, String detail, double at) => {
  'deliverable': 'branch',
  'result': result,
  'detail': detail,
  'at': at,
};

/// A board whose Mission m1 of Operation G is verifying at attempt 2 with the deliverables
/// `branch` and `file:docs/report.md`; the requests the app makes are kept in [calls], and a
/// request to `probe` or `verify` is answered with [refuse] when it is given.
Future<MissionBoard> _board(
  List<String> actions, {
  List<(String, Map<String, dynamic>?)>? calls,
  (int, String)? refuse,
  List<Map<String, Object?>> evidence = const [],
  double? probeRequested,
}) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      final path = request.url.path;
      http.Response json(Object body, [int status = 200]) => http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json'},
      );
      if (path.endsWith('/probe') || path.endsWith('/verify')) {
        calls?.add((
          path,
          request.body.isEmpty
              ? null
              : jsonDecode(request.body) as Map<String, dynamic>,
        ));
        if (refuse != null) return json({'error': refuse.$2}, refuse.$1);
        return json({'operation_id': 'G', 'mission_id': 'm1'});
      }
      return switch (path) {
        '/v1/auth/login' => json({'token': 'user-token'}, 201),
        '/v1/board' => json([
          {
            'operation_id': 'G',
            'mission_id': 'm1',
            'session_id': 's1',
            'state': 'verifying',
            'sequence': 5,
          },
        ]),
        '/v1/stations' => json({'headquarters_station': null, 'stations': []}),
        '/v1/user/profile' => json({
          'person_id': 'operator-007',
          'name': 'operator',
          'display_name': null,
          'groups': [],
          'expires_at': 1_900_000_000,
          'operations': [
            {'operation_id': 'G', 'actions': actions},
          ],
          'stations': [],
        }),
        '/v1/operations/G/missions/m1' => json({
          'operation_id': 'G',
          'mission_id': 'm1',
          'handle': 'G-1',
          'sessions': const <Object>[],
          'attempts': [
            {
              'attempt': 2,
              'session_id': 's1',
              'station': 'hq',
              'status': 'ended',
              'final_report': 'Pushed the branch.',
              'deliverables': ['branch', 'file:docs/report.md'],
              'evidence': evidence,
              'probe_requested': ?probeRequested,
            },
          ],
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
  tester.view.physicalSize = const Size(1280, 1400);
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

Future<void> _detail(WidgetTester tester, MissionBoard board) => _show(
  tester,
  MissionDetail(
    board: board,
    mission: MissionGroup(
      operationId: 'G',
      missionId: 'm1',
      sessions: const [_session],
    ),
    pages: const [OoPage.missions],
    onRoute: (_) {},
    onOpenSession: (_) {},
    onClose: () {},
  ),
);

final _latest = [
  _evidence('missing', 'no branch m1 on origin', 1_800_000_000),
  _evidence('found', 'branch m1 at 1a2b3c', 1_800_000_100),
];

void main() {
  testWidgets('the board counts verifying Missions as waiting for a decision', (
    tester,
  ) async {
    const working = SessionSummary(
      operationId: 'G',
      missionId: 'm2',
      sessionId: 's2',
      state: 'working',
      sequence: 1,
    );
    await _show(
      tester,
      Material(
        child: BoardHeader(
          missions: groupMissions(const [_session, working]),
          selectedStates: const [],
          onSelectStates: (_) {},
          sort: const SizedBox.shrink(),
        ),
      ),
    );
    expect(boardStates, contains('verifying'));
    expect(find.text('verifying (waiting for a decision)  1'), findsOneWidget);
    expect(find.byTooltip('Missions waiting for a decision'), findsOneWidget);
  });

  testWidgets('a verifying Mission shows each deliverable with its latest '
      'probe, and the report', (tester) async {
    final board = await _board(['mission.read'], evidence: _latest);
    await _detail(tester, board);
    expect(find.textContaining('Waiting for a decision'), findsWidgets);
    expect(find.text('branch'), findsOneWidget);
    // The later finding of a deliverable replaces the earlier one.
    expect(find.text('found'), findsOneWidget);
    expect(find.text('missing'), findsNothing);
    expect(find.text('branch m1 at 1a2b3c'), findsOneWidget);
    expect(find.text('file:docs/report.md'), findsOneWidget);
    expect(find.text('not probed yet'), findsOneWidget);
    expect(
      find.textContaining('Pushed the branch.', findRichText: true),
      findsOneWidget,
    );
    // Without mission.verify a reader may look and not decide.
    expect(find.text('Probe now'), findsNothing);
    expect(find.text('Done'), findsNothing);
    expect(find.text('Failed'), findsNothing);
    expect(find.textContaining('Redispatch'), findsNothing);
  });

  testWidgets('a failed probe and an unprobed one are named as such', (
    tester,
  ) async {
    final board = await _board(
      ['mission.read'],
      evidence: [
        _evidence('failed', 'git ls-remote timed out', 1_800_000_000),
        {
          'deliverable': 'file:docs/report.md',
          'result': 'unprobed',
          'detail': 'work is on a Theater this Station does not reach',
          'at': 1_800_000_000.0,
        },
      ],
      probeRequested: 1_800_000_050,
    );
    await _detail(tester, board);
    expect(find.text('failed'), findsOneWidget);
    expect(find.text('unprobed'), findsOneWidget);
    expect(find.text('git ls-remote timed out'), findsOneWidget);
    expect(find.textContaining('has not run it yet'), findsOneWidget);
  });

  testWidgets('a Mission not verifying has no verification panel', (
    tester,
  ) async {
    final board = await _board(['mission.verify']);
    await _show(
      tester,
      SingleChildScrollView(
        child: MissionOutcomeSection(
          board: board,
          operationId: 'G',
          missionId: 'm1',
          state: 'verifying',
        ),
      ),
    );
    expect(find.text('OUTCOME'), findsNothing);
  });

  testWidgets('Probe now asks the Station, with mission.verify', (
    tester,
  ) async {
    final calls = <(String, Map<String, dynamic>?)>[];
    final board = await _board(['mission.verify'], calls: calls);
    await _detail(tester, board);
    await tester.tap(find.text('Probe now'));
    await tester.pumpAndSettle();
    expect(calls.map((call) => call.$1), [
      '/v1/user/operations/G/missions/m1/probe',
    ]);
  });

  testWidgets('Done needs a reason, then records it', (tester) async {
    final calls = <(String, Map<String, dynamic>?)>[];
    final board = await _board(['mission.verify'], calls: calls);
    await _detail(tester, board);
    await tester.tap(find.text('Done'));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<FilledButton>(find.widgetWithText(FilledButton, 'Record it'))
          .onPressed,
      isNull,
    );
    await tester.enterText(find.byType(TextField), 'branch and file are there');
    await tester.pump();
    await tester.tap(find.text('Record it'));
    await tester.pumpAndSettle();
    expect(calls.single.$1, '/v1/user/operations/G/missions/m1/verify');
    expect(calls.single.$2, {
      'decision': 'done',
      'reason': 'branch and file are there',
    });
  });

  testWidgets('Failed records its reason', (tester) async {
    final calls = <(String, Map<String, dynamic>?)>[];
    final board = await _board(['mission.verify'], calls: calls);
    await _detail(tester, board);
    await tester.tap(find.text('Failed'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'never pushed');
    await tester.pump();
    await tester.tap(find.text('Record it'));
    await tester.pumpAndSettle();
    expect(calls.single.$2, {'decision': 'failed', 'reason': 'never pushed'});
  });

  testWidgets('Redispatch says it becomes the next attempt and sends the '
      'amendment', (tester) async {
    final calls = <(String, Map<String, dynamic>?)>[];
    final board = await _board(['mission.verify'], calls: calls);
    await _detail(tester, board);
    expect(find.text('Redispatch as attempt 3'), findsOneWidget);
    await tester.tap(find.text('Redispatch as attempt 3'));
    await tester.pumpAndSettle();
    expect(find.text('Decide: redispatch as attempt 3'), findsOneWidget);
    await tester.enterText(find.byType(TextField).first, 'no branch');
    await tester.enterText(find.byType(TextField).last, 'Push it first.');
    await tester.pump();
    await tester.tap(find.text('Record it'));
    await tester.pumpAndSettle();
    expect(calls.single.$2, {
      'decision': 'redispatch',
      'reason': 'no branch',
      'amendment': 'Push it first.',
    });
  });

  testWidgets('a refusal is shown in the gateway\'s words', (tester) async {
    final board = await _board(
      ['mission.verify'],
      refuse: (409, 'Mission m1 is not verifying: its latest attempt is done'),
    );
    await _detail(tester, board);
    await tester.tap(find.text('Done'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'fine');
    await tester.pump();
    await tester.tap(find.text('Record it'));
    await tester.pumpAndSettle();
    expect(
      find.text('Mission m1 is not verifying: its latest attempt is done'),
      findsOneWidget,
    );
  });

  testWidgets('a forbidden probe is shown in the gateway\'s words', (
    tester,
  ) async {
    final board = await _board(
      ['mission.verify'],
      refuse: (403, 'not permitted: mission.verify on G'),
    );
    await _detail(tester, board);
    await tester.tap(find.text('Probe now'));
    await tester.pumpAndSettle();
    expect(find.text('not permitted: mission.verify on G'), findsOneWidget);
  });
}
