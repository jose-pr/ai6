import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart' show Dialog, Tab, TextField;
import 'package:outer_office/green_ink/green_ink_page.dart';
import 'package:outer_office/main.dart';

import 'shell_navigation.dart';

const requestId = 'green-ink-0123';
const classId = 'class-77';

Map<String, Object?> pendingRequest() => {
  'id': requestId,
  'operation': 'G',
  'mission': 'mission-007',
  'session': 'provider-session-007',
  'agent': '007',
  'scope': {'kind': 'push', 'target': 'origin:refs/heads/main'},
  'reason': 'The release branch passed review.',
  'requested_at': 1_798_000_000.0,
  'expires_at': 1_798_086_400.0,
};

Map<String, Object?> classAuthorization({Map<String, Object?>? revoked}) => {
  'id': classId,
  'operation': 'G',
  'kinds': ['push'],
  'target_pattern': 'origin:refs/heads/ai6/*',
  'agents': ['007'],
  'roles': <String>[],
  'clearance_min': 'field',
  'max_uses': 3,
  'uses': 1,
  'expires_at': 4_000_000_000.0,
  'signer': 'operator-m',
  'created_at': 1_798_000_000.0,
  'revoked': ?revoked,
};

/// A gateway with one request waiting for green ink in Operation G and one class
/// authorisation; every request is kept in [requests].
class FakeGreenInk {
  final requests = <http.Request>[];
  bool decided = false;
  bool revoked = false;

  http.Response answer(http.Request request) {
    final path = request.url.path;
    if (path.endsWith('/green-ink')) {
      return http.Response(jsonEncode(decided ? [] : [pendingRequest()]), 200);
    }
    if (path.endsWith('/approve') || path.endsWith('/reject')) {
      decided = true;
      return http.Response(jsonEncode({}), 200);
    }
    if (path.endsWith('/class-authorizations')) {
      return http.Response(
        jsonEncode([
          classAuthorization(
            revoked: revoked ? {'by': 'operator-007', 'at': 1.0} : null,
          ),
        ]),
        200,
      );
    }
    if (request.method == 'DELETE') {
      revoked = true;
      return http.Response(jsonEncode(classAuthorization()), 200);
    }
    throw StateError('unexpected request to $path');
  }
}

Future<MissionBoard> signedIn(
  FakeGreenInk gateway, {
  List<String> actions = const [
    'mission.read',
    'approval.decide',
    'approval.class',
  ],
}) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      gateway.requests.add(request);
      final path = request.url.path;
      if (path == '/v1/auth/login') {
        return http.Response(jsonEncode({'token': 'user-token'}), 201);
      }
      expect(request.headers['authorization'], 'Bearer user-token');
      if (path.startsWith('/v1/user/operations/')) {
        return gateway.answer(request);
      }
      return http.Response(switch (path) {
        '/v1/board' => jsonEncode([
          {
            'operation_id': 'G',
            'mission_id': 'mission-007',
            'session_id': 'provider-session-007',
            'state': 'blocked',
            'reason': 'green-ink',
            'sequence': 3,
            'updated_at': 100.0,
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
            {'operation_id': 'G', 'actions': actions},
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

Future<void> openAdministration(WidgetTester tester, MissionBoard board) async {
  tester.view.physicalSize = const Size(1400, 1000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await openPage(tester, 'Green Ink');
}

void main() {
  testWidgets('the board says a blocked Session waits for green ink', (
    tester,
  ) async {
    final gateway = FakeGreenInk();
    final board = await signedIn(gateway);
    addTearDown(board.dispose);
    expect(board.sessions.single.reason, 'green-ink');
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await openPage(tester, 'Missions');
    await tester.pumpAndSettle();
    expect(find.text('BLOCKED · GREEN INK'), findsOneWidget);
  });

  testWidgets('an operator signs a step for a chosen time with a note', (
    tester,
  ) async {
    final gateway = FakeGreenInk();
    final board = await signedIn(gateway);
    addTearDown(board.dispose);
    await openAdministration(tester, board);
    await tester.tap(find.widgetWithText(Tab, 'Inbox'));
    await tester.pumpAndSettle();

    expect(find.text('push · origin:refs/heads/main'), findsOneWidget);
    expect(
      find.text(
        'Agent 007 · Mission mission-007 · expires 2026-12-24T04:26:40Z',
      ),
      findsOneWidget,
    );
    expect(find.text('The release branch passed review.'), findsOneWidget);

    await tester.tap(find.text('Approve…'));
    await tester.pumpAndSettle();
    expectNoBackdrop(find.byType(Dialog));
    expectNoBackdrop(find.byType(GreenInkPage));
    await tester.tap(find.text('1 hour'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('4 hours').last);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'push only the tag');
    await tester.tap(find.text('Approve'));
    await tester.pumpAndSettle();

    final signed = gateway.requests.singleWhere(
      (request) => request.url.path.endsWith('/approve'),
    );
    expect(
      signed.url.path,
      '/v1/user/operations/G/green-ink/$requestId/approve',
    );
    expect(jsonDecode(signed.body), {
      'expires_in_secs': 4 * 60 * 60,
      'note': 'push only the tag',
    });
    expect(find.text('Signed in green ink.'), findsOneWidget);
    expect(find.text('No steps are waiting for green ink.'), findsOneWidget);
  });

  testWidgets('an operator rejects a step with a reason', (tester) async {
    final gateway = FakeGreenInk();
    final board = await signedIn(
      gateway,
      actions: const ['mission.read', 'approval.decide'],
    );
    addTearDown(board.dispose);
    await openAdministration(tester, board);
    expect(find.text('Class authorisations'), findsNothing);

    await tester.tap(find.text('Reject…'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'not before the audit');
    await tester.tap(find.text('Reject'));
    await tester.pumpAndSettle();
    final refused = gateway.requests.singleWhere(
      (request) => request.url.path.endsWith('/reject'),
    );
    expect(jsonDecode(refused.body), {'reason': 'not before the audit'});
  });

  testWidgets('class authorisations are listed and a live one is revoked', (
    tester,
  ) async {
    final gateway = FakeGreenInk();
    final board = await signedIn(
      gateway,
      actions: const ['mission.read', 'approval.class'],
    );
    addTearDown(board.dispose);
    await openAdministration(tester, board);
    expect(find.text('Green ink'), findsNothing);

    expect(find.text('push · origin:refs/heads/ai6/*'), findsOneWidget);
    expect(find.textContaining('1/3 uses'), findsOneWidget);
    expect(find.textContaining('signed by operator-m'), findsOneWidget);
    await tester.tap(find.text('Revoke'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Revoke').last);
    await tester.pumpAndSettle();

    final revoke = gateway.requests.singleWhere(
      (request) => request.method == 'DELETE',
    );
    expect(
      revoke.url.path,
      '/v1/user/operations/G/class-authorizations/$classId',
    );
    expect(find.text('REVOKED'), findsOneWidget);
  });

  testWidgets('without approval permissions there is nothing to sign', (
    tester,
  ) async {
    final gateway = FakeGreenInk();
    final board = await signedIn(gateway, actions: const ['mission.read']);
    addTearDown(board.dispose);
    expect(board.greenInkOperations, isEmpty);
    expect(board.classAuthorizationOperations, isEmpty);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.pumpAndSettle();
    expect(await offersPage(tester, 'Green Ink'), isFalse);
    expect(
      gateway.requests.where(
        (request) => request.url.path.startsWith('/v1/user/operations/'),
      ),
      isEmpty,
    );
  });
}
