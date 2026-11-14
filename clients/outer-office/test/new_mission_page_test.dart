import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/new_mission_page.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/shell/operation_names.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'routes_test.dart' show locations;
import 'work_views_test.dart' show openAt;

/// A board whose person holds [actions] in Operation G (called Guatiao); every POST of a
/// Mission is kept in [posts].
MissionBoard boardWith(
  List<String> actions,
  List<http.Request> posts, {
  int status = 202,
  List<Map<String, Object?>> runs = const [],
}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      if (request.method == 'POST') {
        posts.add(request);
        return http.Response(
          jsonEncode({'mission_id': 'm-new'}),
          status,
          headers: {'content-type': 'application/json'},
        );
      }
      if (request.url.path == '/v1/board') {
        return http.Response(
          jsonEncode(runs),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      return http.Response('[]', 404);
    }),
  );
  board.accessToken = 'user-token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'ana',
    name: 'ana',
    displayName: null,
    groups: const [],
    expiresAt: 1_900_000_000,
    operations: [
      UserOperationAccess(operationId: 'G', name: 'Guatiao', actions: actions),
    ],
  );
  return board;
}

const _widths = [('desktop', Size(1400, 1000)), ('phone', Size(390, 1400))];

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    OperationNames.use(const {'G': 'Guatiao'});
  });

  for (final (name, size) in _widths) {
    testWidgets('a person with mission.create writes a brief, queues it and '
        'lands on the Mission, at $name width', (tester) async {
      final posts = <http.Request>[];
      final board = boardWith(['mission.read', 'mission.create'], posts);
      addTearDown(board.dispose);
      final reported = await openAt(
        tester,
        board,
        '/new_mission?operation_id=G',
        size: size,
      );

      expect(find.text('Guatiao'), findsWidgets, reason: 'by its name');
      final queue = find.byKey(const ValueKey('new-mission-queue'));
      expect(
        tester.widget<FilledButton>(queue).onPressed,
        isNull,
        reason: 'a brief needs a title and instructions',
      );
      await tester.enterText(
        find.byKey(const ValueKey('new-mission-title')),
        'Fix the build',
      );
      await tester.enterText(
        find.byKey(const ValueKey('new-mission-prompt')),
        'Make CI green.',
      );
      await tester.pump();
      await tester.ensureVisible(queue);
      await tester.tap(queue);
      await tester.pumpAndSettle();

      expect(posts, hasLength(1), reason: 'one call, one Mission');
      expect(
        posts.single.url.path,
        '/v1/user/operations/G/missions',
        reason: 'the call the app always made for a Mission',
      );
      expect(jsonDecode(posts.single.body), {
        'title': 'Fix the build',
        'prompt': 'Make CI green.',
      });
      expect(
        locations(reported).last.$1,
        '/missions?operation_id=G&mission_id=m-new',
      );
    });

    testWidgets('a refusal is said and keeps what was written, at $name '
        'width', (tester) async {
      final posts = <http.Request>[];
      final board = boardWith(['mission.create'], posts, status: 403);
      addTearDown(board.dispose);
      final reported = await openAt(tester, board, '/new_mission', size: size);
      await tester.enterText(
        find.byKey(const ValueKey('new-mission-title')),
        'Fix',
      );
      await tester.enterText(
        find.byKey(const ValueKey('new-mission-prompt')),
        'Now.',
      );
      await tester.pump();
      final queue = find.byKey(const ValueKey('new-mission-queue'));
      await tester.ensureVisible(queue);
      await tester.tap(queue);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('new-mission-problem')), findsOneWidget);
      expect(find.text('Fix'), findsOneWidget);
      expect(locations(reported).where((l) => l.$1.contains('m-new')), isEmpty);
    });

    testWidgets('without mission.create the page is closed and no New Mission '
        'is offered, at $name width', (tester) async {
      final posts = <http.Request>[];
      final board = boardWith(
        ['mission.read'],
        posts,
        runs: const [
          {
            'operation_id': 'G',
            'mission_id': 'm1',
            'run_id': 'r1',
            'state': 'working',
            'sequence': 1,
            'updated_at': 100.0,
          },
        ],
      );
      addTearDown(board.dispose);
      await openAt(tester, board, '/new_mission?operation_id=G', size: size);
      // A page the account may not open shows Overview, never the form.
      expect(find.byKey(const ValueKey('new-mission-title')), findsNothing);
      expect(find.byKey(const ValueKey('new-mission-queue')), findsNothing);

      await openAt(tester, board, '/missions', size: size);
      expect(find.text('New Mission'), findsNothing);
      expect(find.byKey(const ValueKey('missions-new')), findsNothing);
    });

    testWidgets(
      'the Missions page has a New Mission action that opens the page '
      'on its Operation, at $name width',
      (tester) async {
        final board = boardWith(
          ['mission.read', 'mission.create'],
          [],
          runs: const [
            {
              'operation_id': 'G',
              'mission_id': 'm1',
              'run_id': 'r1',
              'state': 'working',
              'sequence': 1,
              'updated_at': 100.0,
            },
          ],
        );
        addTearDown(board.dispose);
        await board.refresh();
        final reported = await openAt(tester, board, '/missions', size: size);
        final action = find.byKey(const ValueKey('missions-new'));
        expect(action, findsOneWidget);
        await tester.tap(action);
        await tester.pumpAndSettle();
        expect(locations(reported).last.$1, '/new_mission?operation_id=G');
        expect(find.byKey(const ValueKey('new-mission-queue')), findsOneWidget);
      },
    );

    testWidgets('Draft with Moneypenny is the second action where she is '
        'served, at $name width', (tester) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final board = boardWith(['mission.create'], []);
      addTearDown(board.dispose);
      final drafted = <String>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: NewMissionPage(
              board: board,
              route: const OoRoute(OoPage.newMission),
              onRoute: (_) {},
              onDraft: drafted.add,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Queue Mission'), findsOneWidget);
      final draft = find.byKey(const ValueKey('new-mission-draft'));
      await tester.ensureVisible(draft);
      await tester.tap(draft);
      expect(drafted, ['G']);
    });
  }
}
