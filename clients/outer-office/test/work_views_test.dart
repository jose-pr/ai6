import 'dart:convert';

import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/mission_detail.dart';
import 'package:outer_office/shell/missions_board.dart';
import 'package:outer_office/shell/oo_freshness.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;
import 'package:outer_office/shell/overview_page.dart';
import 'package:outer_office/shell/station_watch.dart';

import 'routes_test.dart' show locations;

/// The app on [board] opened at the web location [location], in a window [size] wide; the
/// locations the app reports go to the returned list.
Future<List<MethodCall>> openAt(
  WidgetTester tester,
  MissionBoard board,
  String location, {
  Size size = const Size(1400, 900),
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

MissionBoard demoBoard() {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    demo: true,
    pollInterval: null,
  );
  addTearDown(board.dispose);
  return board;
}

/// A signed-in board with one queued Mission whose Operation's placement preview answers
/// with [preview] (status and body).
Future<MissionBoard> queuedBoard((int, String) preview) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      final path = request.url.path;
      return switch (path) {
        '/v1/auth/login' => http.Response(
          jsonEncode({'token': 'user-token'}),
          201,
        ),
        '/v1/board' => http.Response(
          jsonEncode([
            {
              'operation_id': 'operation-alpha',
              'mission_id': 'mission-007',
              'session_id': 'session-1',
              'state': 'queued',
              'sequence': 1,
              'updated_at': 100.0,
            },
          ]),
          200,
          headers: {'content-type': 'application/json'},
        ),
        '/v1/missions' => http.Response('[]', 200),
        '/v1/stations' => http.Response(
          jsonEncode({'headquarters_station': null, 'stations': []}),
          200,
        ),
        '/v1/user/profile' => http.Response(
          jsonEncode({
            'person_id': 'operator-007',
            'name': 'operator',
            'display_name': null,
            'groups': [],
            'expires_at': 1_900_000_000,
            'operations': [
              {
                'operation_id': 'operation-alpha',
                'actions': ['mission.read'],
              },
            ],
            'stations': [],
          }),
          200,
        ),
        '/v1/user/operations/operation-alpha/placement-preview' =>
          http.Response(
            preview.$2,
            preview.$1,
            headers: {'content-type': 'application/json'},
          ),
        _ => throw StateError('unexpected request to $path'),
      };
    }),
  );
  await board.login('operator', 'secret');
  addTearDown(board.dispose);
  return board;
}

/// The button the focus is in, when it is in one.
Widget? focusedButton() {
  Widget? found;
  FocusManager.instance.primaryFocus?.context?.visitAncestorElements((element) {
    if (element.widget is ButtonStyleButton) {
      found = element.widget;
      return false;
    }
    return true;
  });
  return found;
}

/// The button labelled [label].
Finder button(String label) => find.ancestor(
  of: find.text(label),
  matching: find.bySubtype<ButtonStyleButton>(),
);

void main() {
  test(
    'relative times read from seconds to days, exact times carry the zone',
    () {
      final now = DateTime.fromMillisecondsSinceEpoch(1_800_000_000 * 1000);
      String relative(double ago) =>
          ooRelativeTime(1_800_000_000 - ago, now: now);
      expect(relative(1), 'just now');
      expect(relative(12), '12 s ago');
      expect(relative(300), '5 min ago');
      expect(relative(3 * 3600), '3 h ago');
      expect(relative(2 * 86400), '2 d ago');
      expect(relative(-30), 'in 30 s', reason: 'a deadline');
      expect(relative(-2), 'just now', reason: 'clocks differ a little');
      expect(
        ooExactTime(1_800_000_000),
        matches(
          RegExp(r'^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d \(UTC[+-]\d\d:\d\d\)$'),
        ),
      );
    },
  );

  testWidgets(
    'the queued filter counts held briefs and proposals, which are never rows',
    (tester) async {
      final board = demoBoard();
      await openAt(tester, board, '/missions?state=queued');
      expect(find.text('Also waiting:'), findsOneWidget);
      expect(find.text('1 held brief'), findsOneWidget);
      expect(find.text('1 Mission proposal'), findsOneWidget);
      expect(find.byType(MissionCard), findsNothing);
      expect(find.textContaining('Resolve github item'), findsNothing);
      expect(find.text('Review the role catalog'), findsNothing);

      await tester.tap(find.text('1 held brief'));
      await tester.pumpAndSettle();
      expect(find.text('Held briefs'), findsWidgets);
    },
  );

  testWidgets('without the queued state the strip is not shown', (
    tester,
  ) async {
    await openAt(tester, demoBoard(), '/missions');
    expect(find.text('Also waiting:'), findsNothing);
  });

  testWidgets(
    'equal update times keep the rows in Operation and Mission order',
    (tester) async {
      final board = demoBoard();
      SessionSummary session(String operation, String mission, double? at) =>
          SessionSummary(
            operationId: operation,
            missionId: mission,
            sessionId: 's-$operation-$mission',
            state: 'working',
            sequence: 1,
            updatedAt: at,
          );
      final sessions = [
        session('G', 'm-b', 50),
        session('G', 'm-a', 50),
        session('L', 'm-c', 50),
        session('G', 'm-new', 90),
      ];
      List<String> order() => [
        for (final mission in routeMissions(
          board,
          const OoRoute(OoPage.missions),
        ))
          mission.missionId,
      ];
      board.sessions = sessions;
      final first = order();
      expect(first, ['m-new', 'm-a', 'm-b', 'm-c'], reason: 'newest first');
      board.sessions = sessions.reversed.toList();
      expect(
        order(),
        first,
        reason: 'the board delivered them in another order',
      );
      board.sessions = sessions;
      expect(
        [
          for (final mission in routeMissions(
            board,
            OoRoute.parseUri('app:/missions?sort=updated&order=asc'),
          ))
            mission.missionId,
        ],
        ['m-a', 'm-b', 'm-c', 'm-new'],
        reason: 'ties keep their order when the route reverses the times',
      );

      await openAt(tester, board, '/missions', size: const Size(1400, 1600));
      double top(String mission) =>
          tester.getTopLeft(find.text('Mission $mission')).dy;
      expect(top('m-new'), lessThan(top('m-a')));
      expect(top('m-a'), lessThan(top('m-b')));
      board.sessions = sessions.reversed.toList();
      // A refresh with the same data in another order leaves the rows where they are.
      board.refresh();
      await tester.pumpAndSettle();
      expect(top('m-new'), lessThan(top('m-a')));
      expect(top('m-a'), lessThan(top('m-b')));
      expect(top('m-b'), lessThan(top('m-c')));
    },
  );

  testWidgets(
    "the Missions tile's link shows the Missions it counted, by their latest Session",
    (tester) async {
      final board = demoBoard();
      SessionSummary session(
        String mission,
        String id,
        String state,
        int sequence,
        double updatedAt,
      ) => SessionSummary(
        operationId: 'G',
        missionId: mission,
        sessionId: id,
        state: state,
        sequence: sequence,
        updatedAt: updatedAt,
      );
      board.sessions = [
        session('m-finished', 's-older', 'blocked', 4, 100),
        session('m-finished', 's-latest', 'done', 7, 200),
        session('m-open', 's-open', 'working', 2, 300),
        // A long failed first attempt: its event count says nothing about the retry after it.
        session('m-retry', 's-first', 'failed', 40, 400),
        session('m-retry', 's-retry', 'working', 2, 500),
      ];
      await openAt(tester, board, '/overview', size: const Size(1400, 1600));
      final overview = find.byType(OverviewPage);
      final tile = find.descendant(
        of: overview,
        matching: find.text('Missions'),
      );
      expect(find.text('2 working · 3 on the board'), findsOneWidget);
      await tester.tap(tile.first);
      await tester.pumpAndSettle();
      expect(find.text('Mission m-open'), findsOneWidget);
      expect(find.text('Mission m-retry'), findsOneWidget);
      expect(find.text('Mission m-finished'), findsNothing);
      expect(
        find.text('All  3'),
        findsOneWidget,
        reason: 'the chips count Missions, not Sessions',
      );
      expect(find.text('blocked  0'), findsOneWidget);
      expect(find.text('done  1'), findsOneWidget);
      await tester.tap(find.text('done  1'));
      await tester.pumpAndSettle();
      expect(find.text('Mission m-finished'), findsOneWidget);
      expect(find.text('s-older'), findsOneWidget, reason: 'all its Sessions');
      expect(find.text('s-latest'), findsOneWidget);
    },
  );

  test('a Station watch names the Session updated last, not the longest', () {
    final board = demoBoard();
    SessionSummary session(String id, int sequence, double updatedAt) =>
        SessionSummary(
          operationId: 'G',
          missionId: 'm',
          sessionId: id,
          state: 'working',
          sequence: sequence,
          updatedAt: updatedAt,
          stationId: 'hq',
        );
    board.sessions = [session('s-first', 40, 100), session('s-retry', 2, 200)];
    board.missionAttempts = {
      ('G', 'm'): const [
        MissionAttemptSummary(attempt: 2, station: 'hq', status: 'active'),
      ],
    };
    expect(sessionsHere(board, 'hq').single.$2?.sessionId, 's-retry');
  });

  group('a Session row', () {
    const short = 's-1234';
    final long = 'a' * 40;

    SessionSummary row(String id) => SessionSummary(
      operationId: 'G',
      missionId: 'm-$id',
      sessionId: id,
      state: 'blocked',
      reason: 'green-ink',
      sequence: 3,
      exposure: const ['web'],
    );

    int lines(WidgetTester tester, String text) {
      final paragraph = tester.renderObject<RenderParagraph>(
        find.descendant(of: find.text(text), matching: find.byType(RichText)),
      );
      final boxes = paragraph.getBoxesForSelection(
        TextSelection(baseOffset: 0, extentOffset: text.length),
      );
      return {for (final box in boxes) box.top}.length;
    }

    Rect exposed(WidgetTester tester, String id) => tester.getRect(
      find
          .descendant(
            of: find
                .ancestor(of: find.text(id), matching: find.byType(InkWell))
                .first,
            matching: find.byType(Chip),
          )
          .first,
    );

    testWidgets('at 390 px its ID reads across the row, the chips under it', (
      tester,
    ) async {
      final board = demoBoard();
      board.sessions = [row(short), row(long)];
      await openAt(tester, board, '/missions', size: const Size(390, 2400));
      expect(lines(tester, short), 1);
      expect(
        lines(tester, long),
        lessThan(long.length ~/ 4),
        reason: 'a long ID wraps by the row, not a letter at a time',
      );
      for (final id in [short, long]) {
        expect(
          exposed(tester, id).top,
          greaterThanOrEqualTo(tester.getRect(find.text(id)).bottom),
          reason: '$id: the chips go under the ID',
        );
      }
    });

    testWidgets('with room the chips stay beside its ID', (tester) async {
      final board = demoBoard();
      board.sessions = [row(short)];
      await openAt(tester, board, '/missions', size: const Size(1400, 1600));
      expect(lines(tester, short), 1);
      final id = tester.getRect(find.text(short));
      final chip = exposed(tester, short);
      expect(chip.left, greaterThanOrEqualTo(id.right));
      expect(chip.top, lessThan(id.bottom));
    });
  });

  testWidgets(
    'a Mission detail lists every attempt with links to its Stations',
    (tester) async {
      final reported = await openAt(
        tester,
        demoBoard(),
        '/missions?operation_id=G&mission_id=mission-007',
        size: const Size(1400, 1600),
      );
      expect(find.text('Attempt 1 · lost'), findsOneWidget);
      expect(find.text('Attempt 2 · active'), findsOneWidget);
      expect(
        find.textContaining('went dark with Theater "local"'),
        findsNWidgets(2),
      );
      expect(
        find.text('Checkpointed to branch mission-007-review'),
        findsNWidgets(2),
      );
      expect(
        find.text('State from · attempt 2 · Session preview-session-007'),
        findsOneWidget,
      );
      expect(find.text('Mission brief'), findsOneWidget);
      expect(button('Timeline'), findsOneWidget);
      expect(find.text('Open Session preview-session-007'), findsOneWidget);
      expect(find.text('Open Session preview-session-008'), findsOneWidget);
      expect(
        find.text('No Session of this Mission is recorded on this Station.'),
        findsOneWidget,
        reason: 'the lost attempt ran elsewhere',
      );
      expect(
        find.widgetWithText(TextButton, 'Station station-remote-02'),
        findsOneWidget,
      );
      // The queue stays beside the detail in a window this wide.
      expect(find.byType(MissionCard), findsOneWidget);

      await tester.tap(
        find.widgetWithText(TextButton, 'Station station-control-01'),
      );
      await tester.pumpAndSettle();
      expect(
        locations(reported).last.$1,
        '/stations?station_id=station-control-01',
      );
      // The Station Watch lists the Mission whose attempt runs there.
      expect(find.text('SESSIONS HERE'), findsOneWidget);
      expect(
        find.text('G › mission-007 · preview-session-007'),
        findsOneWidget,
      );
      expect(find.text('Online'), findsOneWidget);

      await tester.tap(find.text('G › mission-007 · preview-session-007'));
      await tester.pumpAndSettle();
      expect(
        locations(reported).last.$1,
        '/missions?operation_id=G&mission_id=mission-007',
      );
    },
  );

  testWidgets('a Theater link opens the Station theaters tab on that Theater', (
    tester,
  ) async {
    final reported = await openAt(
      tester,
      demoBoard(),
      '/missions?operation_id=G&mission_id=mission-007',
    );
    await tester.tap(find.text('Theater local').last);
    await tester.pumpAndSettle();
    expect(
      locations(reported).last.$1,
      '/stations?station_id=station-control-01&tab=theaters&theater_id=local',
    );
  });

  testWidgets('every link of a Mission detail is reached by the Tab key', (
    tester,
  ) async {
    await openAt(
      tester,
      demoBoard(),
      '/missions?operation_id=G&mission_id=mission-007',
      size: const Size(1400, 1600),
    );
    final links = [
      button('Back to the queue'),
      button('Station station-remote-02'),
      button('Station station-control-01'),
      button('Open Session preview-session-007'),
      button('Open Session preview-session-008'),
      button('Mission brief'),
      button('Timeline'),
    ];
    for (final link in links) {
      expect(link, findsOneWidget, reason: '$link');
    }
    final wanted = {for (final link in links) tester.widget(link)};
    expect(wanted, hasLength(links.length));
    final reached = <Widget>{};
    for (
      var press = 0;
      press < 150 && reached.length < wanted.length;
      press++
    ) {
      await tester.sendKeyEvent(LogicalKeyboardKey.tab);
      await tester.pump();
      if (focusedButton() case final button? when wanted.contains(button)) {
        reached.add(button);
      }
    }
    expect(reached, wanted);
  });

  testWidgets('closing a detail returns focus to the row that opened it', (
    tester,
  ) async {
    final board = demoBoard();
    await openAt(tester, board, '/missions', size: const Size(800, 1000));
    await tester.tap(find.text('Mission details').first);
    await tester.pumpAndSettle();
    expect(find.text('Back to the queue'), findsOneWidget);
    expect(
      find.byType(MissionCard),
      findsNothing,
      reason: 'in place, below 975',
    );

    await tester.tap(find.text('Back to the queue'));
    await tester.pumpAndSettle();
    expect(find.byType(MissionCard), findsOneWidget);
    final focused = focusedButton();
    expect(focused, isNotNull);
    expect(
      find.descendant(
        of: find.byWidget(focused!),
        matching: find.text('Mission details'),
      ),
      findsOneWidget,
    );
  });

  testWidgets('a Station that reports no resources says so, never zero', (
    tester,
  ) async {
    await openAt(tester, demoBoard(), '/stations?station_id=station-remote-02');
    expect(find.text('Dark'), findsOneWidget);
    expect(find.text('Resources: Not reported'), findsOneWidget);
    expect(find.textContaining('Last check-in'), findsOneWidget);
    expect(find.text('No active attempt is on this Station.'), findsOneWidget);
  });

  testWidgets('a Station that reports resources shows them as sampled', (
    tester,
  ) async {
    await openAt(
      tester,
      demoBoard(),
      '/stations?station_id=station-control-01',
    );
    expect(find.textContaining('8 CPUs'), findsOneWidget);
    expect(find.textContaining('sampled'), findsOneWidget);
    expect(find.text('HQS'), findsOneWidget);
    expect(find.text('Running 2 of 4 Sessions'), findsOneWidget);
    expect(find.text('Roles: control, execution'), findsOneWidget);
  });

  testWidgets(
    'an Operation summary counts Missions by state with filtered links',
    (tester) async {
      final reported = await openAt(
        tester,
        demoBoard(),
        '/operations?operation_id=G',
      );
      expect(find.text('Operation G'), findsWidgets);
      expect(find.text('0 of 1 Mission done'), findsOneWidget);
      expect(find.textContaining('Missions: '), findsOneWidget);
      expect(find.byTooltip('mission.read, mission.control'), findsOneWidget);
      expect(find.text('1 held brief'), findsOneWidget);
      expect(find.text('1 Mission proposal'), findsOneWidget);
      expect(
        find.text('Resume applied'),
        findsOneWidget,
        reason: 'recent activity',
      );

      await tester.tap(find.byKey(const ValueKey('count-working')));
      await tester.pumpAndSettle();
      expect(
        locations(reported).last.$1,
        '/missions?operation_id=G&state=working',
      );
    },
  );

  testWidgets(
    'an Operation summary links the whole timeline only with audit.read',
    (tester) async {
      final reported = await openAt(
        tester,
        demoBoard(),
        '/operations?operation_id=G',
      );
      await tester.tap(find.text('View all'));
      await tester.pumpAndSettle();
      expect(locations(reported).last.$1, '/timeline?operation_id=G');
    },
  );

  testWidgets('without audit.read an Operation summary has no activity', (
    tester,
  ) async {
    final board = await queuedBoard((200, '{}'));
    await openAt(tester, board, '/operations?operation_id=operation-alpha');
    expect(find.text('Operation operation-alpha'), findsOneWidget);
    expect(find.text('RECENT ACTIVITY'), findsNothing);
    expect(find.text('View all'), findsNothing);
    expect(find.text('WAITING'), findsNothing, reason: 'no mission.create');
  });

  testWidgets('a queued Mission explains the placement of its Operation', (
    tester,
  ) async {
    final board = await queuedBoard((
      200,
      jsonEncode({
        'operation_id': 'operation-alpha',
        'policy_station': 'station-a',
        'choices': <Object>[],
        'rejections': [
          {
            'station': 'station-b',
            'theater': null,
            'agent': null,
            'reason': 'it is at capacity (2 of 2 Missions)',
          },
        ],
      }),
    ));
    await openAt(
      tester,
      board,
      '/missions?operation_id=operation-alpha&mission_id=mission-007',
    );
    expect(find.text('WHY IT MAY WAIT'), findsOneWidget);
    expect(
      find.text('Placement for Operation operation-alpha now'),
      findsOneWidget,
    );
    expect(
      find.text('station-b: it is at capacity (2 of 2 Missions)'),
      findsOneWidget,
    );
  });

  testWidgets('a placement preview answering 403 hides the explanation', (
    tester,
  ) async {
    final board = await queuedBoard((403, 'forbidden'));
    await openAt(
      tester,
      board,
      '/missions?operation_id=operation-alpha&mission_id=mission-007',
    );
    expect(find.byType(MissionDetail), findsOneWidget);
    expect(find.text('queued'), findsWidgets);
    expect(find.byType(WhyItMayWait), findsOneWidget);
    expect(find.text('WHY IT MAY WAIT'), findsNothing);
    expect(find.textContaining('Placement'), findsNothing);
  });

  testWidgets('a Mission that is not working never asks for placement', (
    tester,
  ) async {
    await openAt(
      tester,
      demoBoard(),
      '/missions?operation_id=G&mission_id=mission-007',
    );
    expect(find.byType(WhyItMayWait), findsNothing);
  });

  testWidgets('Overview names what each Upcoming row is', (tester) async {
    await openAt(tester, demoBoard(), '/');
    expect(find.text('Held brief'), findsOneWidget);
  });
}
