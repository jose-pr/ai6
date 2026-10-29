import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/chat_region.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/shell/top_bar.dart';
import 'package:outer_office/theme/oo_decor.dart';
import 'package:outer_office/theme/oo_icon.dart';
import 'package:outer_office/theme/oo_theme.dart';

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
      'Open navigation menu',
      'Search',
      'Notifications: 3 need your attention',
      'Operator account and preferences',
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

    await tester.tap(find.byTooltip('Open navigation menu'));
    await tester.pumpAndSettle();
    final drawer = find.byType(NavigationDrawer);
    expect(
      find.descendant(of: drawer, matching: find.byType(OoTile)),
      findsNWidgets(12),
    );
    await tester.tap(
      find.descendant(of: drawer, matching: find.text('Sessions')),
    );
    await tester.pumpAndSettle();
    expect(find.byType(DataTable), findsNothing);
    expect(find.text('preview-session-007'), findsOneWidget);
    expect(find.text('Operation G · Mission mission-007'), findsNWidgets(2));
  });

  testWidgets('the bell counts what needs attention and opens its page', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    final bell = find.byTooltip('Notifications: 3 need your attention');
    expect(find.descendant(of: bell, matching: find.text('3')), findsOneWidget);
    await tester.tap(bell);
    await tester.pumpAndSettle();
    final item = find.descendant(
      of: find.byType(MenuItemButton),
      matching: find.text('Station station-remote-02 has gone dark'),
    );
    expect(item, findsOneWidget);
    await tester.tap(item);
    await tester.pumpAndSettle();
    expect(find.text('station-control-01'), findsOneWidget);
  });

  testWidgets('search finds what the board holds and opens its page', (
    tester,
  ) async {
    await demoAt(tester, const Size(1400, 900));
    await tester.tap(find.text(OoSearch.hint));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).last, 'remote');
    await tester.pumpAndSettle();
    expect(find.text('Session preview-session-007'), findsNothing);
    await tester.tap(find.text('Station station-remote-02'));
    await tester.pumpAndSettle();
    expect(find.text('station-control-01'), findsOneWidget);
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
  });

  testWidgets('on a phone Moneypenny opens full screen, and the Overview card '
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
    expect(
      find.descendant(
        of: find.byType(ChatPanel),
        matching: find.widgetWithText(ActionChip, 'Summarise the board'),
      ),
      findsOneWidget,
    );
    await tester.tap(find.byTooltip('Close Moneypenny (Ctrl+Alt+I)'));
    await tester.pumpAndSettle();
    expect(find.byType(ChatPanel), findsNothing);

    final chip = find.widgetWithText(ActionChip, 'What needs my attention?');
    await tester.ensureVisible(chip);
    await tester.pumpAndSettle();
    await tester.tap(chip);
    await tester.pumpAndSettle();
    expect(find.byType(ChatPanel), findsOneWidget);
    expect(channel.sent.last, {
      'type': 'message',
      'text': 'What needs my attention?',
      'page': {'view': 'overview'},
    });
  });

  testWidgets('without a conversation the Overview card is disabled and says '
      'why', (tester) async {
    await atSize(tester, const Size(1400, 900));
    final board = signedIn(FakeChannel());
    addTearDown(board.dispose);
    await tester.pumpWidget(
      MaterialApp(home: MissionBoardPage(board: board, desktopWindows: false)),
    );
    await tester.pumpAndSettle();
    expect(find.byTooltip('Ask Moneypenny (Ctrl+Alt+I)'), findsNothing);
    final chip = tester.widget<ActionChip>(
      find.widgetWithText(ActionChip, 'Summarise the board'),
    );
    expect(chip.onPressed, isNull);
    expect(
      find.text('Moneypenny answers once you are signed in.'),
      findsOneWidget,
    );
  });

  test('a question asked before the conversation is ready waits for it', () {
    final channel = FakeChannel();
    final controller = AssistantController(
      token: 'user-token',
      connect: () => channel,
    );
    addTearDown(controller.dispose);
    controller.ask('Summarise the board', page: const {'view': 'overview'});
    expect(channel.sent.single, {'type': 'hello', 'token': 'user-token'});
    channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
    return pumpEventQueue().then((_) {
      expect(channel.sent.last, {
        'type': 'message',
        'text': 'Summarise the board',
        'page': {'view': 'overview'},
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
