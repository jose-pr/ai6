import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/assistant_view.dart';

/// One view of every kind and the views Universal Exports refuses, shared with its tests.
final Map<String, Object?> fixture = (jsonDecode(
  File('test/fixtures/assistant_views.json').readAsStringSync(),
) as Map).cast<String, Object?>();

Map<String, Object?> get valid => (fixture['valid'] as Map).cast();
List<Object?> get invalid => fixture['invalid'] as List;

Future<void> showView(
  WidgetTester tester,
  Object? view, {
  void Function(String link)? onOpen,
  void Function(Map<String, Object?> values)? onSubmit,
}) async {
  tester.view.physicalSize = const Size(900, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: AssistantViewCard(
            json: view,
            onOpen: onOpen,
            onSubmit: onSubmit,
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

/// The chart whose accessible label is [label].
Finder chartLabelled(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

/// A socket that keeps what the app sends and lets the test answer.
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

void main() {
  testWidgets('every kind of view renders natively from the fixture', (
    tester,
  ) async {
    final opened = <String>[];
    await showView(tester, valid['text'], onOpen: opened.add);
    expect(find.text('Why mission-7 stalled'), findsOneWidget);
    expect(find.text('Stalled'), findsOneWidget);
    expect(find.text('the Mission'), findsOneWidget);
    await tester.tap(find.text('the Mission'));
    expect(opened, ['app:/missions/G/mission-7']);

    await showView(tester, valid['table']);
    expect(find.byType(DataTable), findsOneWidget);
    expect(find.text('worker'), findsOneWidget);
    expect(find.text('—'), findsOneWidget, reason: 'a null cell');

    await showView(tester, valid['key_values']);
    expect(find.text('Headquarters'), findsOneWidget);
    expect(find.text('4'), findsOneWidget);

    await showView(tester, valid['bar_chart']);
    expect(chartLabelled('bar chart: Tokens today'), findsOneWidget);
    expect(find.text('y: tokens'), findsOneWidget);

    await showView(tester, valid['line_chart']);
    expect(chartLabelled('line chart'), findsOneWidget);

    opened.clear();
    await showView(tester, valid['cards'], onOpen: opened.add);
    expect(find.text('Mission mission-7 · Operation G'), findsOneWidget);
    expect(find.text('Station worker'), findsOneWidget);
    expect(find.text('Theater build-box'), findsOneWidget);
    await tester.tap(find.text('Station worker'));
    expect(opened, ['app:/stations/worker']);

    await showView(tester, valid['timeline']);
    expect(find.text('stalled'), findsOneWidget);
    expect(find.textContaining('no output for 600 s'), findsOneWidget);

    await showView(tester, valid['diff']);
    expect(find.text('- priority = 1'), findsOneWidget);
    expect(find.text('+ priority = 3'), findsOneWidget);
    expect(find.text('  handler = "codex"'), findsOneWidget);

    final submitted = <Map<String, Object?>>[];
    await showView(tester, valid['form'], onSubmit: submitted.add);
    expect(find.text('Edit Agent 008'), findsOneWidget);
    expect(find.text('kind: agents'), findsOneWidget);
    await tester.tap(find.text('Draft a proposal'));
    expect(submitted, [
      {'definition': '{"priority": 3}'},
    ]);
  });

  testWidgets(
    'an invalid view is refused with its reason and nothing of it shows',
    (tester) async {
      for (final item in invalid) {
        final testCase = (item as Map).cast<String, Object?>();
        await showView(tester, testCase['view']);
        expect(find.text('View refused'), findsOneWidget, reason: '$testCase');
        expect(
          find.textContaining(testCase['reason'] as String),
          findsOneWidget,
          reason: '$testCase',
        );
        expect(find.textContaining('alert(1)'), findsNothing);
        expect(find.textContaining('evil.example'), findsNothing);
      }
    },
  );

  testWidgets('the panel streams the conversation and confirms a proposal', (
    tester,
  ) async {
    final channel = FakeChannel();
    final controller = AssistantController(
      token: 'user-token',
      connect: () => channel,
    );
    addTearDown(controller.dispose);
    tester.view.physicalSize = const Size(800, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AssistantPanel(
            controller: controller,
            page: const {'view': 'stations'},
          ),
        ),
      ),
    );
    expect(channel.sent.single, {'type': 'hello', 'token': 'user-token'});
    channel.answer({
      'type': 'ready',
      'conversation': 'c-1',
      'busy': false,
      'entries': [
        {'role': 'person', 'text': 'Earlier question', 'at': 1.0},
      ],
    });
    await tester.pumpAndSettle();
    expect(find.text('Earlier question'), findsOneWidget);
    expect(
      find.byType(MoneypennyAvatar),
      findsNothing,
      reason: 'only her messages carry her avatar',
    );

    await tester.enterText(find.byType(TextField), 'Which Stations are busy?');
    await tester.tap(find.byTooltip('Send'));
    await tester.pump();
    expect(channel.sent.last, {
      'type': 'message',
      'text': 'Which Stations are busy?',
      'page': {'view': 'stations'},
    });

    channel
      ..answer({'type': 'busy', 'busy': true})
      ..answer({
        'type': 'entry',
        'entry': {'role': 'tools', 'text': 'mcp__moneypenny__board', 'at': 2.0},
      })
      ..answer({
        'type': 'entry',
        'entry': {
          'role': 'moneypenny',
          'text': '**worker** is busy: 4 of 4.',
          'at': 3.0,
        },
      });
    await tester.pump();
    await tester.pump();
    expect(find.byType(LinearProgressIndicator), findsOneWidget);
    expect(find.text('board'), findsOneWidget);
    expect(find.textContaining('is busy: 4 of 4.'), findsOneWidget);
    expect(find.byType(MoneypennyAvatar), findsOneWidget);
    expect(
      find.image(const AssetImage(MoneypennyAvatar.asset)),
      findsOneWidget,
    );
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics &&
            widget.properties.label == 'Moneypenny' &&
            widget.properties.image == true,
      ),
      findsOneWidget,
    );

    final proposal = {
      'id': 'proposal-1',
      'tool': 'propose_station_settings',
      'summary': 'Set Station worker\'s limits',
      'request': {
        'method': 'PUT',
        'path': '/v1/user/stations/worker/settings',
        'body': {'max_concurrent_missions': 6},
      },
      'needs': {'action': 'station.configure', 'on': 'station:worker'},
      'status': 'pending',
      'result': null,
    };
    channel
      ..answer({
        'type': 'entry',
        'entry': {
          'role': 'proposal',
          'text': 'Set Station worker\'s limits',
          'at': 4.0,
          'data': proposal,
        },
      })
      ..answer({'type': 'busy', 'busy': false});
    await tester.pumpAndSettle();
    expect(
      find.textContaining('PUT /v1/user/stations/worker/settings'),
      findsOneWidget,
    );
    expect(find.textContaining('Needs station.configure'), findsOneWidget);
    await tester.tap(find.text('Confirm'));
    await tester.pump();
    expect(channel.sent.last, {'type': 'confirm', 'proposal': 'proposal-1'});

    channel.answer({
      'type': 'proposal',
      'proposal': {...proposal, 'status': 'done', 'result': 'AI6 answered 200'},
    });
    await tester.pumpAndSettle();
    expect(find.text('done'), findsOneWidget);
    expect(find.text('Confirm'), findsNothing);
    expect(find.text('AI6 answered 200'), findsOneWidget);

    channel.answer({
      'type': 'error',
      'code': 'busy',
      'message': 'Moneypenny is still answering the last message.',
    });
    await tester.pumpAndSettle();
    expect(
      find.text('Moneypenny is still answering the last message.'),
      findsOneWidget,
    );
  });

  testWidgets(
    'Stop interrupts her answer and the conversation shows it stopped',
    (tester) async {
      final channel = FakeChannel();
      final controller = AssistantController(
        token: 'user-token',
        connect: () => channel,
      );
      addTearDown(controller.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: AssistantPanel(controller: controller)),
        ),
      );
      channel.answer({
        'type': 'ready',
        'conversation': 'c-1',
        'busy': false,
        'entries': const [],
      });
      await tester.pumpAndSettle();
      expect(find.byTooltip('Stop'), findsNothing, reason: 'nothing to stop');
      expect(find.byTooltip('Send'), findsOneWidget);

      channel.answer({'type': 'busy', 'busy': true});
      await tester.pump();
      await tester.pump();
      expect(find.byTooltip('Send'), findsNothing);
      await tester.tap(find.byTooltip('Stop'));
      await tester.pump();
      expect(channel.sent.last, {'type': 'interrupt'});
      expect(find.text('Stopping…'), findsOneWidget);
      expect(
        tester
            .widget<IconButton>(find.widgetWithIcon(IconButton, Icons.stop))
            .onPressed,
        isNull,
        reason: 'one interrupt is enough',
      );

      channel
        ..answer({'type': 'interrupt', 'stopping': true})
        ..answer({
          'type': 'entry',
          'entry': {
            'role': 'notice',
            'text': "You stopped Moneypenny's answer.",
            'at': 2.0,
          },
        })
        ..answer({'type': 'busy', 'busy': false});
      await tester.pumpAndSettle();
      expect(find.text("You stopped Moneypenny's answer."), findsOneWidget);
      expect(find.byIcon(Icons.stop_circle_outlined), findsOneWidget);
      expect(find.byTooltip('Stop'), findsNothing);
      expect(find.byTooltip('Send'), findsOneWidget);
      expect(
        channel.sent.where((frame) => frame['type'] == 'interrupt').length,
        1,
      );
    },
  );

  testWidgets('a proposal shows every hidden character of its request', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProposalCard(
            proposal: {
              'id': 'proposal-2',
              'summary': 'Queue a Mission',
              'request': {
                'method': 'POST',
                'path': '/v1/user/operations/G/missions',
                'body': {
                  'title': 'Tidy up',
                  'prompt': 'Delete the cache\u202E then push',
                  'tags\u200B': ['ok', 'hid\u{E0041}den'],
                },
              },
              'needs': {'action': 'mission.create', 'on': 'operation:G'},
              'status': 'pending',
            },
            onConfirm: (_) {},
            onCancel: (_) {},
          ),
        ),
      ),
    );
    final shown = tester
        .widget<SelectableText>(find.byType(SelectableText))
        .data!;
    expect(shown, contains('Delete the cache<U+202E> then push'));
    expect(shown, contains('tags<U+200B>'));
    expect(shown, contains('hid<tags: A>den'));
    expect(
      shown.runes.any((rune) => rune == 0x202E || rune == 0x200B),
      isFalse,
    );
  });

  testWidgets('the panel says when older entries were not sent', (
    tester,
  ) async {
    final channel = FakeChannel();
    final controller = AssistantController(
      token: 'user-token',
      connect: () => channel,
    );
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: AssistantPanel(controller: controller)),
      ),
    );
    channel.answer({
      'type': 'ready',
      'conversation': 'c-1',
      'busy': false,
      'earlier': 40,
      'entries': [
        {'role': 'person', 'text': 'Latest', 'at': 1.0},
      ],
    });
    await tester.pumpAndSettle();
    expect(find.text('40 earlier entries are not shown.'), findsOneWidget);
  });

  test('the socket is the gateway origin on ws or wss', () {
    expect(
      assistantSocketUri(Uri.parse('https://exports.example/app/')).toString(),
      'wss://exports.example/v1/user/moneypenny',
    );
    expect(
      assistantSocketUri(Uri.parse('http://127.0.0.1:7082')).toString(),
      'ws://127.0.0.1:7082/v1/user/moneypenny',
    );
  });

  test('a diff keeps the lines both sides share', () {
    expect(diffLines('a\nb\nc', 'a\nx\nc'), [
      (' ', 'a'),
      ('-', 'b'),
      ('+', 'x'),
      (' ', 'c'),
    ]);
  });
}
