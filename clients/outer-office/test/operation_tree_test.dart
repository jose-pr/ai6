import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart'
    show Card, DropdownButton, DropdownMenuItem, Key, MaterialApp, Scaffold;
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/missions_board.dart';
import 'package:outer_office/operations/operation_tree.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;

import 'shell_navigation.dart';

UserOperationAccess access(String id, [String? parent]) => UserOperationAccess(
  operationId: id,
  actions: const ['mission.read'],
  parent: parent,
);

MissionBoard boardOf(
  List<UserOperationAccess> operations, {
  List<String> roster = const [],
  http.Response Function(http.Request)? respond,
}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      if (respond != null && request.url.path.startsWith('/v1/user/roster/')) {
        return respond(request);
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
    operations: operations,
    roster: roster,
  );
  return board;
}

Future<void> open(WidgetTester tester, MissionBoard board, String page) async {
  tester.view.physicalSize = const Size(800, 1400);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await openPage(tester, page);
  await tester.pumpAndSettle();
}

/// Where the card of the Operation `text` names starts.
double left(WidgetTester tester, String text) => tester
    .getTopLeft(find.ancestor(of: find.text(text), matching: find.byType(Card)))
    .dx;

void main() {
  test('a hidden ancestor lifts its child to the top', () {
    final tree = operationTree([access('G', 'P'), access('H', 'ROOT')]);
    expect([for (final node in tree) node.operationId], ['G', 'H']);
    final nested = operationTree([
      access('G', 'P'),
      access('P', 'ROOT'),
      access('ROOT'),
    ]);
    expect(nested.single.operationId, 'ROOT');
    expect(nested.single.subtree.toList(), ['ROOT', 'P', 'G']);
  });

  test('the parent choices leave out the Operation and its own subtree', () {
    final parents = {'ROOT': null, 'P': 'ROOT', 'G': 'P', 'OTHER': null};
    expect(parentChoices(parents, 'P'), ['OTHER', 'ROOT']);
    expect(parentChoices(parents, 'OTHER'), ['G', 'P', 'ROOT']);
  });

  testWidgets('a three-level tree renders nested and folds', (tester) async {
    final board = boardOf([
      access('G', 'P'),
      access('P', 'ROOT'),
      access('ROOT'),
    ]);
    await open(tester, board, 'Operations');

    expect(
      left(tester, 'Operation P'),
      greaterThan(left(tester, 'Operation ROOT')),
    );
    expect(
      left(tester, 'Operation G'),
      greaterThan(left(tester, 'Operation P')),
    );
    expect(find.textContaining('1 subordinate Operation'), findsNWidgets(2));

    await tester.tap(find.byTooltip("Hide P's subordinates"));
    await tester.pumpAndSettle();
    expect(find.text('Operation G'), findsNothing);
    expect(find.text('Operation P'), findsOneWidget);
    await tester.tap(find.byTooltip("Hide ROOT's subordinates"));
    await tester.pumpAndSettle();
    expect(find.text('Operation P'), findsNothing);
  });

  testWidgets('a hidden ancestor lifts its child to the top of the board', (
    tester,
  ) async {
    final board = boardOf([access('G', 'P'), access('H')]);
    await open(tester, board, 'Operations');
    expect(left(tester, 'Operation G'), left(tester, 'Operation H'));
  });

  testWidgets('the picker omits the Operation and its subtree', (tester) async {
    final board = boardOf(
      [access('ROOT'), access('P', 'ROOT'), access('G', 'P'), access('OTHER')],
      roster: ['roster.read', 'roster.manage'],
      respond: (request) => http.Response(
        jsonEncode({
          'kind': 'operations',
          'revision': '3',
          'writable': true,
          'records': [
            {
              'key': 'P',
              'definition': {'parent': 'ROOT'},
            },
          ],
        }),
        200,
      ),
    );
    await open(tester, board, 'Roster');
    await tester.tap(find.byType(DropdownButton<String>).first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Operations').last);
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Edit P'));
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const Key('roster-parent')));
    await tester.pumpAndSettle();
    final offered = [
      for (final item in tester.widgetList<DropdownMenuItem<String?>>(
        find.byType(DropdownMenuItem<String?>),
      ))
        if (item.value != null) item.value,
    ];
    expect(offered.toSet(), {'ROOT', 'OTHER'});
    expect(offered, isNot(contains('G')));
    expect(offered, isNot(contains('P')));
  });

  testWidgets('the effective view names the Operation that wrote a key', (
    tester,
  ) async {
    final effective = EffectiveOperationSummary.fromJson({
      'operation_id': 'G',
      'lineage': ['P', 'ROOT'],
      'inherit': true,
      'definition': {
        'max_concurrent': 2,
        'green_ink': {'kinds': 'sign'},
      },
      'origins': {'max_concurrent': 'ROOT', 'green_ink.kinds': 'G'},
      'revision': '4',
    });
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: EffectivePolicyView(effective: effective)),
      ),
    );
    expect(find.text('Under P › ROOT'), findsOneWidget);
    expect(find.text('max_concurrent = 2'), findsOneWidget);
    expect(find.text('from ROOT'), findsOneWidget);
    expect(find.text('green_ink.kinds = "sign"'), findsOneWidget);
    expect(
      find.text('from G'),
      findsNothing,
      reason: 'its own key is unmarked',
    );
  });

  test('the Missions of a subtree route include the grandchild', () {
    final board = boardOf([
      access('G', 'P'),
      access('P', 'ROOT'),
      access('ROOT'),
    ]);
    board.sessions = [
      for (final operation in ['G', 'P', 'ROOT', 'OTHER'])
        SessionSummary(
          operationId: operation,
          missionId: 'm-$operation',
          sessionId: 's-$operation',
          state: 'working',
          sequence: 1,
          stationId: 'station-a',
          agent: '007',
        ),
    ];
    final route = OoRoute(OoPage.missions)
        .withField('operation_id', 'P')
        .withField('subtree', 'true');
    expect(
      {for (final mission in routeMissions(board, route)) mission.operationId},
      {'P', 'G'},
    );
  });
}
