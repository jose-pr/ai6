import 'dart:ui' show PointerDeviceKind;

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/oo_empty.dart';
import 'package:outer_office/shell/oo_link.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/shell/overview_page.dart';

import 'moneypenny_off_test.dart' show gateway;
import 'routes_test.dart' show locations;

const wide = Size(1600, 1400);

/// The demo at [location]; what the app reports as its address goes into the returned list.
Future<List<MethodCall>> demoAt(
  WidgetTester tester,
  String location, {
  Size size = wide,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
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
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    demo: true,
    pollInterval: null,
  );
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  return reported;
}

/// Taps the link [text] and returns where the app went.
Future<String> follow(
  WidgetTester tester,
  List<MethodCall> reported,
  String text, {
  int nth = 0,
}) async {
  final link = find.ancestor(
    of: find.text(text),
    matching: find.byType(OoLink),
  );
  expect(link, findsWidgets, reason: 'no link says $text');
  await tester.ensureVisible(link.at(nth));
  await tester.pumpAndSettle();
  await tester.tap(link.at(nth));
  await tester.pumpAndSettle();
  return locations(reported).last.$1;
}

void main() {
  group('a name opens its item', () {
    for (final (from, text, to) in const [
      ('/overview', 'Operation G', '/operations?operation_id=G'),
      (
        '/overview',
        'station-control-01',
        '/stations?station_id=station-control-01',
      ),
      (
        '/overview',
        'station-remote-02',
        '/stations?station_id=station-remote-02',
      ),
      (
        '/overview',
        'Resolve github item org/repo#7',
        '/briefings?tab=held&operation_id=G&item_id=source_github--org_repo_7',
      ),
      ('/operations', 'Operation G', '/operations?operation_id=G'),
      ('/missions', 'Operation G', '/operations?operation_id=G'),
      (
        '/missions',
        'Station station-control-01',
        '/stations?station_id=station-control-01',
      ),
      (
        '/missions',
        'Theater local',
        '/stations?station_id=station-control-01&tab=theaters&theater_id=local',
      ),
      (
        '/missions',
        'Agent technical-officer',
        '/agents?tab=agents&operation_id=G&agent=technical-officer',
      ),
      (
        '/missions',
        'Role case-officer',
        '/agents?tab=roles&operation_id=G&role=case-officer',
      ),
      ('/sessions', 'G', '/operations?operation_id=G'),
      (
        '/sessions',
        'mission-021',
        '/missions?operation_id=L&mission_id=mission-021',
      ),
      (
        '/sessions',
        'station-remote-02',
        '/stations?station_id=station-remote-02',
      ),
      (
        '/stations',
        'station-remote-02',
        '/stations?station_id=station-remote-02',
      ),
      (
        '/timeline',
        'Mission mission-007',
        '/missions?operation_id=G&mission_id=mission-007',
      ),
      (
        '/timeline',
        'Session preview-session-008',
        '/sessions?operation_id=G&session_id=preview-session-008',
      ),
      ('/audit', 'Operation G', '/operations?operation_id=G'),
      (
        '/green_ink',
        'Mission mission-007',
        '/missions?operation_id=G&mission_id=mission-007',
      ),
      (
        '/green_ink',
        'Agent 007',
        '/agents?tab=agents&operation_id=G&agent=007',
      ),
      (
        '/briefings',
        'Parent Mission mission-007',
        '/missions?operation_id=G&mission_id=mission-007',
      ),
      (
        '/briefings?tab=held',
        'Mission mission-007',
        '/missions?operation_id=G&mission_id=mission-007',
      ),
      (
        '/pools',
        'Mission mission-007',
        '/missions?operation_id=G&mission_id=mission-007',
      ),
      ('/agents', '007', '/missions?agent=007'),
    ]) {
      testWidgets('$from: $text', (tester) async {
        final reported = await demoAt(tester, from);
        expect(await follow(tester, reported, text), to);
      });
    }

    testWidgets('a Theater chip opens that Theater', (tester) async {
      final reported = await demoAt(tester, '/stations');
      await tester.tap(find.widgetWithText(ActionChip, 'Theater build-box'));
      await tester.pumpAndSettle();
      expect(
        locations(reported).last.$1,
        '/stations?station_id=station-control-01&tab=theaters&theater_id=build-box',
      );
    });

    testWidgets('an Operation chip on a Station opens that Operation', (
      tester,
    ) async {
      final reported = await demoAt(tester, '/stations');
      await tester.tap(find.widgetWithText(ActionChip, 'Operation G'));
      await tester.pumpAndSettle();
      expect(locations(reported).last.$1, '/operations?operation_id=G');
    });

    testWidgets('a state chip on an Operation opens those Missions', (
      tester,
    ) async {
      final reported = await demoAt(tester, '/operations');
      await tester.tap(find.widgetWithText(ActionChip, 'working 1'));
      await tester.pumpAndSettle();
      expect(
        locations(reported).last.$1,
        '/missions?operation_id=G&state=working',
      );
    });
  });

  group('a tile with a count opens what it counted', () {
    for (final (label, to) in [
      ('Operations', '/operations'),
      ('Stations', '/stations'),
      ('Missions', '/missions?state=${activeMissionStates.join('%2C')}'),
      ('Sessions waiting', '/sessions?state=blocked'),
    ]) {
      testWidgets(label, (tester) async {
        final reported = await demoAt(tester, '/overview');
        final tile = find.descendant(
          of: find.byType(OverviewPage),
          matching: find.text(label),
        );
        await tester.tap(tile.first);
        await tester.pumpAndSettle();
        expect(locations(reported).last.$1.replaceAll(',', '%2C'), to);
      });
    }
  });

  testWidgets('what opened the tile shows only what it counted', (
    tester,
  ) async {
    await demoAt(tester, '/overview');
    await tester.tap(
      find.descendant(
        of: find.byType(OverviewPage),
        matching: find.text('Sessions waiting'),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('preview-session-008'), findsOneWidget);
    expect(find.text('preview-session-007'), findsNothing);
  });

  group('how a link behaves', () {
    Widget host({
      required List<OoPage> pages,
      required ValueChanged<OoRoute> onRoute,
    }) => MaterialApp(
      home: Scaffold(
        body: OoLinks(
          pages: pages,
          onRoute: onRoute,
          child: Center(
            child: OoLink(
              'Station hq',
              route: OoLinkTo.station('hq'),
              semanticLabel: 'Open Station hq',
            ),
          ),
        ),
      ),
    );

    testWidgets(
      'a page the person may not open makes plain text, no dead link',
      (tester) async {
        var opened = 0;
        await tester.pumpWidget(
          host(pages: const [OoPage.overview], onRoute: (_) => opened++),
        );
        expect(find.text('Station hq'), findsOneWidget);
        expect(find.byType(InkWell), findsNothing);
        await tester.tap(find.text('Station hq'));
        expect(opened, 0);
      },
    );

    testWidgets('it has a pointer, a semantics label, and Enter opens it', (
      tester,
    ) async {
      final opened = <OoRoute>[];
      await tester.pumpWidget(
        host(pages: const [OoPage.stations], onRoute: opened.add),
      );
      final semantics = tester.ensureSemantics();
      final node = tester.getSemantics(find.byType(OoLink));
      expect(node.label, 'Open Station hq');
      expect(node.getSemanticsData().flagsCollection.isLink, isTrue);
      semantics.dispose();
      expect(
        tester.widget<InkWell>(find.byType(InkWell)).mouseCursor,
        isNull,
        reason: 'InkWell then gives the clickable hand',
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.tab);
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(opened.single.location, '/stations?station_id=hq');
    });

    testWidgets('it underlines under the pointer', (tester) async {
      await tester.pumpWidget(
        host(pages: const [OoPage.stations], onRoute: (_) {}),
      );
      TextDecoration? decoration() =>
          tester.widget<Text>(find.text('Station hq')).style?.decoration;
      expect(decoration(), isNot(TextDecoration.underline));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      addTearDown(mouse.removePointer);
      await mouse.addPointer(location: Offset.zero);
      await mouse.moveTo(tester.getCenter(find.text('Station hq')));
      await tester.pump();
      expect(decoration(), TextDecoration.underline);
    });

    testWidgets('on a phone it keeps a full tap target', (tester) async {
      tester.view.physicalSize = const Size(390, 844);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        host(pages: const [OoPage.stations], onRoute: (_) {}),
      );
      expect(
        tester.getSize(find.byType(InkWell)).height,
        greaterThanOrEqualTo(OoLink.phoneTarget),
      );
    });

    test('an ID a route cannot take still goes to its page', () {
      expect(OoLinkTo.station('bad\nid').location, '/stations');
      expect(OoLinkTo.mission('G', 'bad\nid').page, OoPage.missions);
    });
  });

  group('refused and failed are not empty', () {
    Future<void> at(
      WidgetTester tester,
      String location,
      int Function(String path) status,
    ) async {
      tester.view.physicalSize = wide;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      tester.binding.platformDispatcher.defaultRouteNameTestValue = location;
      addTearDown(
        tester.binding.platformDispatcher.clearDefaultRouteNameTestValue,
      );
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: gateway({}, status: status),
      );
      addTearDown(board.dispose);
      await tester.pumpWidget(
        OuterOfficeApp(board: board, desktopWindows: false),
      );
      await board.login('ana', 'secret');
      await tester.pumpAndSettle();
    }

    int all(String path, int code) => path == '/v1/user/profile' ? 200 : code;

    testWidgets('Sessions refused', (tester) async {
      await at(tester, '/sessions', (path) => all(path, 403));
      expect(find.text('Your account may not read Sessions.'), findsOneWidget);
      expect(find.text('No Sessions yet'), findsNothing);
    });

    testWidgets('Sessions failed', (tester) async {
      await at(tester, '/sessions', (path) => all(path, 500));
      expect(find.text('Sessions could not be read.'), findsOneWidget);
      expect(find.text('Try again'), findsOneWidget);
      expect(find.text('No Sessions yet'), findsNothing);
    });

    testWidgets('Sessions read and empty', (tester) async {
      await at(tester, '/sessions', (_) => 200);
      expect(find.text('No Sessions yet'), findsOneWidget);
    });

    testWidgets('Missions refused says so, with no connection banner', (
      tester,
    ) async {
      await at(tester, '/missions', (path) => all(path, 403));
      expect(find.text('Your account may not read Sessions.'), findsOneWidget);
      expect(find.textContaining('Could not reach'), findsNothing);
    });

    testWidgets('Agents failed is not "no Agent is posted"', (tester) async {
      await at(tester, '/agents', (path) => all(path, 500));
      expect(find.textContaining('could not give'), findsOneWidget);
      expect(find.textContaining('No Agent is posted'), findsNothing);
      expect(find.text('Try again'), findsOneWidget);
    });

    testWidgets('Pools and Timeline say refused, not empty', (tester) async {
      await at(tester, '/pools', (path) => all(path, 403));
      expect(
        find.textContaining('may not read the account pools'),
        findsOneWidget,
      );
      expect(find.textContaining('No pool account'), findsNothing);
    });

    testWidgets('Timeline failed is not "no activity"', (tester) async {
      await at(tester, '/timeline', (path) => all(path, 500));
      expect(find.textContaining('could not give'), findsOneWidget);
      expect(find.textContaining('No activity recorded'), findsNothing);
    });
  });

  group('an empty page says what it is for, with a way on', () {
    MissionBoard empty({List<String> actions = const ['mission.read']}) {
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
      );
      board.accessToken = 't';
      board.loading = false;
      board.userSession = UserSessionSummary(
        personId: 'ana',
        name: 'ana',
        displayName: null,
        groups: const [],
        expiresAt: 0,
        operations: [UserOperationAccess(operationId: 'G', actions: actions)],
      );
      return board;
    }

    Future<void> open(
      WidgetTester tester,
      MissionBoard board,
      String location,
    ) async {
      tester.view.physicalSize = wide;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      addTearDown(board.dispose);
      tester.binding.platformDispatcher.defaultRouteNameTestValue = location;
      addTearDown(
        tester.binding.platformDispatcher.clearDefaultRouteNameTestValue,
      );
      await tester.pumpWidget(
        OuterOfficeApp(board: board, desktopWindows: false),
      );
      await tester.pumpAndSettle();
    }

    testWidgets('Missions offers the first Mission where the person may', (
      tester,
    ) async {
      await open(
        tester,
        empty(actions: const ['mission.read', 'mission.create']),
        '/missions',
      );
      expect(find.text('No Missions yet'), findsOneWidget);
      expect(find.textContaining('one brief of work'), findsWidgets);
      await tester.tap(find.text('New Mission in G'));
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsOneWidget);
    });

    testWidgets('Missions says who can when the person may not', (
      tester,
    ) async {
      await open(tester, empty(), '/missions');
      expect(find.textContaining('a controller grants'), findsOneWidget);
      expect(find.text('New Mission in G'), findsNothing);
    });

    testWidgets('Sessions links on to Missions', (tester) async {
      await open(tester, empty(), '/sessions');
      expect(find.byType(OoEmptyState), findsOneWidget);
      await tester.tap(find.text('Go to Missions'));
      await tester.pumpAndSettle();
      expect(find.text('No Missions yet'), findsOneWidget);
    });
  });
}
