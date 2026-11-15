import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/chat_region.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/shell/top_bar.dart';
import 'package:outer_office/theme/oo_decor.dart';
import 'package:outer_office/theme/oo_icon.dart';
import 'package:outer_office/theme/oo_theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'shell_test.dart' show demoAt;

/// A Moneypenny socket that keeps what the app sends and answers what the test says.
class FakeChannel implements AssistantChannel {
  final sent = <Map<String, Object?>>[];
  final _incoming = StreamController<String>();

  void answer(Map<String, Object?> frame) => _incoming.add(jsonEncode(frame));

  @override
  Stream<String> get incoming => _incoming.stream;

  @override
  void send(String frame) =>
      sent.add((jsonDecode(frame) as Map).cast<String, Object?>());

  @override
  Future<void> close() => _incoming.close();
}

/// A signed-in board whose person may only read Missions, talking to [channel].
MissionBoard signedIn(FakeChannel channel) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    assistantChannel: () => channel,
    client: MockClient((request) async => http.Response('[]', 404)),
  );
  board.accessToken = 'user-token';
  board.loading = false;
  board.userSession = const UserSessionSummary(
    personId: 'ana',
    moneypenny: true,
    name: 'ana',
    displayName: 'Ana Ortiz',
    groups: ['operators'],
    expiresAt: 1_900_000_000,
    operations: [
      UserOperationAccess(operationId: 'G', actions: ['mission.read']),
    ],
  );
  return board;
}

Future<void> atSize(WidgetTester tester, Size size) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
}

void main() {
  // The chat asks the device for its last conversation before it says hello.
  setUp(() => SharedPreferences.setMockInitialValues({}));

  testWidgets('a phone shows one page, the menu as icon tiles and tables as '
      'rows', (tester) async {
    await demoAt(tester, const Size(390, 844));
    expect(find.byType(OoRail), findsNothing);
    expect(find.byType(NavigationRail), findsNothing);
    // The top bar keeps the page title, search, Moneypenny, the bell and the user chip.
    expect(
      find.descendant(of: find.byType(AppBar), matching: find.text('Overview')),
      findsOneWidget,
    );
    for (final tooltip in [
      OoSearch.tooltip,
      'Needs attention: 4',
      'Your account and settings',
    ]) {
      expect(find.byTooltip(tooltip), findsOneWidget, reason: tooltip);
    }
    // Every control there is a 48 px touch target.
    final semantics = tester.ensureSemantics();
    await expectLater(tester, meetsGuideline(androidTapTargetGuideline));
    semantics.dispose();
    // Overview is one column: every stat tile starts at the same left edge.
    final tiles = find.descendant(
      of: find.byType(Card),
      matching: find.byType(OoTile),
    );
    expect(tiles, findsNWidgets(4));
    final lefts = {
      for (final element in tiles.evaluate())
        tester.getTopLeft(find.byWidget(element.widget)).dx,
    };
    expect(lefts, hasLength(1));

    await tester.tap(
      find.descendant(
        of: find.byType(NavigationBar),
        matching: find.text('More'),
      ),
    );
    await tester.pumpAndSettle();
    final drawer = find.byType(NavigationDrawer);
    expect(
      tester
          .widget<NavigationDrawer>(drawer)
          .children
          .whereType<NavigationDrawerDestination>()
          .where((entry) => entry.icon is OoTile),
      hasLength(14),
    );
    await tester.tap(find.descendant(of: drawer, matching: find.text('Runs')));
    await tester.pumpAndSettle();
    expect(find.byType(DataTable), findsNothing);
    expect(find.text('preview-run-007'), findsOneWidget);
    expect(find.text('Operation G'), findsNWidgets(2));
    expect(find.text('Mission mission-007'), findsNWidgets(2));
  });

  testWidgets('the bell counts what needs attention and opens its page', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    final bell = find.byTooltip('Needs attention: 4');
    expect(find.descendant(of: bell, matching: find.text('4')), findsOneWidget);
    await tester.tap(bell);
    await tester.pumpAndSettle();
    final item = find.descendant(
      of: find.byType(MenuItemButton),
      matching: find.text('Station station-remote-02 has gone dark'),
    );
    expect(item, findsOneWidget);
    await tester.tap(item);
    await tester.pumpAndSettle();
    expect(find.text('station-remote-02'), findsOneWidget);
  });

  testWidgets('search finds what the board holds and opens its page', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    await tester.tap(find.text(OoSearch.hint));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).last, 'remote');
    await tester.pumpAndSettle();
    expect(find.text('Run preview-run-007'), findsNothing);
    await tester.tap(find.text('Station station-remote-02'));
    await tester.pumpAndSettle();
    // It opens that Station, not just the page of Stations.
    expect(find.text('station-remote-02'), findsWidgets);
    expect(find.text('station-control-01'), findsNothing);
  });

  testWidgets('the user chip shows initials, name and directory group', (
    tester,
  ) async {
    await atSize(tester, const Size(1400, 900));
    final board = signedIn(FakeChannel());
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.pumpAndSettle();
    final chip = find.byType(UserChip);
    expect(
      find.descendant(of: chip, matching: find.text('AO')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: chip, matching: find.text('Ana Ortiz')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: chip, matching: find.text('operators')),
      findsOneWidget,
    );
    expect(ooInitials('Operator 007'), 'O');
    expect(ooInitials('Ana Maria Ortiz'), 'AO');
    expect(ooInitials('Ana Ortiz'), 'AO');
    expect(ooInitials('Ana'), 'A');
    // A middle initial says where the surname starts, however many words it has.
    expect(ooInitials('Ana M. Ortiz Vega'), 'AO');
    expect(ooInitials('Ana M Ortiz Vega'), 'AO');
    expect(ooInitials('A. M. Ortiz'), 'AO');
    expect(ooInitials('Ana M.'), 'AM');
  });

  testWidgets('on a phone Moneypenny opens full screen and a quick question '
      'asks her', (tester) async {
    await atSize(tester, const Size(390, 844));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('Ask Moneypenny (Ctrl+Alt+I)'));
    await tester.pumpAndSettle();
    expect(tester.getSize(find.byType(ChatPanel)).width, 390);
    expect(find.byType(AppBar), findsNothing, reason: 'one view at a time');
    expect(channel.sent.single['type'], 'hello');
    channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
    await tester.pumpAndSettle();
    // An empty chat offers the quick questions as chips.
    final chip = find.descendant(
      of: find.byType(ChatPanel),
      matching: find.widgetWithText(ActionChip, 'What needs my attention?'),
    );
    expect(chip, findsOneWidget);
    await tester.tap(chip);
    await tester.pumpAndSettle();
    expect(channel.sent.last, {
      'type': 'message',
      'text': 'What needs my attention?',
      'route': {'page': 'overview'},
    });
    await tester.tap(find.byTooltip('Close Moneypenny (Ctrl+Alt+I)'));
    await tester.pumpAndSettle();
    expect(find.byType(ChatPanel), findsNothing);
  });

  for (final (name, size, columns, tallest) in [
    ('desktop', const Size(1280, 800), 3, 130.0),
    ('phone', const Size(390, 844), 1, 140.0),
  ]) {
    testWidgets('the Overview at $name width has no Moneypenny card, a short '
        'banner and the Operations panel in the freed space', (tester) async {
      await atSize(tester, size);
      final board = signedIn(FakeChannel());
      addTearDown(board.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: MissionBoardPage(
            board: board,
            desktopWindows: false,
            route: const OoRoute(OoPage.overview),
            onRoute: (_) {},
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('MONEYPENNY'), findsNothing);
      expect(
        find.widgetWithText(ActionChip, 'Summarise the board'),
        findsNothing,
      );
      expect(find.byType(TextField), findsNothing);

      final hero = tester.getRect(find.byType(OoHero));
      expect(hero.height, lessThanOrEqualTo(tallest), reason: '$name banner');
      expect(hero.width, greaterThan(size.width * 0.7), reason: 'full width');

      final operations = tester.getRect(find.text('OPERATIONS'));
      final activity = tester.getRect(find.text('RECENT ACTIVITY'));
      expect(operations.top, greaterThan(hero.bottom));
      if (columns == 3) {
        expect(operations.left, greaterThan(activity.left));
        expect(
          operations.top,
          closeTo(activity.top, 6),
          reason: 'third column',
        );
      } else {
        expect(operations.top, lessThan(activity.top));
      }
      expect(tester.takeException(), isNull);
    });
  }

  test('a question asked before the conversation is ready waits for it', () {
    final channel = FakeChannel();
    final controller = AssistantController(
      token: 'user-token',
      connect: () => channel,
    );
    addTearDown(controller.dispose);
    controller.ask(
      'Summarise the board',
      route: const OoRoute(OoPage.overview),
    );
    expect(channel.sent.single, {'type': 'hello', 'token': 'user-token'});
    channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
    return pumpEventQueue().then((_) {
      expect(channel.sent.last, {
        'type': 'message',
        'text': 'Summarise the board',
        'route': {'page': 'overview'},
      });
      expect(channel.sent, hasLength(2));
    });
  });

  testWidgets('duotone icons take the theme\'s colours in every theme', (
    tester,
  ) async {
    for (final theme in OoThemeId.values) {
      await tester.pumpWidget(
        MaterialApp(
          theme: ooTheme(theme),
          home: const Scaffold(
            body: Row(
              children: [
                OoDuotone(OoIconName.stations, size: 40, semanticLabel: 'st'),
                OoTile(OoIconName.missions),
              ],
            ),
          ),
        ),
      );
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 50)),
      );
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull, reason: theme.label);
      expect(tester.getSize(find.byType(OoDuotone).first), const Size(40, 40));
    }
  });
}
