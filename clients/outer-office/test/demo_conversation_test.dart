import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/assistant_view.dart'
    show OutsideTextStrip;
import 'package:outer_office/assistant/chat_region.dart';
import 'package:outer_office/assistant/demo_conversation.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/overview_page.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'polish_test.dart' show FakeChannel, atSize, signedIn;

/// Long enough for any scripted reply.
const _reply = Duration(seconds: 2);

/// Taps the Overview card's quick question [prompt].
Future<void> askFromOverview(WidgetTester tester, String prompt) async {
  final chip = find.descendant(
    of: find.byType(OverviewPage),
    matching: find.widgetWithText(ActionChip, prompt),
  );
  await tester.ensureVisible(chip);
  await tester.pumpAndSettle();
  await tester.tap(chip);
  await tester.pump(_reply);
  await tester.pumpAndSettle();
}

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  testWidgets('the preview\'s Moneypenny answers from a labelled sample and '
      'changes nothing', (tester) async {
    await atSize(tester, const Size(1400, 900));
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      demo: true,
      pollInterval: null,
      // Neither her socket nor the gateway may be reached in the preview.
      assistantChannel: () => throw StateError('the preview opened a socket'),
      client: MockClient(
        (request) => throw StateError('the preview sent ${request.url}'),
      ),
    );
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.pumpAndSettle();
    expect(find.byTooltip('Ask Moneypenny (Ctrl+Alt+I)'), findsOneWidget);

    await askFromOverview(tester, 'Summarise the board');
    final chat = find.byType(ChatPanel);
    expect(
      find.descendant(of: chat, matching: find.text(demoConversationLabel)),
      findsOneWidget,
    );
    // The newest entry is at the bottom; older ones are built as the chat scrolls up to them.
    final entries = find
        .descendant(of: chat, matching: find.byType(Scrollable))
        .first;
    for (final title in [
      'station-control-01 hands mission-007 to build-box',
      'Where G runs',
      'Attempts of mission-007',
    ]) {
      await tester.scrollUntilVisible(
        find.text(title),
        120,
        scrollable: entries,
      );
    }
    expect(
      find.text(OutsideTextStrip.message),
      findsNWidgets(2),
      reason: 'a view and a page card made after reading briefs are marked',
    );
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('navigation-demo-nav-g')),
        matching: find.text(OutsideTextStrip.message),
      ),
      findsOneWidget,
    );
    await tester.scrollUntilVisible(
      find.text('The board by Station'),
      120,
      scrollable: entries,
    );
    // Back to the newest entry, where the next answer appears.
    await tester.drag(entries, const Offset(0, -10000));
    await tester.pumpAndSettle();

    await askFromOverview(tester, 'What needs my attention?');
    expect(find.text('Raise station-control-01 to 6 Missions'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, 'Confirm'));
    await tester.pumpAndSettle();
    expect(find.text('This is the demo: nothing was changed.'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(ProposalCard),
        matching: find.text('Waiting for you'),
      ),
      findsOneWidget,
      reason: 'Confirm is inert',
    );

    // Anything else gets the fallback.
    await tester.enterText(
      find.descendant(of: chat, matching: find.byType(TextField)),
      'Deploy to production',
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump(_reply);
    await tester.pumpAndSettle();
    expect(
      find.textContaining('This is the demo, so I only know the sample'),
      findsOneWidget,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('signed in, the sample never answers: messages go to Universal '
      'Exports', (tester) async {
    await atSize(tester, const Size(1400, 900));
    final channel = FakeChannel();
    final board = signedIn(channel);
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.pumpAndSettle();
    final page = tester.widget<MissionBoardPage>(find.byType(MissionBoardPage));
    expect(page.chat!.label, isNull);

    page.chat!.open(focus: false);
    await tester.pumpAndSettle();
    channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
    await tester.pumpAndSettle();
    await askFromOverview(tester, 'Summarise the board');
    expect(channel.sent.last['text'], 'Summarise the board');
    expect(find.text(demoConversationLabel), findsNothing);
    expect(find.text('The board by Station'), findsNothing);
    expect(
      find.descendant(
        of: find.byType(ChatPanel),
        matching: find.textContaining('Operation G has one Mission'),
      ),
      findsNothing,
    );
  });

  test(
    'the sample answers each quick question and falls back otherwise',
    () async {
      final channel = DemoAssistantChannel(replyDelay: Duration.zero);
      final frames = <String>[];
      final subscription = channel.incoming.listen(frames.add);
      channel.send('{"type":"hello","token":"demo"}');
      for (final prompt in [...moneypennyPrompts, 'anything']) {
        channel.send('{"type":"message","text":"$prompt"}');
      }
      await pumpEventQueue();
      final text = frames.join('\n');
      expect(text, contains('"type":"ready"'));
      expect(text, contains('"role":"proposal"'));
      expect(text, contains('"role":"view"'));
      expect(text, contains('station-remote-02'));
      expect(text, contains('only know the sample questions'));
      await subscription.cancel();
      await channel.close();
    },
  );
}
