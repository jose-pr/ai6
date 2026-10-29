import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/chat_layout.dart';
import 'package:outer_office/assistant/chat_region.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/settings/oo_config.dart';
import 'package:outer_office/shell/app_window.dart';
import 'package:outer_office/shell/overview_page.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'polish_test.dart' show FakeChannel, atSize, signedIn;
import 'shell_navigation.dart';

/// A window that only records being closed.
class FakeWindow implements AppWindow {
  bool closed = false;
  int activated = 0;

  @override
  void close() => closed = true;

  @override
  void activate() => activated++;
}

/// Signs in as [board]'s person, lets her conversation connect, and returns its chat.
Future<MoneypennyChat> signIn(
  WidgetTester tester,
  MissionBoard board,
  FakeChannel channel,
) async {
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  return tester.widget<MissionBoardPage>(find.byType(MissionBoardPage)).chat!;
}

/// Answers the hello the panel sends when it first opens.
Future<void> connect(WidgetTester tester, FakeChannel channel) async {
  if (channel.sent.isEmpty) return;
  channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
  await tester.pumpAndSettle();
}

/// Presses [key] with Ctrl+Alt (and Shift), the chat's chord off macOS.
Future<void> chord(
  WidgetTester tester,
  LogicalKeyboardKey key, {
  bool shift = false,
  String? character,
}) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
  if (shift) await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyDownEvent(key, character: character);
  await tester.sendKeyUpEvent(key);
  if (shift) await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  await tester.pumpAndSettle();
}

/// The drag handle, by its label.
final _handle = find.byWidgetPredicate(
  (widget) =>
      widget is Semantics && widget.properties.label == 'Moneypenny width',
);

Finder get _sheetBarrier => find.descendant(
  of: find.byType(ChatRegion),
  matching: find.byType(ModalBarrier),
);

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  testWidgets('docked is beside the board when wide, a modal sheet when medium '
      'and the whole screen when narrow', (tester) async {
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);

    await atSize(tester, const Size(1400, 900));
    final chat = await signIn(tester, board, channel);
    chat.open();
    await tester.pumpAndSettle();
    await connect(tester, channel);
    final panel = tester.getRect(find.byType(ChatPanel));
    final page = tester.getRect(find.byType(OverviewPage));
    expect(panel.right, 1400);
    expect(panel.width, const ChatSettings().width);
    expect(page.right, lessThanOrEqualTo(panel.left));
    expect(_sheetBarrier, findsNothing, reason: 'not modal beside the board');
    expect(_handle, findsOneWidget, reason: 'the drag handle is labelled');

    await atSize(tester, const Size(1000, 800));
    await tester.pumpAndSettle();
    expect(_sheetBarrier, findsOneWidget);
    expect(
      find.byType(OverviewPage),
      findsOneWidget,
      reason: 'under the sheet',
    );
    expect(tester.getRect(find.byType(ChatPanel)).right, 1000);
    // Tapping the page behind the sheet closes it.
    await tester.tapAt(const Offset(200, 400));
    await tester.pumpAndSettle();
    expect(chat.layout, ChatLayout.collapsed);

    await atSize(tester, const Size(800, 1600));
    chat.open();
    await tester.pumpAndSettle();
    expect(tester.getSize(find.byType(ChatPanel)), const Size(800, 1600));
    expect(find.byType(AppBar), findsNothing, reason: 'one view at a time');
    expect(find.byTooltip('Expand (Ctrl+Alt+Shift+I)'), findsNothing);
  });

  testWidgets('the chat follows the window across every breakpoint, drawn in '
      'full', (tester) async {
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    await atSize(tester, const Size(800, 800));
    final chat = await signIn(tester, board, channel);
    chat.open(focus: false);
    await tester.pumpAndSettle();
    await connect(tester, channel);
    for (final layout in [ChatLayout.docked, ChatLayout.expanded]) {
      chat.open(to: layout, focus: false);
      for (final width in <double>[800, 1000, 1400, 800, 1400, 1000, 800]) {
        await atSize(tester, Size(width, 800));
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull, reason: '$layout at $width');
        final panel = tester.getRect(find.byType(ChatPanel));
        expect(panel.right, width, reason: '$layout at $width');
        for (final opacity in tester.widgetList<Opacity>(
          find.ancestor(
            of: find.byType(ChatPanel),
            matching: find.byType(Opacity),
          ),
        )) {
          expect(opacity.opacity, 1, reason: '$layout at $width');
        }
      }
    }
  });

  testWidgets('expanded hides the board and keeps its filter for docked', (
    tester,
  ) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel)
      ..sessions = const [
        SessionSummary(
          operationId: 'G',
          missionId: 'mission-1',
          sessionId: 'session-blocked',
          state: 'blocked',
          sequence: 1,
        ),
        SessionSummary(
          operationId: 'G',
          missionId: 'mission-2',
          sessionId: 'session-working',
          state: 'working',
          sequence: 1,
        ),
      ];
    addTearDown(board.dispose);
    final chat = await signIn(tester, board, channel);
    await openPage(tester, 'Missions');
    await tester.tap(find.byTooltip('Filter by blocked'));
    await tester.pumpAndSettle();
    expect(find.text('Mission mission-2'), findsNothing);
    expect(find.text('Mission mission-1'), findsOneWidget);

    await chord(tester, LogicalKeyboardKey.keyI, shift: true);
    await connect(tester, channel);
    expect(chat.layout, ChatLayout.expanded);
    expect(find.text('Mission board'), findsNothing);
    expect(
      find.text('Mission board', skipOffstage: false),
      findsOneWidget,
      reason: 'the board stays mounted',
    );
    expect(tester.getSize(find.byType(ChatPanel)).width, 1400 - 226 - 1);

    await tester.tap(find.widgetWithText(ActionChip, 'Back to Missions'));
    await tester.pumpAndSettle();
    expect(chat.layout, ChatLayout.docked);
    expect(find.text('Mission mission-1'), findsOneWidget);
    expect(find.text('Mission mission-2'), findsNothing);

    // The rail stays while expanded; choosing a page shows it.
    chat.toggleExpanded();
    await tester.pumpAndSettle();
    await openPage(tester, 'Overview');
    expect(chat.layout, ChatLayout.docked);
    expect(find.byType(OverviewPage), findsOneWidget);
  });

  testWidgets('Ctrl+Alt+I, Ctrl+Alt+Shift+I and Esc move between the layouts', (
    tester,
  ) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    final chat = await signIn(tester, board, channel);
    expect(chat.layout, ChatLayout.collapsed);

    // Hidden: open where it was last open, the composer focused.
    await chord(tester, LogicalKeyboardKey.keyI);
    await connect(tester, channel);
    expect(chat.layout, ChatLayout.docked);
    expect(chat.composer.hasFocus, isTrue);

    // Focus inside the chat: hide.
    await chord(tester, LogicalKeyboardKey.keyI);
    expect(chat.layout, ChatLayout.collapsed);
    expect(find.byType(ChatPanel), findsNothing);

    // Open with focus elsewhere: focus the composer, stay open.
    await chord(tester, LogicalKeyboardKey.keyI);
    chat.composer.unfocus();
    await tester.pumpAndSettle();
    await chord(tester, LogicalKeyboardKey.keyI);
    expect(chat.layout, ChatLayout.docked);
    expect(chat.composer.hasFocus, isTrue);

    // Esc in the composer: focus back to the page.
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(chat.layout, ChatLayout.docked);
    expect(chat.composer.hasFocus, isFalse);

    await chord(tester, LogicalKeyboardKey.keyI, shift: true);
    expect(chat.layout, ChatLayout.expanded);
    await chord(tester, LogicalKeyboardKey.keyI, shift: true);
    expect(chat.layout, ChatLayout.docked);

    // Esc in expanded with focus in the chat: docked.
    await chord(tester, LogicalKeyboardKey.keyI, shift: true);
    chat.focusComposer();
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(chat.layout, ChatLayout.docked);

    // Hidden after expanded: opens expanded again.
    await chord(tester, LogicalKeyboardKey.keyI, shift: true);
    chat.focusComposer();
    await tester.pumpAndSettle();
    await chord(tester, LogicalKeyboardKey.keyI);
    expect(chat.layout, ChatLayout.collapsed);
    await chord(tester, LogicalKeyboardKey.keyI);
    expect(chat.layout, ChatLayout.expanded);

    // Each header button names its shortcut.
    for (final tooltip in [
      'Back to the side (Ctrl+Alt+Shift+I)',
      'Hide Moneypenny (Ctrl+Alt+I)',
    ]) {
      expect(find.byTooltip(tooltip), findsWidgets, reason: tooltip);
    }
    await tester.tap(find.widgetWithIcon(IconButton, Icons.close_fullscreen));
    await tester.pumpAndSettle();
    for (final tooltip in [
      'Expand (Ctrl+Alt+Shift+I)',
      'Hide Moneypenny (Ctrl+Alt+I)',
    ]) {
      expect(find.byTooltip(tooltip), findsWidgets, reason: tooltip);
    }
    expect(find.byTooltip('Pop out (Ctrl+Alt+O)'), findsNothing);
    // Without her own window Ctrl+Alt+O is not hers.
    await chord(tester, LogicalKeyboardKey.keyO);
    expect(chat.layout, ChatLayout.docked);

    // Esc in a modal sheet closes it.
    await atSize(tester, const Size(1000, 800));
    chat.focusComposer();
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(chat.layout, ChatLayout.collapsed);
  });

  testWidgets('a chord that types a character in the composer changes '
      'nothing', (tester) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    final chat = await signIn(tester, board, channel);
    await chord(tester, LogicalKeyboardKey.keyI);
    await connect(tester, channel);
    expect(chat.composer.hasFocus, isTrue);

    // AltGr+I on US-International arrives as Ctrl+Alt+I typing "í".
    await chord(tester, LogicalKeyboardKey.keyI, character: 'í');
    expect(chat.layout, ChatLayout.docked);
    expect(chat.composer.hasFocus, isTrue);
    await chord(tester, LogicalKeyboardKey.keyI, shift: true, character: 'Í');
    expect(chat.layout, ChatLayout.docked);

    // A browser reports Ctrl+Alt+I with its own letter: that is still the shortcut.
    await chord(tester, LogicalKeyboardKey.keyI, character: 'i');
    expect(chat.layout, ChatLayout.collapsed);
  });

  testWidgets('Enter sends and Shift+Enter does not', (tester) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    final chat = await signIn(tester, board, channel);
    await chord(tester, LogicalKeyboardKey.keyI);
    await connect(tester, channel);
    await tester.enterText(find.byType(TextField).last, 'Status?');
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    expect(channel.sent.last['type'], 'hello');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(channel.sent.last, {
      'type': 'message',
      'text': 'Status?',
      'page': {'view': 'overview'},
    });
    expect(chat.message.text, isEmpty);
  });

  testWidgets('a dot says she answered while the chat was hidden', (
    tester,
  ) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    final chat = await signIn(tester, board, channel);
    chat.open();
    await tester.pumpAndSettle();
    await connect(tester, channel);
    chat.collapse();
    await tester.pumpAndSettle();
    Badge dot() => tester.widget<Badge>(
      find.descendant(
        of: find.byType(MoneypennyButton),
        matching: find.byType(Badge),
      ),
    );
    expect(dot().isLabelVisible, isFalse);
    channel.answer({
      'type': 'entry',
      'entry': {
        'role': 'moneypenny',
        'text': 'Two Stations are dark.',
        'at': 5,
      },
    });
    await tester.pumpAndSettle();
    expect(dot().isLabelVisible, isTrue);
    await tester.tap(find.byTooltip('Ask Moneypenny (Ctrl+Alt+I)'));
    await tester.pumpAndSettle();
    expect(chat.unseen, isFalse);
    expect(find.text('Two Stations are dark.'), findsOneWidget);
  });

  testWidgets('each person\'s layout and width are restored separately', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({
      ChatPreferences.key('ana'): jsonEncode({
        'layout': 'expanded',
        'width': 400,
      }),
      ChatPreferences.key('ben'): jsonEncode({
        'layout': 'docked',
        'width': 560,
      }),
    });
    await atSize(tester, const Size(1400, 900));
    final anaChannel = FakeChannel();
    final ana = signedIn(anaChannel);
    addTearDown(ana.dispose);
    final anaChat = await signIn(tester, ana, anaChannel);
    expect(anaChat.layout, ChatLayout.expanded);
    expect(find.byType(OverviewPage), findsNothing);

    // Ben changes his width; Ana's stays.
    final benChannel = FakeChannel();
    final ben = signedIn(benChannel)
      ..userSession = const UserSessionSummary(
        personId: 'ben',
        name: 'ben',
        displayName: 'Ben Lee',
        groups: [],
        expiresAt: 1_900_000_000,
        operations: [],
      );
    addTearDown(ben.dispose);
    final benChat = await signIn(tester, ben, benChannel);
    expect(benChat.layout, ChatLayout.docked);
    expect(tester.getSize(find.byType(ChatPanel)).width, 560);
    benChat.resize(608);
    await tester.pumpAndSettle();

    final preferences = await SharedPreferences.getInstance();
    expect(jsonDecode(preferences.getString(ChatPreferences.key('ben'))!), {
      'layout': 'docked',
      'width': 608,
    });
    expect(jsonDecode(preferences.getString(ChatPreferences.key('ana'))!), {
      'layout': 'expanded',
      'width': 400,
    });

    // Narrow, a remembered docked or expanded chat starts hidden.
    final narrow = MoneypennyChat(
      controller: AssistantController(token: 't', connect: FakeChannel.new),
      preferences: ChatPreferences(SharedPreferences.getInstance()),
    );
    addTearDown(narrow.dispose);
    await narrow.restore('ana', narrow: true);
    expect(narrow.layout, ChatLayout.collapsed);
    expect(narrow.lastOpen, ChatLayout.expanded);
  });

  testWidgets('OO.cfg [chat] sizes the docked chat and its limits', (
    tester,
  ) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    final settings = OoConfig.parse(
      '[chat]\nmin_width = 300\nmax_width = 500\nwidth = 450\nwidth_step = 25\n',
    ).chat;
    await tester.pumpWidget(
      OuterOfficeApp(
        board: board,
        desktopWindows: false,
        chatSettings: settings,
      ),
    );
    await tester.pumpAndSettle();
    final chat = tester
        .widget<MissionBoardPage>(find.byType(MissionBoardPage))
        .chat!;
    chat.open(focus: false);
    await tester.pumpAndSettle();
    expect(tester.getSize(find.byType(ChatPanel)).width, 450);
    await tester.drag(_handle, const Offset(-400, 0));
    await tester.pumpAndSettle();
    expect(chat.width, 500, reason: 'max_width');
    Focus.of(
      tester.element(
        find.descendant(of: _handle, matching: find.byType(MouseRegion)).first,
      ),
    ).requestFocus();
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pumpAndSettle();
    expect(chat.width, 475, reason: 'width_step');
  });

  testWidgets('the drag handle resizes the chat by drag and by arrow keys', (
    tester,
  ) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    final chat = await signIn(tester, board, channel);
    chat.open(focus: false);
    await tester.pumpAndSettle();
    await tester.drag(_handle, const Offset(-100, 0));
    await tester.pumpAndSettle();
    expect(chat.width, const ChatSettings().width + 100);
    Focus.of(
      tester.element(
        find.descendant(of: _handle, matching: find.byType(MouseRegion)).first,
      ),
    ).requestFocus();
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pumpAndSettle();
    expect(
      chat.width,
      const ChatSettings().width + 100 + const ChatSettings().widthStep,
    );
    await tester.drag(_handle, const Offset(2000, 0));
    await tester.pumpAndSettle();
    expect(chat.width, const ChatSettings().minWidth);
  });

  testWidgets('with desktop windows Pop out opens her window once, and closing '
      'it docks the chat', (tester) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    final opened = <VoidCallback>[];
    final window = FakeWindow();
    final chat = MoneypennyChat(
      controller: AssistantController(
        token: 'user-token',
        connect: () => channel,
      ),
      openWindow: (context, chat, onClosed) {
        opened.add(onClosed);
        return window;
      },
    );
    addTearDown(chat.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: MissionBoardPage(board: board, desktopWindows: true, chat: chat),
      ),
    );
    chat.open();
    await tester.pumpAndSettle();
    await connect(tester, channel);

    await tester.tap(find.byTooltip('Pop out (Ctrl+Alt+O)'));
    await tester.pumpAndSettle();
    expect(opened, hasLength(1));
    expect(chat.layout, ChatLayout.popped);
    expect(find.byType(ChatPanel), findsNothing);
    expect(find.text('Moneypenny is in her own window'), findsOneWidget);

    // The person closes her window.
    opened.single();
    await tester.pumpAndSettle();
    expect(chat.layout, ChatLayout.docked);
    expect(find.byType(ChatPanel), findsOneWidget);
    expect(opened, hasLength(1));

    // Ctrl+Alt+O pops out again; Bring back closes the window.
    await chord(tester, LogicalKeyboardKey.keyO);
    expect(opened, hasLength(2));
    await tester.tap(find.text('Bring back'));
    await tester.pumpAndSettle();
    expect(window.closed, isTrue);
    expect(chat.layout, ChatLayout.docked);
    expect(opened, hasLength(2));
  });
}
