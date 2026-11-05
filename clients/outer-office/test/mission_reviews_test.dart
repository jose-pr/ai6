import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/reviews/mission_reviews.dart';

const _reviews = [
  {
    'gate': 'output',
    'policy': 'two-models',
    'pass_at': 0.66,
    'distinct_models': 2,
    'rounds': 2,
    'redo_limit': 1,
    'status': 'person',
    'why': 'the panel did not agree',
    'iteration': 2,
    'round': 1,
    'sent_back': 1,
    'panel': [
      {
        'seat': 0,
        'agent': 'r1',
        'role': 'reviewer',
        'weight': 2,
        'veto': true,
        'model': 'gemini-free',
      },
      {
        'seat': 1,
        'agent': 'Tanner',
        'weight': 1,
        'veto': false,
        'model': 'haiku',
      },
    ],
    'votes': [
      {
        'iteration': 1,
        'round': 1,
        'agent': 'r1',
        'model': 'gemini-free',
        'weight': 2,
        'vote': 'redo',
        'reasons': ['no test for the new path'],
        'at': 1.0,
      },
      {
        'iteration': 2,
        'round': 1,
        'agent': 'Tanner',
        'model': 'haiku',
        'weight': 1,
        'vote': 'absent',
        'reasons': ['it gave no answer within 3600 seconds'],
        'at': 2.0,
      },
    ],
    'turns': 3,
    'tokens': 1200,
    'overrule': null,
  },
];

Future<MissionBoard> _board(
  List<String> actions,
  List<http.Request> seen,
) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      seen.add(request);
      final path = request.url.path;
      http.Response json(Object body, [int status = 200]) => http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json'},
      );
      return switch (path) {
        '/v1/auth/login' => json({'token': 'user-token'}, 201),
        '/v1/board' || '/v1/missions' => json(const <Object>[]),
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
        '/v1/user/operations/G/missions/m1/reviews' => json(_reviews),
        '/v1/user/operations/G/missions/m1/reviews/output/overrule' => json(
          _reviews.first,
        ),
        _ => json(const <Object>[]),
      };
    }),
  );
  await board.login('operator', 'secret');
  addTearDown(board.dispose);
  return board;
}

Future<void> _show(WidgetTester tester, double width, Widget child) async {
  tester.view.physicalSize = Size(width, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(body: SingleChildScrollView(child: child)),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  for (final width in [390.0, 1280.0]) {
    testWidgets(
      'the panel, weights, votes, reasons and rounds show at $width px',
      (tester) async {
        final board = await _board(['mission.read'], <http.Request>[]);
        await _show(
          tester,
          width,
          MissionReviewsSection(
            board: board,
            operationId: 'G',
            missionId: 'm1',
          ),
        );
        expect(tester.takeException(), isNull, reason: 'no overflow at $width');
        expect(find.text('REVIEW'), findsOneWidget);
        expect(find.text('Output: waiting for a person'), findsOneWidget);
        expect(find.text('the panel did not agree'), findsOneWidget);
        expect(find.text('r1 (gemini-free), weight 2, veto'), findsOneWidget);
        expect(
          find.textContaining(
            'r1 voted redo (weight 2): no test for the new path',
          ),
          findsOneWidget,
        );
        expect(find.textContaining('Tanner voted absent'), findsOneWidget);
        expect(
          find.text('Pass it'),
          findsNothing,
          reason: 'reading is not deciding',
        );
      },
    );
  }

  testWidgets('a profile that holds mission.review offers the overrule', (
    tester,
  ) async {
    final board = await _board(['mission.read', 'mission.review'], []);
    expect(board.hasOperationAction('G', 'mission.review'), isTrue);
    await _show(
      tester,
      1280,
      MissionReviewsSection(board: board, operationId: 'G', missionId: 'm1'),
    );
    expect(find.text('Pass it'), findsOneWidget);
    expect(find.text('Send it back'), findsOneWidget);
    expect(find.text('Refuse it'), findsOneWidget);
  });

  testWidgets(
    'a person with mission.review decides over the panel with a reason',
    (tester) async {
      final seen = <http.Request>[];
      final board = await _board(['mission.read', 'mission.review'], seen);
      await _show(
        tester,
        1280,
        MissionReviewsSection(board: board, operationId: 'G', missionId: 'm1'),
      );
      await tester.tap(find.text('Send it back'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), 'add the test');
      await tester.tap(find.text('Decide'));
      await tester.pumpAndSettle();
      final decided = seen.lastWhere(
        (request) => request.url.path.endsWith('/overrule'),
      );
      expect(decided.method, 'POST');
      expect(jsonDecode(decided.body), {
        'decision': 'redo',
        'reason': 'add the test',
      });
    },
  );
}
