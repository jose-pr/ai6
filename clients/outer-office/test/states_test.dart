import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/controls/mission_actions.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/oo_freshness.dart';
import 'package:outer_office/shell/top_bar.dart';

import 'routes_test.dart' show locations;

const _jsonHeaders = {'content-type': 'application/json'};

/// A gateway with one Operation, one working Session, one held brief and one green ink
/// request waiting, which each test turns into the failure it needs. Every request is kept in
/// [requests].
class Gateway {
  final requests = <http.Request>[];

  /// The gateway does not answer at all.
  bool down = false;

  /// Status of `/v1/board` and of the held briefs.
  int boardStatus = 200;
  int heldStatus = 200;

  List<Map<String, Object?>> held = [_held];
  List<Map<String, Object?>> greenInk = [_greenInk];
  List<Map<String, Object?>> sessions = [
    {
      'operation_id': 'operation-alpha',
      'mission_id': 'mission-007',
      'session_id': 'session-1',
      'state': 'working',
      'sequence': 1,
      'updated_at': 100.0,
    },
  ];

  /// Seconds since the epoch the Station last checked in.
  double stationSeen = 1_900_000_000;

  int posts() =>
      requests.where((request) => request.method == 'POST').length -
      requests.where((request) => request.url.path == '/v1/auth/login').length;

  Future<http.Response> handle(http.Request request) async {
    requests.add(request);
    if (down) throw http.ClientException('connection refused');
    final path = request.url.path;
    http.Response ok(Object body) =>
        http.Response(jsonEncode(body), 200, headers: _jsonHeaders);
    const actions = '/v1/user/operations/operation-alpha';
    if (path == '/v1/auth/login') {
      return http.Response(
        jsonEncode({'token': 'user-token'}),
        201,
        headers: _jsonHeaders,
      );
    }
    if (path == '/v1/board') {
      return boardStatus == 200 ? ok(sessions) : http.Response('', boardStatus);
    }
    if (path == '/v1/missions') return ok(<Object>[]);
    if (path == '/v1/stations') {
      return ok({
        'headquarters_station': 'hq',
        'stations': [
          {
            'station_id': 'hq',
            'healthy': true,
            'headquarters': true,
            'seen': stationSeen,
            'roles': ['control'],
            'running': 1,
            'capacity': 4,
            'operations': ['operation-alpha'],
          },
        ],
      });
    }
    if (path == '/v1/user/profile') {
      return ok({
        'person_id': 'operator-007',
        'name': 'operator',
        'display_name': null,
        'groups': <Object>[],
        'expires_at': 1_900_000_000,
        'operations': [
          {
            'operation_id': 'operation-alpha',
            'actions': [
              'mission.read',
              'mission.control',
              'mission.create',
              'approval.decide',
            ],
          },
        ],
        'stations': <Object>[],
      });
    }
    if (path == '$actions/held-briefs') {
      return heldStatus == 200 ? ok(held) : http.Response('', heldStatus);
    }
    if (path == '$actions/green-ink') return ok(greenInk);
    if (path == '$actions/mission-proposals') return ok(<Object>[]);
    if (path.endsWith('/missions/mission-007/actions') &&
        request.method == 'POST') {
      return http.Response(
        jsonEncode(_control('queued')),
        202,
        headers: _jsonHeaders,
      );
    }
    if (path.endsWith('/actions/control-1')) return ok(_control('queued'));
    return http.Response('', 404);
  }
}

const _held = {
  'key': 'source_github--org_repo_7',
  'operation': 'operation-alpha',
  'brief': {'title': 'Fix the login page', 'body': 'It crashes.'},
  'vetting': {'findings': <Object>[]},
  'held_at': 1_900_000_000.0,
};

const _greenInk = {
  'id': 'green-ink-1',
  'operation': 'operation-alpha',
  'mission': 'mission-007',
  'session': 'session-9',
  'agent': '007',
  'scope': {'kind': 'push', 'target': 'origin:refs/heads/main'},
  'reason': 'Ship it.',
  'requested_at': 1_900_000_000.0,
  'expires_at': 1_900_086_400.0,
};

Map<String, Object?> _control(String status) => {
  'control_id': 'control-1',
  'operation_id': 'operation-alpha',
  'mission_id': 'mission-007',
  'action': 'interrupt',
  'station_id': 'hq',
  'status': status,
};

Future<MissionBoard> signedIn(Gateway gateway) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient(gateway.handle),
  );
  await board.login('operator', 'secret');
  addTearDown(board.dispose);
  return board;
}

Future<List<MethodCall>> openAt(
  WidgetTester tester,
  MissionBoard board,
  String location, {
  Size size = const Size(1400, 1000),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  tester.binding.platformDispatcher.defaultRouteNameTestValue = location;
  addTearDown(tester.binding.platformDispatcher.clearDefaultRouteNameTestValue);
  final reported = <MethodCall>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.navigation,
    (call) async {
      reported.add(call);
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.navigation,
      null,
    ),
  );
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  return reported;
}

/// Reads the board again and lets the app show the result.
Future<void> refresh(WidgetTester tester, MissionBoard board) async {
  await board.refresh();
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('a refresh failing after a good one keeps the Sessions and says '
      'when they were read', (tester) async {
    final gateway = Gateway();
    final board = await signedIn(gateway);
    await openAt(tester, board, '/missions');
    expect(find.text('session-1'), findsOneWidget);
    expect(find.byType(GatewayStrip), findsOneWidget);
    expect(find.textContaining('unreachable'), findsNothing);
    final at = ooClockTime(board.lastUpdated(BoardSource.board)!);

    gateway.down = true;
    await refresh(tester, board);
    expect(find.text('session-1'), findsOneWidget, reason: 'the data stays');
    expect(board.freshness(BoardSource.board), OoFreshness.stale);
    expect(find.textContaining('Sessions: Last updated $at'), findsOneWidget);
    expect(
      find.text(
        'Universal Exports unreachable, showing data from $at. Retrying.',
      ),
      findsOneWidget,
    );

    gateway.down = false;
    await refresh(tester, board);
    expect(find.textContaining('unreachable'), findsNothing);
    expect(find.textContaining('Last updated'), findsNothing);
    expect(board.freshness(BoardSource.board), OoFreshness.live);
  });

  testWidgets('a 403 on held briefs hides them and Overview says what it could '
      'not read', (tester) async {
    final gateway = Gateway()..greenInk = [];
    final board = await signedIn(gateway);
    await openAt(tester, board, '/');
    expect(find.textContaining('Fix the login page'), findsWidgets);

    gateway
      ..heldStatus = 403
      ..held = [];
    await refresh(tester, board);
    expect(board.heldBriefs, isEmpty);
    expect(board.freshness(BoardSource.heldBriefs), OoFreshness.noAccess);
    expect(find.textContaining('Fix the login page'), findsNothing);
    expect(find.textContaining('Nothing needs your attention'), findsNothing);
    expect(
      find.text(
        'Nothing found in what loaded. Could not be read: Held briefs.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('Overview says nothing needs the person only when every source '
      'was read', (tester) async {
    final gateway = Gateway()
      ..held = []
      ..greenInk = [];
    final board = await signedIn(gateway);
    await openAt(tester, board, '/');
    expect(find.text('Nothing needs your attention.'), findsOneWidget);
    expect(find.text('Nothing found in what loaded.'), findsNothing);
  });

  testWidgets('a 401 ends the sign-in and leaves no board data behind', (
    tester,
  ) async {
    final gateway = Gateway();
    final board = await signedIn(gateway);
    // Open a Mission's detail so there is a view over the data.
    await openAt(
      tester,
      board,
      '/missions?operation_id=operation-alpha&mission_id=mission-007',
    );
    expect(find.text('session-1'), findsWidgets);

    gateway.boardStatus = 401;
    await refresh(tester, board);
    expect(board.accessToken, isNull);
    expect(board.sessions, isEmpty);
    expect(board.heldBriefs, isEmpty);
    expect(board.greenInkRequests, isEmpty);
    expect(board.userSession, isNull);
    expect(find.text('Continue'), findsOneWidget, reason: 'the sign-in page');
    expect(find.textContaining('session-1'), findsNothing);
    expect(find.textContaining('mission-007'), findsNothing);
    expect(find.textContaining('Fix the login page'), findsNothing);
    expect(find.text('Your sign-in ended. Sign in again.'), findsOneWidget);
  });

  testWidgets('a waiting green ink request is in the bell and opens its '
      'request', (tester) async {
    final gateway = Gateway()..held = [];
    final board = await signedIn(gateway);
    final reported = await openAt(tester, board, '/');
    final bell = find.byTooltip('Needs attention: 1');
    expect(bell, findsOneWidget);
    await tester.tap(bell);
    await tester.pumpAndSettle();
    const text =
        'Agent 007 waits for green ink to push origin:refs/heads/main in '
        'Operation operation-alpha';
    final item = find.descendant(
      of: find.byType(MenuItemButton),
      matching: find.text(text),
    );
    expect(item, findsOneWidget);
    await tester.tap(item);
    await tester.pumpAndSettle();
    expect(
      locations(reported).last.$1,
      '/green_ink?operation_id=operation-alpha&request_id=green-ink-1',
    );
  });

  testWidgets('a control still open after ten seconds is an unknown outcome, '
      'checked again without asking twice', (tester) async {
    final gateway = Gateway()
      ..held = []
      ..greenInk = [];
    final board = await signedIn(gateway);
    await openAt(
      tester,
      board,
      '/missions?operation_id=operation-alpha&mission_id=mission-007',
      size: const Size(1400, 1600),
    );

    await tester.tap(find.byTooltip('Mission actions').last);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Interrupt Session'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Interrupt'));
    await tester.pump();
    expect(board.requests.values.single.open, isTrue);
    for (var i = 0; i < 24; i++) {
      await tester.pump(const Duration(milliseconds: 500));
    }
    await tester.pumpAndSettle();
    final request = board.requests.values.single;
    expect(request.stage, ControlStage.unknown);
    expect(gateway.posts(), 1);

    // The Mission's own menu cannot start another.
    await tester.tap(find.byTooltip('Mission actions').last);
    await tester.pumpAndSettle();
    expect(find.text('A request is in progress'), findsNWidgets(4));
    await tester.tapAt(const Offset(5, 5));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('Needs attention'));
    await tester.pumpAndSettle();
    expect(find.text('YOUR REQUESTS'), findsOneWidget);
    expect(
      find.text(
        'Interrupt Session · Operation operation-alpha › mission-007 · '
        'Outcome unknown',
      ),
      findsOneWidget,
    );
    final before = gateway.requests.length;
    await tester.tap(find.text('Check again'));
    await tester.pumpAndSettle();
    final since = gateway.requests.skip(before).toList();
    expect(since, hasLength(1));
    expect(since.single.method, 'GET');
    expect(since.single.url.path.endsWith('/actions/control-1'), isTrue);
    expect(gateway.posts(), 1, reason: 'never posted again');
  });

  testWidgets('a request that was applied no longer disables the menu', (
    tester,
  ) async {
    final gateway = Gateway();
    final board = await signedIn(gateway);
    await openAt(tester, board, '/missions');
    expect(board.requestInProgress('operation-alpha', 'mission-007'), isFalse);
    expect(board.openRequests, isEmpty);
  });

  testWidgets('search with no match says what it covers', (tester) async {
    final board = await signedIn(Gateway());
    await openAt(tester, board, '/');
    await tester.tap(find.text(OoSearch.hint));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).last, 'zzz');
    await tester.pumpAndSettle();
    expect(find.text('No matches in the loaded board'), findsOneWidget);
    await tester.enterText(find.byType(TextField).last, 'session-1');
    await tester.pumpAndSettle();
    expect(find.text('Results in the loaded board'), findsOneWidget);
    expect(find.text('Session session-1'), findsOneWidget);
  });

  testWidgets('Ctrl+K opens search from the page and from a field', (
    tester,
  ) async {
    final board = await signedIn(Gateway());
    await openAt(tester, board, '/');
    final field = find.byType(TextField);
    expect(field, findsOneWidget, reason: "Moneypenny's field on Overview");
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(field, findsNWidgets(2), reason: 'the search view opened');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(field, findsOneWidget);

    // A key press without a character, in a text field, is the shortcut as well.
    await tester.tap(field);
    await tester.pump();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(field, findsNWidgets(2));
  });

  testWidgets('the same keys typing a character in a text field do not open '
      'search', (tester) async {
    final board = await signedIn(Gateway());
    await openAt(tester, board, '/');
    final field = find.byType(TextField);
    await tester.tap(field);
    await tester.pump();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    // AltGr+K arrives as Ctrl+Alt+K with its own character on some layouts.
    await tester.sendKeyDownEvent(LogicalKeyboardKey.keyK, character: 'q');
    await tester.sendKeyUpEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(field, findsOneWidget, reason: 'no search view');
  });

  testWidgets('the demo says it is sample data', (tester) async {
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      demo: true,
      pollInterval: null,
    );
    addTearDown(board.dispose);
    await openAt(tester, board, '/');
    expect(find.text('Demo data'), findsOneWidget);
  });

  testWidgets('a Station silent for more than three board intervals says so', (
    tester,
  ) async {
    final clock = ooClock;
    addTearDown(() => ooClock = clock);
    ooClock = () => DateTime.fromMillisecondsSinceEpoch(1_900_000_050 * 1000);
    final gateway = Gateway()..stationSeen = 1_900_000_000;
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      client: MockClient(gateway.handle),
    );
    addTearDown(board.dispose);
    await board.login('operator', 'secret');
    await openAt(tester, board, '/stations?station_id=hq');
    expect(find.text('Online'), findsOneWidget);
    expect(find.text('No check-in for 50 s'), findsOneWidget);
    expect(find.text('Last check-in 50 s ago'), findsOneWidget);

    gateway.stationSeen = 1_900_000_045;
    await refresh(tester, board);
    expect(find.textContaining('No check-in for'), findsNothing);
    expect(find.text('Last check-in 5 s ago'), findsOneWidget);
  });

  testWidgets('no visible Operation is said without saying whether any exist', (
    tester,
  ) async {
    final gateway = Gateway();
    final board = await signedIn(gateway);
    board.userSession = null;
    await openAt(tester, board, '/');
    expect(
      find.text(
        'No Operations you can see. A controller grants access to Operations.',
      ),
      findsWidgets,
    );
    expect(find.textContaining('exist'), findsNothing);
  });
}
