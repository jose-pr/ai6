import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/settings/oo_config.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/shell/top_bar.dart';

import 'shell_test.dart' show demoAt;

const wide = Size(1440, 900);
const compact = Size(1024, 768);
const phone = Size(390, 844);

/// The demo at [location], at [size].
Future<void> demoOpenAt(WidgetTester tester, Size size, String location) async {
  tester.binding.platformDispatcher.defaultRouteNameTestValue = location;
  addTearDown(tester.binding.platformDispatcher.clearDefaultRouteNameTestValue);
  await demoAt(tester, size);
}

/// The vertical position of each of [labels] in the rail.
List<double> tops(WidgetTester tester, List<String> labels) => [
  for (final label in labels)
    tester
        .getTopLeft(
          find.descendant(of: find.byType(OoRail), matching: find.text(label)),
        )
        .dy,
];

void main() {
  group('the groups', () {
    test('every page but her memory is in one group', () {
      final grouped = [for (final group in OoRailGroup.values) ...group.pages];
      expect(grouped.toSet(), hasLength(grouped.length));
      expect(
        grouped.toSet(),
        OoPage.values.toSet()
          ..remove(OoPage.memory)
          ..remove(OoPage.pages),
      );
    });

    test('a group with nothing the person may see is left out', () {
      final groups = OoRailGroup.of(const [
        OoPage.overview,
        OoPage.stations,
        OoPage.audit,
      ]);
      expect(groups.map((group) => group.$1), [
        OoRailGroup.work,
        OoRailGroup.machines,
        OoRailGroup.record,
      ]);
      expect(groups.last.$2, [OoPage.audit]);
    });

    testWidgets('at 1440 the labels stand above their pages, in order', (
      tester,
    ) async {
      await demoAt(tester, wide);
      final ordered = tops(tester, [
        'WORK',
        'Overview',
        'Missions',
        'Sessions',
        'Green Ink',
        'Briefings',
        'PEOPLE AND POLICY',
        'Operations',
        'Agents',
        'Roster',
        'Account Pools',
        'MACHINES',
        'Stations',
        'RECORD',
        'Timeline',
        'Audit',
      ]);
      expect(ordered, [...ordered]..sort());
    });

    testWidgets('a person with no record pages sees no Record label', (
      tester,
    ) async {
      tester.view.physicalSize = wide;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
      );
      addTearDown(board.dispose);
      board.accessToken = 't';
      board.loading = false;
      board.userSession = const UserSessionSummary(
        personId: 'ana',
        name: 'ana',
        displayName: null,
        groups: [],
        expiresAt: 0,
        operations: [
          UserOperationAccess(operationId: 'G', actions: ['mission.read']),
        ],
      );
      await tester.pumpWidget(
        OuterOfficeApp(board: board, desktopWindows: false),
      );
      await tester.pumpAndSettle();
      expect(find.text('RECORD'), findsNothing);
      expect(find.text('WORK'), findsOneWidget);
      expect(find.text('MACHINES'), findsOneWidget);
    });
  });

  group('per width', () {
    testWidgets('1024 is icons, each label on hover and to a screen reader', (
      tester,
    ) async {
      await demoAt(tester, compact);
      expect(tester.getSize(find.byType(OoRail)).width, OoRail.compactWidth);
      expect(find.text('WORK'), findsNothing);
      for (final page in OoRailGroup.values.expand((group) => group.pages)) {
        if (page == OoPage.access || page == OoPage.compartments) continue;
        expect(
          find.descendant(
            of: find.byType(OoRail),
            matching: find.byWidgetPredicate(
              (widget) => widget is Tooltip && widget.message == page.label,
            ),
          ),
          findsOneWidget,
          reason: page.label,
        );
      }
      final semantics = tester.ensureSemantics();
      expect(
        tester.getSemantics(find.byKey(const ValueKey('rail-missions'))),
        matchesSemantics(
          label: 'Missions',
          isButton: true,
          hasTapAction: true,
          hasFocusAction: true,
          isFocusable: true,
          hasSelectedState: true,
        ),
      );
      semantics.dispose();
    });

    testWidgets('a phone has a bottom bar and More opens the rest', (
      tester,
    ) async {
      await demoAt(tester, phone);
      final bar = find.byType(NavigationBar);
      for (final label in [
        'Overview',
        'Missions',
        'Sessions',
        'Green Ink',
        'More',
      ]) {
        expect(
          find.descendant(of: bar, matching: find.text(label)),
          findsOneWidget,
          reason: label,
        );
      }
      expect(find.byTooltip('Open navigation menu'), findsNothing);
      await tester.tap(find.descendant(of: bar, matching: find.text('More')));
      await tester.pumpAndSettle();
      final drawer = find.byType(NavigationDrawer);
      expect(
        find.descendant(of: drawer, matching: find.text('PEOPLE AND POLICY')),
        findsOneWidget,
      );
      final audit = find.descendant(of: drawer, matching: find.text('Audit'));
      await tester.scrollUntilVisible(
        audit,
        200,
        scrollable: find.descendant(
          of: drawer,
          matching: find.byType(Scrollable),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(audit);
      await tester.pumpAndSettle();
      expect(find.byType(NavigationDrawer), findsNothing);
      expect(
        tester.widget<NavigationBar>(bar).selectedIndex,
        4,
        reason: 'a page behind More selects More',
      );
      await tester.tap(
        find.descendant(of: bar, matching: find.text('Missions')),
      );
      await tester.pumpAndSettle();
      expect(tester.widget<NavigationBar>(bar).selectedIndex, 1);
    });
  });

  group('a deep link lands on its entry', () {
    for (final size in [wide, compact]) {
      testWidgets('at ${size.width}', (tester) async {
        await demoOpenAt(tester, size, '/audit');
        final rail = tester.widget<OoRail>(find.byType(OoRail));
        expect(rail.pages[rail.selected], OoPage.audit);
      });
    }

    testWidgets('on a phone in the bar', (tester) async {
      await demoOpenAt(tester, phone, '/sessions');
      expect(
        tester.widget<NavigationBar>(find.byType(NavigationBar)).selectedIndex,
        2,
      );
    });

    testWidgets('in the drawer at a tablet width', (tester) async {
      await demoOpenAt(tester, const Size(800, 1000), '/stations');
      await tester.tap(find.byTooltip('Open navigation menu'));
      await tester.pumpAndSettle();
      final drawer = tester.widget<NavigationDrawer>(
        find.byType(NavigationDrawer),
      );
      // Work holds five, People and policy four in the demo; Stations is next.
      expect(drawer.selectedIndex, 9);
    });
  });

  group('the keyboard', () {
    for (final size in [wide, compact]) {
      testWidgets('reaches every entry at ${size.width}, and Enter opens it', (
        tester,
      ) async {
        await demoAt(tester, size);
        final wanted = {
          for (final page in tester.widget<OoRail>(find.byType(OoRail)).pages)
            'rail-${page.id}',
        };
        final reached = <String>{};
        for (var i = 0; i < 80 && !reached.containsAll(wanted); i++) {
          await tester.sendKeyEvent(LogicalKeyboardKey.tab);
          await tester.pump();
          final context = FocusManager.instance.primaryFocus?.context;
          context?.visitAncestorElements((element) {
            final key = element.widget.key;
            if (key is ValueKey<String> && key.value.startsWith('rail-')) {
              reached.add(key.value);
              return false;
            }
            return true;
          });
          // Selection is a click through the same entry; the last one reached is opened below.
        }
        expect(reached, containsAll(wanted));
        // Walk back to Audit and open it.
        for (var i = 0; i < 80; i++) {
          var onAudit = false;
          FocusManager.instance.primaryFocus?.context?.visitAncestorElements((
            element,
          ) {
            if (element.widget.key == const ValueKey('rail-audit')) {
              onAudit = true;
              return false;
            }
            return true;
          });
          if (onAudit) break;
          await tester.sendKeyEvent(LogicalKeyboardKey.tab);
          await tester.pump();
        }
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await tester.pumpAndSettle();
        final rail = tester.widget<OoRail>(find.byType(OoRail));
        expect(rail.pages[rail.selected], OoPage.audit);
      });
    }
  });

  group('the top bar', () {
    for (final size in [wide, compact, phone]) {
      testWidgets('every icon says what it is at ${size.width}', (
        tester,
      ) async {
        await demoAt(tester, size);
        final bar = find.byType(AppBar);
        final buttons = tester.widgetList<IconButton>(
          find.descendant(of: bar, matching: find.byType(IconButton)),
        );
        expect(buttons, isNotEmpty);
        for (final button in buttons) {
          expect(button.tooltip, isNotEmpty, reason: '${button.icon}');
        }
      });
    }

    testWidgets('the bell shows a badge only with something to count', (
      tester,
    ) async {
      await demoAt(tester, wide);
      final badge = tester.widget<Badge>(
        find.descendant(
          of: find.byType(AttentionBell),
          matching: find.byType(Badge),
        ),
      );
      expect(badge.isLabelVisible, isTrue);

      tester.view.physicalSize = wide;
      final empty = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
      );
      addTearDown(empty.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AttentionBell(board: empty, pages: const [], onOpen: (_) {}),
          ),
        ),
      );
      expect(tester.widget<Badge>(find.byType(Badge)).isLabelVisible, isFalse);
    });

    testWidgets('search says what it covers and when nothing matches', (
      tester,
    ) async {
      await demoAt(tester, wide);
      expect(find.text(OoSearch.hint), findsOneWidget);
      await tester.tap(find.text(OoSearch.hint));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField).last, 'zzz-nothing');
      await tester.pumpAndSettle();
      expect(find.text('No matches in the loaded board'), findsOneWidget);
      await tester.enterText(find.byType(TextField).last, 'mission-021');
      await tester.pumpAndSettle();
      expect(find.text('Results in the loaded board'), findsOneWidget);
      await tester.tap(find.text('Mission mission-021'));
      await tester.pumpAndSettle();
      // It opens the Mission, not just its page.
      expect(find.text('L › mission-021'), findsOneWidget);
    });

    testWidgets('search lists as many results as ai6.cfg says, and says so', (
      tester,
    ) async {
      tester.view.physicalSize = wide;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        demo: true,
        pollInterval: null,
        limits: const OoLimits(pages: PageSettings(search: 2)),
      );
      addTearDown(board.dispose);
      await tester.pumpWidget(
        OuterOfficeApp(board: board, desktopWindows: false),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text(OoSearch.hint));
      await tester.pumpAndSettle();
      expect(find.textContaining('First 2 of '), findsOneWidget);
      expect(find.text('Mission mission-021'), findsNothing);
    });
  });
}
