import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart' show TextField;
import 'package:outer_office/controls/mission_actions.dart';
import 'package:outer_office/main.dart';

/// A signed-in board whose gateway shows one Mission with an earlier, queued attempt and a
/// later, working one; every request is kept in [requests].
Future<MissionBoard> signedIn(
  List<http.Request> requests, {
  List<String> actions = const ['mission.read', 'mission.control'],
  Map<String, Object?> Function(Map<String, dynamic> body)? control,
  String current = 'working',
}) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      requests.add(request);
      final path = request.url.path;
      if (path == '/v1/auth/login') {
        return http.Response(jsonEncode({'token': 'user-token'}), 201);
      }
      if (path.endsWith('/actions')) {
        final body = jsonDecode(request.body) as Map<String, dynamic>;
        return http.Response(jsonEncode(control!(body)), 202);
      }
      if (path == '/v1/attach-grants' || path == '/v1/user/attach-grants') {
        return http.Response('refused', 403);
      }
      return http.Response(switch (path) {
        '/v1/board' => jsonEncode([
          {
            'operation_id': 'operation-alpha',
            'mission_id': 'mission-007',
            'session_id': 'claude-session-001',
            'state': 'queued',
            'sequence': 9,
            'updated_at': 100.0,
          },
          {
            'operation_id': 'operation-alpha',
            'mission_id': 'mission-007',
            'session_id': 'claude-session-002',
            'state': current,
            'sequence': 2,
            'updated_at': 200.0,
          },
        ]),
        '/v1/missions' => '[]',
        '/v1/stations' => jsonEncode({
          'headquarters_station': null,
          'stations': [],
        }),
        '/v1/user/profile' => jsonEncode({
          'person_id': 'operator-007',
          'name': 'operator',
          'display_name': null,
          'groups': [],
          'expires_at': 1_900_000_000,
          'operations': [
            {'operation_id': 'operation-alpha', 'actions': actions},
          ],
          'stations': [],
        }),
        _ => throw StateError('unexpected request to $path'),
      }, 200);
    }),
  );
  await board.login('operator', 'secret');
  return board;
}

Map<String, Object?> outcome(
  String action,
  String status, {
  String? state,
  String? reason,
}) => {
  'control_id': 'control-1',
  'operation_id': 'operation-alpha',
  'mission_id': 'mission-007',
  'action': action,
  'station_id': 'station-alpha',
  'status': status,
  'state': state,
  'reason': reason,
};

void main() {
  test('each control is offered only in the states its Station accepts', () {
    Map<String, String?> offered(String? state, {bool permitted = true}) => {
      for (final option in missionControls(state: state, permitted: permitted))
        option.action: option.unavailable,
    };

    final working = offered('working');
    expect(working['cancel'], isNull);
    expect(working['interrupt'], isNull);
    expect(working['resume'], contains('parked, blocked or stalled'));
    expect(working['retry'], contains('stalled or finished'));

    final stalled = offered('stalled');
    expect(stalled['retry'], isNull);
    expect(stalled['resume'], isNull);
    expect(stalled['interrupt'], contains('working Session'));

    final failed = offered('failed');
    expect(failed['retry'], isNull);
    expect(failed['cancel'], 'The Mission already ended (failed).');
    expect(failed['resume'], isNotNull);

    expect(offered('queued')['retry'], 'The Mission is already queued.');
    expect(offered('quarantined')['retry'], contains('release its Agent'));
    expect(offered('parked')['resume'], isNull);

    final denied = offered('working', permitted: false);
    expect(denied.values.toSet(), {
      'Your account lacks mission.control in this Operation.',
    });
  });

  test('the most recently updated Session carries the Mission state', () {
    const earlier = SessionSummary(
      operationId: 'G',
      missionId: 'm',
      sessionId: 'first',
      state: 'queued',
      sequence: 40,
      updatedAt: 10,
    );
    const later = SessionSummary(
      operationId: 'G',
      missionId: 'm',
      sessionId: 'second',
      state: 'working',
      sequence: 1,
      updatedAt: 20,
    );
    expect(currentSession([earlier, later])?.sessionId, 'second');
    expect(currentSession([later, earlier])?.sessionId, 'second');
    expect(currentSession(const []), isNull);
  });

  testWidgets('the action menu explains unavailable controls and interrupts', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(900, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final requests = <http.Request>[];
    final board = await signedIn(
      requests,
      control: (body) {
        expect(body['action'], 'interrupt');
        expect(body['session_id'], 'claude-session-002');
        expect(body['idempotency_key'], startsWith('oo-'));
        return outcome('interrupt', 'applied', state: 'blocked');
      },
    );
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );

    await tester.tap(find.byTooltip('Mission actions'));
    await tester.pumpAndSettle();
    expect(find.text('Cancel Mission'), findsOneWidget);
    expect(find.text('Interrupt Session'), findsOneWidget);
    expect(
      find.text(
        'Only a stalled or finished Mission can be retried; cancel it first.',
      ),
      findsOneWidget,
    );
    expect(
      find.text('Only a parked, blocked or stalled Mission can resume.'),
      findsOneWidget,
    );
    // A disabled entry does nothing.
    await tester.tap(find.text('Resume Mission'), warnIfMissed: false);
    await tester.pumpAndSettle();
    expect(find.text('Resume Mission mission-007?'), findsNothing);

    await tester.tap(find.text('Interrupt Session'));
    await tester.pumpAndSettle();
    expect(find.text('Interrupt the Session?'), findsOneWidget);
    await tester.tap(find.text('Interrupt'));
    await tester.pumpAndSettle();

    expect(
      requests.where((request) => request.url.path.endsWith('/actions')),
      hasLength(1),
    );
    expect(
      requests.last.url.path,
      isNot(endsWith('/actions')),
      reason: 'the board refreshes after the outcome',
    );
    expect(
      find.text('Interrupt Session applied · Mission is blocked'),
      findsNWidgets(2),
    );
  });

  testWidgets('a cancel the Station rejects says why', (tester) async {
    final requests = <http.Request>[];
    final board = await signedIn(
      requests,
      control: (body) {
        expect(body, containsPair('action', 'cancel'));
        expect(body.containsKey('session_id'), isFalse);
        return outcome(
          'cancel',
          'rejected',
          state: 'working',
          reason: 'Station "station-beta" holds this Mission\'s Session',
        );
      },
    );
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );

    await tester.tap(find.byTooltip('Mission actions'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cancel Mission'));
    await tester.pumpAndSettle();
    expect(find.text('Cancel Mission mission-007?'), findsOneWidget);
    expect(find.byType(TextField), findsNothing);
    await tester.tap(find.text('Cancel Mission').last);
    await tester.pumpAndSettle();
    expect(
      find.text(
        'Cancel Mission rejected by station-alpha: '
        'Station "station-beta" holds this Mission\'s Session',
      ),
      findsWidgets,
    );
  });

  testWidgets('a retry of a failed Mission names the Agent it goes to', (
    tester,
  ) async {
    final requests = <http.Request>[];
    final board = await signedIn(
      requests,
      current: 'failed',
      control: (body) {
        expect(body['action'], 'retry');
        expect(body['agent'], '008');
        expect(body.containsKey('message'), isFalse);
        return outcome('retry', 'applied', state: 'queued');
      },
    );
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );

    await tester.tap(find.byTooltip('Mission actions'));
    await tester.pumpAndSettle();
    expect(find.text('The Mission already ended (failed).'), findsOneWidget);
    await tester.tap(find.text('Retry Mission'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), ' 008 ');
    await tester.tap(find.text('Retry'));
    await tester.pumpAndSettle();
    expect(
      find.text('Retry Mission applied · Mission is queued'),
      findsWidgets,
    );
  });

  testWidgets('without mission.control every control is disabled', (
    tester,
  ) async {
    final requests = <http.Request>[];
    final board = await signedIn(requests, actions: const ['mission.read']);
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );

    await tester.tap(find.byTooltip('Mission actions'));
    await tester.pumpAndSettle();
    expect(
      find.text('Your account lacks mission.control in this Operation.'),
      findsNWidgets(4),
    );
    await tester.tap(find.text('Cancel Mission'), warnIfMissed: false);
    await tester.pumpAndSettle();
    expect(find.text('Cancel Mission mission-007?'), findsNothing);
    expect(
      requests.where((request) => request.url.path.endsWith('/actions')),
      isEmpty,
    );
  });

  testWidgets('closing a Session view sends no control', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final requests = <http.Request>[];
    final board = await signedIn(requests);
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );

    await tester.tap(find.text('claude-session-002'));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Close Session details'));
    await tester.pumpAndSettle();
    expect(
      requests.where((request) => request.url.path.endsWith('/actions')),
      isEmpty,
    );
  });
}
