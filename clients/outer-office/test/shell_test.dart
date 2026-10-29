import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart'
    show
        DataTable,
        NavigationDrawer,
        NavigationDrawerDestination,
        NavigationRail,
        NavigationRailLabelType;
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/theme/oo_backdrop.dart';

import 'shell_navigation.dart';

const railPages = [
  'Overview',
  'Operations',
  'Missions',
  'Sessions',
  'Agents',
  'Stations',
  'Green Ink',
  'Briefings',
  'Roster',
  'Account Pools',
  'Timeline',
  'Audit',
];

Future<MissionBoard> demoAt(WidgetTester tester, Size size) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    demo: true,
    pollInterval: null,
  );
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  return board;
}

void main() {
  testWidgets('a wide window has the labelled rail of twelve pages', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    expect(tester.widget<OoRail>(find.byType(OoRail)).extended, isTrue);
    expect(find.byType(NavigationRail), findsNothing);
    expect(tester.getSize(find.byType(OoRail)).width, 226);
    for (final page in railPages) {
      expect(navLabel(page), findsOneWidget, reason: page);
    }
    expect(find.byTooltip('Open navigation menu'), findsNothing);
  });

  testWidgets('a middle-width window has the compact rail', (tester) async {
    await demoAt(tester, const Size(1000, 800));
    final rail = tester.widget<NavigationRail>(find.byType(NavigationRail));
    expect(rail.extended, isFalse);
    expect(rail.labelType, NavigationRailLabelType.all);
    expect(rail.destinations, hasLength(12));
    expect(tester.getSize(find.byType(NavigationRail)).width, lessThan(226));
    await openPage(tester, 'Audit');
    expect(
      find.text('operator-007 · mission.create.requested'),
      findsOneWidget,
    );
  });

  testWidgets('a narrow window puts the rail behind a menu button', (
    tester,
  ) async {
    await demoAt(tester, const Size(800, 1600));
    expect(find.byType(NavigationRail), findsNothing);
    expect(find.byTooltip('Open navigation menu'), findsOneWidget);
    await tester.tap(find.byTooltip('Open navigation menu'));
    await tester.pumpAndSettle();
    expect(
      find.descendant(
        of: find.byType(NavigationDrawer),
        matching: find.byType(NavigationDrawerDestination),
      ),
      findsNWidgets(12),
    );
    await tester.tap(navLabel('Sessions'));
    await tester.pumpAndSettle();
    expect(find.byType(NavigationDrawer), findsNothing);
    expect(find.byType(DataTable), findsOneWidget);
  });

  testWidgets('Overview counts what the board holds and links what needs '
      'attention', (tester) async {
    await demoAt(tester, const Size(1400, 900));
    await openPage(tester, 'Overview');
    expect(find.text('Command overview'), findsOneWidget);
    expect(find.byType(OoBackdrop), findsOneWidget);
    expect(find.text('2 online · 0 dark'), findsNothing);
    expect(find.text('1 online · 1 dark'), findsOneWidget);
    expect(find.text('of 3 Sessions'), findsOneWidget);
    expect(
      find.text('Station station-remote-02 has gone dark'),
      findsOneWidget,
    );
    await tester.tap(find.text('Open Stations'));
    await tester.pumpAndSettle();
    expect(find.text('station-control-01'), findsOneWidget);
  });

  testWidgets('Operations counts each Operation\'s Missions by state', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    await openPage(tester, 'Operations');
    expect(find.text('Operation G'), findsOneWidget);
    expect(find.text('1 Mission'), findsOneWidget);
    expect(find.text('working 1'), findsOneWidget);
  });

  testWidgets('Sessions sort by any column and open without a wallpaper', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    await openPage(tester, 'Sessions');
    expectNoBackdrop(find.byType(DataTable));
    expect(find.byType(OoBackdrop), findsNothing);
    List<String> order() =>
        [
          for (final session in [
            'preview-session-007',
            'preview-session-008',
            'preview-session-021',
          ])
            session,
        ]..sort(
          (a, b) => tester
              .getTopLeft(find.text(a))
              .dy
              .compareTo(tester.getTopLeft(find.text(b)).dy),
        );
    expect(order(), [
      'preview-session-007',
      'preview-session-008',
      'preview-session-021',
    ]);
    await tester.tap(find.text('Event'));
    await tester.pumpAndSettle();
    expect(order(), [
      'preview-session-008',
      'preview-session-007',
      'preview-session-021',
    ]);
    await tester.tap(find.text('Event'));
    await tester.pumpAndSettle();
    expect(order().first, 'preview-session-021');

    await tester.tap(find.text('preview-session-008'));
    await tester.pumpAndSettle();
    expectNoBackdrop(find.byType(SessionWindow));
  });

  testWidgets('Theaters are a tab of Stations', (tester) async {
    await demoAt(tester, const Size(1400, 900));
    await openPage(tester, 'Stations');
    await tester.tap(find.text('Theaters'));
    await tester.pumpAndSettle();
    expect(find.text('Theater build-box'), findsOneWidget);
    expect(find.textContaining('Station station-control-01'), findsWidgets);
  });
}
