import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart'
    show NavigationDrawer, NavigationDrawerDestination;
import 'package:outer_office/main.dart';
import 'package:outer_office/widgets/oo_data_table.dart';
import 'package:outer_office/shell/missions_board.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/theme/oo_backdrop.dart';
import 'package:outer_office/theme/oo_decor.dart';

import 'shell_navigation.dart';

const railPages = [
  'Overview',
  'Operations',
  'Missions',
  'Runs',
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
  testWidgets('a fresh start opens Overview, which lists held briefs', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    expect(find.text('Command overview'), findsOneWidget);
    expect(
      find.text(
        'Brief "Resolve github item org/repo#7" is held for review in '
        'Operation G',
      ),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Open Briefings').first);
    await tester.tap(find.text('Open Briefings').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Held briefs'));
    await tester.pumpAndSettle();
    expect(find.text('Resolve github item org/repo#7'), findsOneWidget);
  });

  testWidgets('a wide window has the labelled rail of twelve pages', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    expect(tester.widget<OoRail>(find.byType(OoRail)).extended, isTrue);
    expect(tester.getSize(find.byType(OoRail)).width, 226);
    for (final page in railPages) {
      expect(navLabel(page), findsOneWidget, reason: page);
    }
    expect(find.byTooltip('Open navigation menu'), findsNothing);
  });

  testWidgets('a middle-width window has the icon rail, labels on hover', (
    tester,
  ) async {
    await demoAt(tester, const Size(1000, 800));
    final rail = tester.widget<OoRail>(find.byType(OoRail));
    expect(rail.extended, isFalse);
    expect(rail.pages, hasLength(12));
    expect(tester.getSize(find.byType(OoRail)).width, OoRail.compactWidth);
    for (final page in railPages) {
      expect(navLabel(page), findsOneWidget, reason: page);
    }
    await openPage(tester, 'Audit');
    expect(find.text('operator-007'), findsOneWidget);
    expect(find.text('Mission create requested'), findsOneWidget);
  });

  testWidgets('a narrow window puts the rail behind a menu button', (
    tester,
  ) async {
    await demoAt(tester, const Size(800, 1600));
    expect(find.byType(OoRail), findsNothing);
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
    await tester.tap(navLabel('Runs'));
    await tester.pumpAndSettle();
    expect(find.byType(NavigationDrawer), findsNothing);
    expect(find.byType(OoDataTable<RunSummary>), findsOneWidget);
  });

  testWidgets('Overview counts what the board holds and links what needs '
      'attention', (tester) async {
    await demoAt(tester, const Size(1400, 900));
    await openPage(tester, 'Overview');
    expect(find.text('Command overview'), findsOneWidget);
    // The theme's hero art heads Overview; the wallpaper is only for empty pages.
    expect(find.byType(OoHero), findsOneWidget);
    expect(find.byType(OoBackdrop), findsNothing);
    expect(find.text('2 online · 0 dark'), findsNothing);
    expect(find.text('1 online · 1 dark'), findsOneWidget);
    expect(find.text('of 3 Runs'), findsOneWidget);
    expect(
      find.text('Station station-remote-02 has gone dark'),
      findsOneWidget,
    );
    // The past refusal is listed too, muted and labelled, with its own Station link.
    expect(find.text('Historical, no longer current'), findsOneWidget);
    expect(find.text('PAST CONDITIONS'), findsOneWidget);
    await tester.ensureVisible(find.text('Open Stations').first);
    await tester.tap(find.text('Open Stations').first);
    await tester.pumpAndSettle();
    expect(find.text('station-remote-02'), findsOneWidget);
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

  testWidgets('Runs sort by state or update time and open without a '
      'wallpaper', (tester) async {
    final board = await demoAt(tester, const Size(1400, 900));
    board.runs = [
      for (final (run, at) in [
        (board.runs[0], 300.0),
        (board.runs[1], 100.0),
        (board.runs[2], 200.0),
      ])
        RunSummary(
          operationId: run.operationId,
          missionId: run.missionId,
          runId: run.runId,
          state: run.state,
          sequence: run.sequence,
          updatedAt: at,
          stationId: run.stationId,
          agent: run.agent,
        ),
    ];
    await openPage(tester, 'Runs');
    expectNoBackdrop(find.byType(OoDataTable<RunSummary>));
    expect(find.byType(OoBackdrop), findsNothing);
    List<String> order() =>
        [
          for (final run in [
            'preview-run-007',
            'preview-run-008',
            'preview-run-021',
          ])
            run,
        ]..sort(
          (a, b) => tester
              .getTopLeft(find.text(a))
              .dy
              .compareTo(tester.getTopLeft(find.text(b)).dy),
        );
    expect(order(), ['preview-run-007', 'preview-run-008', 'preview-run-021']);
    await tester.tap(find.text('Updated'));
    await tester.pumpAndSettle();
    expect(order(), ['preview-run-008', 'preview-run-021', 'preview-run-007']);
    expect(find.text('Sorted by: updated'), findsOneWidget);
    await tester.tap(find.text('Updated'));
    await tester.pumpAndSettle();
    expect(order().first, 'preview-run-007');
    await tester.tap(find.text('State'));
    await tester.pumpAndSettle();
    expect(order(), ['preview-run-007', 'preview-run-008', 'preview-run-021']);
    // Every column sorts through the route.
    await tester.tap(find.text('Run'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Run'));
    await tester.pumpAndSettle();
    expect(find.text('Sorted by: run'), findsOneWidget);
    expect(order(), ['preview-run-021', 'preview-run-008', 'preview-run-007']);
    await tester.tap(find.text('Operation'));
    await tester.pumpAndSettle();
    expect(order().last, 'preview-run-021', reason: 'Operation L after G');

    await tester.tap(find.text('preview-run-008'));
    await tester.pumpAndSettle();
    expectNoBackdrop(find.byType(RunWindow));
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
