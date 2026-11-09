import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/chat_commands.dart';

import 'polish_test.dart' show FakeChannel;

class FakeCommands implements ChatCommands {
  FakeCommands(this.commands);

  final List<ChatCommand> commands;
  final asked = <String?>[];

  @override
  Future<List<ChatCommand>> load({String? conversation}) async {
    asked.add(conversation);
    return commands;
  }
}

const _skills = [
  ChatCommand(
    name: 'report-writer',
    description: 'Writes a report.',
    argumentHint: '[period]',
  ),
  ChatCommand(name: 'charts', description: 'Draws charts.'),
  ChatCommand(name: 'compact', description: 'Shrinks it.', source: 'harness'),
];

Future<
  ({FakeChannel channel, FakeCommands commands, TextEditingController text})
>
panel(WidgetTester tester) async {
  final channel = FakeChannel();
  final commands = FakeCommands(_skills);
  final controller = AssistantController(token: 't', connect: () => channel);
  addTearDown(controller.dispose);
  final text = TextEditingController();
  addTearDown(text.dispose);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: AssistantPanel(
          controller: controller,
          commands: commands,
          message: text,
        ),
      ),
    ),
  );
  channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
  await tester.pumpAndSettle();
  return (channel: channel, commands: commands, text: text);
}

Finder get _field => find.byType(TextField);
Finder _row(String name) => find.byKey(ValueKey('command-$name'));

void main() {
  testWidgets('typing / opens the menu of her skills and harness commands', (
    tester,
  ) async {
    final p = await panel(tester);
    expect(_row('charts'), findsNothing);
    await tester.enterText(_field, '/');
    await tester.pumpAndSettle();
    expect(_row('report-writer'), findsOneWidget);
    expect(_row('charts'), findsOneWidget);
    expect(_row('compact'), findsOneWidget);
    expect(
      find.textContaining('/report-writer', findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining('[period]', findRichText: true), findsOneWidget);
    expect(find.text('Writes a report.'), findsOneWidget);
    expect(p.commands.asked, ['c-1'], reason: 'asked for this conversation');
  });

  testWidgets('the menu filters as you type and a pick inserts /name', (
    tester,
  ) async {
    final p = await panel(tester);
    await tester.enterText(_field, '/co');
    await tester.pumpAndSettle();
    expect(_row('compact'), findsOneWidget);
    expect(_row('report-writer'), findsNothing);
    expect(_row('charts'), findsNothing);

    await tester.enterText(_field, '/rep');
    await tester.pumpAndSettle();
    expect(_row('report-writer'), findsOneWidget);
    expect(_row('compact'), findsNothing);
    await tester.tap(_row('report-writer'));
    await tester.pumpAndSettle();
    expect(p.text.text, '/report-writer ');
    expect(_row('report-writer'), findsNothing, reason: 'closed once picked');
    expect(
      p.text.selection,
      const TextSelection.collapsed(offset: '/report-writer '.length),
    );
  });

  testWidgets('the arrow keys move and Enter picks without sending', (
    tester,
  ) async {
    final p = await panel(tester);
    await tester.enterText(_field, '/');
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(p.text.text, '/compact ');
    expect(
      p.channel.sent.where((frame) => frame['type'] == 'message'),
      isEmpty,
    );
  });

  testWidgets('Escape closes the menu and typing on opens it again', (
    tester,
  ) async {
    await panel(tester);
    await tester.enterText(_field, '/c');
    await tester.pumpAndSettle();
    expect(_row('charts'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(_row('charts'), findsNothing);
    await tester.enterText(_field, '/ch');
    await tester.pumpAndSettle();
    expect(_row('charts'), findsOneWidget);
  });

  testWidgets('a / that does not open the message, or is past its first '
      'word, offers nothing', (tester) async {
    await panel(tester);
    await tester.enterText(_field, 'about /charts');
    await tester.pumpAndSettle();
    expect(_row('charts'), findsNothing);
    await tester.enterText(_field, '/charts last week');
    await tester.pumpAndSettle();
    expect(_row('charts'), findsNothing);
  });

  testWidgets('a name no menu lists is sent as text, and the chat says so', (
    tester,
  ) async {
    final p = await panel(tester);
    await tester.enterText(_field, '/nothing at all');
    await tester.pumpAndSettle();
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await tester.pumpAndSettle();
    final message = p.channel.sent.singleWhere(
      (frame) => frame['type'] == 'message',
    );
    expect(message['text'], '/nothing at all');
    // The gateway echoes the person's message; the notice follows it.
    p.channel.answer({
      'type': 'entry',
      'entry': {'role': 'person', 'text': '/nothing at all', 'at': 1.0},
    });
    await tester.pumpAndSettle();
    expect(
      find.textContaining('/nothing is not one of her commands'),
      findsOneWidget,
    );
  });

  testWidgets('a name in the menu is sent as it is, with no notice', (
    tester,
  ) async {
    final p = await panel(tester);
    await tester.enterText(_field, '/charts');
    await tester.pumpAndSettle();
    await tester.enterText(_field, '/charts last week');
    await tester.pumpAndSettle();
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await tester.pumpAndSettle();
    p.channel.answer({
      'type': 'entry',
      'entry': {'role': 'person', 'text': '/charts last week', 'at': 1.0},
    });
    await tester.pumpAndSettle();
    expect(
      p.channel.sent.singleWhere((frame) => frame['type'] == 'message')['text'],
      '/charts last week',
    );
    expect(find.textContaining('is not one of her commands'), findsNothing);
  });

  test('which word is a command, which are arguments', () {
    expect(commandQuery('/', 1), '');
    expect(commandQuery('/ch', 3), 'ch');
    expect(commandQuery('/ch', 0), isNull, reason: 'cursor before the slash');
    expect(commandQuery('/ch arg', 3), 'ch');
    expect(
      commandQuery('/ch arg', 6),
      isNull,
      reason: 'cursor in the arguments',
    );
    expect(commandQuery('hi /ch', 6), isNull);
    expect(typedCommand('  /charts x'), 'charts');
    expect(typedCommand('/'), isNull);
    expect(typedCommand('no'), isNull);
  });

  test('names that start with the query come before those that contain it', () {
    expect(matchingCommands(_skills, 'c').map((c) => c.name), [
      'charts',
      'compact',
    ]);
    expect(matchingCommands(_skills, 'WRITES').map((c) => c.name), [
      'report-writer',
    ], reason: 'the description, any case');
    expect(matchingCommands(_skills, ''), _skills);
    expect(matchingCommands(_skills, 'zzz'), isEmpty);
  });

  test('inserting keeps what follows the first word', () {
    final inserted = insertCommand('/re last week', 'report-writer');
    expect(inserted.text, '/report-writer last week');
    expect(inserted.cursor, '/report-writer '.length);
    expect(insertCommand('/', 'charts').text, '/charts ');
  });

  test('a command is read from the gateway\'s JSON', () {
    final command = ChatCommand.fromJson({
      'name': 'charts',
      'description': 'Draws.',
      'argument_hint': null,
      'source': 'harness',
    })!;
    expect(command.name, 'charts');
    expect(command.argumentHint, isNull);
    expect(command.source, 'harness');
    expect(ChatCommand.fromJson({'description': 'no name'}), isNull);
    expect(ChatCommand.fromJson('x'), isNull);
  });
}
