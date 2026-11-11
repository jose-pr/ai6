import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/chat_layout.dart';
import 'package:outer_office/assistant/chat_scope.dart';
import 'package:outer_office/assistant/turns.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'polish_test.dart' show FakeChannel, atSize, signedIn;
import 'shell_navigation.dart';

AssistantEntry entry(String role, String text, [double at = 1]) =>
    AssistantEntry(role: role, text: text, at: at);

/// The entries of a finished turn that looked up [lookups] things before answering.
List<Map<String, Object?>> turn(int lookups) => [
  {'role': 'person', 'text': 'What needs my attention?', 'at': 1.0},
  for (var call = 0; call < lookups; call++)
    {
      'role': 'tools',
      'text':
          'mcp__moneypenny__${const ['missions', 'held_briefs', 'board'][call % 3]}',
      'at': 2.0 + call,
    },
  {
    'role': 'moneypenny',
    'text': 'Nothing needs attention now in All accessible Operations.',
    'at': 100.0,
  },
];

Future<MoneypennyChat> openChat(
  WidgetTester tester,
  FakeChannel channel,
  List<Map<String, Object?>> entries, {
  Size size = const Size(1400, 900),
}) async {
  await atSize(tester, size);
  final board = signedIn(channel);
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  final chat = tester
      .widget<MissionBoardPage>(find.byType(MissionBoardPage))
      .chat!;
  chat.open();
  await tester.pumpAndSettle();
  channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': entries});
  await tester.pumpAndSettle();
  return chat;
}

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  group('turnsOf', () {
    test(
      'a finished turn leads with what she said and folds its tools after',
      () {
        final items = turnsOf([
          entry('person', 'a'),
          entry('tools', 'mcp__moneypenny__board'),
          entry('error', 'the audit tool was refused'),
          entry('tools', 'mcp__moneypenny__missions, mcp__moneypenny__board'),
          entry('moneypenny', 'answer'),
        ], busy: false);
        expect(items.map((item) => item.runtimeType.toString()), [
          'EntryItem',
          'EntryItem',
          'EntryItem',
          'SourcesItem',
        ]);
        expect(
          [for (final item in items.whereType<EntryItem>()) item.entry.role],
          ['person', 'error', 'moneypenny'],
          reason: 'a failure stays inline, in its place',
        );
        expect((items.last as SourcesItem).tools, hasLength(3));
        expect((items.last as SourcesItem).live, isFalse);
      },
    );

    test('the turn being answered keeps its row where the first call was', () {
      final items = turnsOf([
        entry('person', 'a'),
        entry('tools', 'mcp__moneypenny__board'),
        entry('moneypenny', 'so far'),
        entry('tools', 'mcp__moneypenny__missions'),
      ], busy: true);
      expect(items[1], isA<SourcesItem>());
      expect((items[1] as SourcesItem).live, isTrue);
      expect((items[1] as SourcesItem).tools, hasLength(2));
      expect(items, hasLength(3));
    });

    test(
      'each turn folds its own tools, and an earlier turn is never live',
      () {
        final items = turnsOf([
          entry('person', 'one'),
          entry('tools', 'mcp__moneypenny__board'),
          entry('moneypenny', 'first'),
          entry('person', 'two'),
          entry('tools', 'mcp__moneypenny__missions'),
        ], busy: true);
        final rows = items.whereType<SourcesItem>().toList();
        expect(rows.map((row) => row.live), [false, true]);
      },
    );

    test('a conversation with no tools is unchanged', () {
      final items = turnsOf([
        entry('person', 'a'),
        entry('moneypenny', 'b'),
      ], busy: false);
      expect(items.whereType<SourcesItem>(), isEmpty);
      expect(items, hasLength(2));
    });
  });

  for (final (name, size) in [
    ('desktop', const Size(1400, 900)),
    ('phone', const Size(390, 844)),
  ]) {
    testWidgets('$name: fourteen lookups are one row beneath the answer', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      await openChat(tester, FakeChannel(), turn(14), size: size);
      expect(
        find.textContaining('Nothing needs attention now'),
        findsOneWidget,
      );
      expect(find.text('Sources checked (14)'), findsOneWidget);
      expect(find.byIcon(Icons.build_outlined), findsNothing);
      expect(
        find.bySemanticsLabel(
          'Sources checked, 14 lookups. Expand to list them.',
        ),
        findsOneWidget,
      );
      // The answer is above the sources row.
      expect(
        tester
            .getTopLeft(find.textContaining('Nothing needs attention now'))
            .dy,
        lessThan(tester.getTopLeft(find.text('Sources checked (14)')).dy),
      );
      await tester.tap(find.text('Sources checked (14)'));
      await tester.pumpAndSettle();
      expect(find.text('missions ×5'), findsOneWidget);
      expect(find.text('held briefs ×5'), findsOneWidget);
      expect(find.text('board ×4'), findsOneWidget);
      semantics.dispose();
    });
  }

  testWidgets(
    'a failure and a question stay in the transcript, not in the row',
    (tester) async {
      await openChat(tester, FakeChannel(), [
        {'role': 'person', 'text': 'q', 'at': 1.0},
        {'role': 'tools', 'text': 'mcp__moneypenny__audit', 'at': 2.0},
        {
          'role': 'error',
          'text': 'Refused: you may not read the audit log',
          'at': 3.0,
        },
        {
          'role': 'moneypenny',
          'text': 'Which Operation did you mean?',
          'at': 4.0,
        },
      ]);
      expect(find.textContaining('Refused: you may not read'), findsOneWidget);
      expect(
        find.textContaining('Which Operation did you mean?'),
        findsOneWidget,
      );
      expect(find.text('Sources checked (1)'), findsOneWidget);
    },
  );

  group('scope', () {
    test('the label names what the route is about', () {
      expect(scopeLabel(null), 'All accessible Operations');
      expect(
        scopeLabel(
          const OoRoute(OoPage.missions).withField('operation_id', 'guatiao'),
        ),
        'Operation · guatiao',
      );
      expect(
        scopeLabel(
          const OoRoute(OoPage.missions)
              .withField('operation_id', 'guatiao')
              .withField('mission_id', 'm-7'),
        ),
        'Mission · m-7',
      );
      expect(scopeLabel(const OoRoute(OoPage.stations)), 'Page · Stations');
    });

    testWidgets('it follows the page until removed, and a new page is offered, '
        'never taken', (tester) async {
      final channel = FakeChannel();
      final chat = await openChat(tester, channel, const []);
      expect(find.byKey(const ValueKey('chat-scope')), findsOneWidget);
      expect(find.text('Page · Overview'), findsOneWidget);

      // Removing it asks across everything.
      await tester.tap(
        find.byTooltip('Remove the scope: ask across everything'),
      );
      await tester.pumpAndSettle();
      expect(chat.scopeMode, ChatScopeMode.none);
      expect(find.text('All accessible Operations'), findsOneWidget);
      await tester.enterText(find.byType(TextField).last, 'hello');
      await tester.tap(find.byTooltip('Send'));
      await tester.pump();
      expect(channel.sent.last['route'], isNull);

      // Moving to another page offers it; the scope stays.
      await openPage(tester, 'Stations');
      expect(find.text('All accessible Operations'), findsOneWidget);
      expect(find.text('Ask about Page · Stations instead'), findsOneWidget);
      await tester.tap(find.text('Ask about Page · Stations instead'));
      await tester.pumpAndSettle();
      expect(chat.scopeMode, ChatScopeMode.pinned);
      expect(find.text('Page · Stations'), findsOneWidget);

      // Pinned, it stays through another page, and the picker takes an Operation.
      await openPage(tester, 'Missions');
      expect(find.text('Page · Stations'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('chat-scope-picker')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Operation · G'));
      await tester.pumpAndSettle();
      expect(find.text('Operation · G'), findsOneWidget);
      await tester.enterText(find.byType(TextField).last, 'status?');
      await tester.tap(find.byTooltip('Send'));
      await tester.pump();
      expect(channel.sent.last['route'], {
        'page': 'operations',
        'operation_id': 'G',
      });

      await tester.tap(find.byKey(const ValueKey('chat-scope-follow')));
      await tester.pumpAndSettle();
      expect(chat.scopeMode, ChatScopeMode.followPage);
    });
  });

  testWidgets('with the chat open the dashboard has no second composer', (
    tester,
  ) async {
    final chat = await openChat(tester, FakeChannel(), const []);
    expect(chat.layout, ChatLayout.docked);
    expect(find.text('Ask Moneypenny…'), findsOneWidget);
    expect(find.byKey(const ValueKey('ask-about-this-page')), findsOneWidget);
    chat.composer.unfocus();
    await tester.ensureVisible(
      find.byKey(const ValueKey('ask-about-this-page')),
    );
    await tester.tap(find.byKey(const ValueKey('ask-about-this-page')));
    await tester.pumpAndSettle();
    expect(chat.composer.hasFocus, isTrue);

    chat.collapse();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('ask-about-this-page')), findsNothing);
    expect(find.text('Ask Moneypenny…'), findsOneWidget, reason: 'the card');
  });

  testWidgets(
    'the draft, the scroll and the focus survive docked to expanded',
    (tester) async {
      final chat = await openChat(tester, FakeChannel(), [
        for (var index = 0; index < 40; index++)
          {
            'role': index.isEven ? 'person' : 'moneypenny',
            'text': 'line $index',
            'at': index.toDouble(),
          },
      ]);
      final transcript = find.descendant(
        of: find.byType(AssistantPanel),
        matching: find.byWidgetPredicate(
          (widget) =>
              widget is Scrollable && widget.axisDirection == AxisDirection.up,
        ),
      );
      await tester.enterText(find.byType(TextField).last, 'half a thought');
      chat.composer.requestFocus();
      await tester.pump();
      await tester.drag(transcript.first, const Offset(0, 300));
      await tester.pumpAndSettle();
      final scrolled = tester.state<ScrollableState>(transcript.first);
      final offset = scrolled.position.pixels;
      expect(offset, greaterThan(0));

      chat.toggleExpanded();
      await tester.pumpAndSettle();
      expect(chat.layout, ChatLayout.expanded);
      expect(chat.message.text, 'half a thought');
      expect(chat.composer.hasFocus, isTrue);
      final after = tester.state<ScrollableState>(transcript.first);
      expect(after.position.pixels, offset);

      chat.toggleExpanded();
      await tester.pumpAndSettle();
      expect(chat.layout, ChatLayout.docked);
      expect(chat.message.text, 'half a thought');
      expect(chat.composer.hasFocus, isTrue);
    },
  );

  testWidgets('tab goes from the answer to the sources row, the scope and the '
      'composer in that order', (tester) async {
    await openChat(tester, FakeChannel(), turn(3));
    bool inside(Finder finder) {
      final focus = FocusManager.instance.primaryFocus?.context;
      if (focus == null) return false;
      var found = false;
      focus.visitAncestorElements((element) {
        if (finder.evaluate().contains(element)) found = true;
        return !found;
      });
      return found;
    }

    final sources = find.byKey(const ValueKey('sources-checked'));
    final scope = find.byKey(const ValueKey('chat-scope'));
    final composer = find.byType(TextField).last;
    // Start on the sources row.
    final row = tester.element(find.text('Sources checked (3)'));
    Focus.of(row).requestFocus();
    await tester.pump();
    final seen = <String>[];
    for (var press = 0; press < 12 && seen.length < 3; press++) {
      final label = inside(sources)
          ? 'sources'
          : inside(scope)
          ? 'scope'
          : inside(composer)
          ? 'composer'
          : null;
      if (label != null && !seen.contains(label)) seen.add(label);
      await tester.sendKeyEvent(LogicalKeyboardKey.tab);
      await tester.pump();
    }
    expect(seen, ['sources', 'scope', 'composer']);
  });
}
