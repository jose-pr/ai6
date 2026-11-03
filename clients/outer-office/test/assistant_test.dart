import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/services.dart' show SystemChannels;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/assistant_view.dart';
import 'package:outer_office/assistant/export.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/settings/oo_config.dart' show ChatSettings;
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/terminal/session_connection.dart'
    show sessionSocketUri;

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

/// The chart, schedule or graph whose accessible summary begins with [label].
Finder chartLabelled(String label) => find.byWidgetPredicate(
  (widget) =>
      widget is Semantics && (widget.properties.label ?? '').startsWith(label),
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

  /// The socket ends without the app having closed it.
  void dropped() => unawaited(_incoming.close());

  @override
  Future<void> close() => _incoming.close();
}

void main() {
  testWidgets(
    'a reset password reaches the person once and no entry holds it',
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
      channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
      await tester.pumpAndSettle();
      channel.answer({
        'type': 'reveal',
        'proposal': 'p-1',
        'secret': 'n3w-s3cret',
      });
      await tester.pumpAndSettle();
      expect(find.text('n3w-s3cret'), findsOneWidget);
      expect(controller.hasReveal, isFalse, reason: 'taken by the dialog');
      expect(
        controller.entries.any(
          (entry) => '${entry.data}'.contains('n3w-s3cret'),
        ),
        isFalse,
      );
      await tester.tap(find.text('Done'));
      await tester.pumpAndSettle();
      expect(find.text('n3w-s3cret'), findsNothing);
    },
  );

  testWidgets('every kind of view renders natively from the fixture', (
    tester,
  ) async {
    final opened = <String>[];
    await showView(tester, valid['text'], onOpen: opened.add);
    expect(find.text('Why mission-7 stalled'), findsOneWidget);
    expect(find.text('Stalled'), findsOneWidget);
    expect(find.textRange.ofSubstring('the Mission'), findsOneWidget);
    await tester.tapOnText(find.textRange.ofSubstring('the Mission'));
    expect(opened, ['app:/missions?operation_id=G&mission_id=mission-7']);

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
    expect(opened, ['app:/stations?station_id=worker']);

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
        // What an SVG may hold is checked once, by Universal Exports.
        if (testCase['gateway_only'] == true) continue;
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

  testWidgets(
    'charts, schedules, graphs and drawings render from the fixture',
    (tester) async {
      await showView(tester, valid['bar_chart']);
      expect(find.byType(BarChart), findsOneWidget);
      expect(
        chartLabelled('bar chart: Tokens today. 1 series'),
        findsOneWidget,
      );

      await showView(tester, valid['stacked_bar_chart']);
      final stacked = tester.widget<BarChart>(find.byType(BarChart)).data;
      expect(stacked.barGroups, hasLength(2), reason: 'one bar per Operation');
      expect(
        stacked.barGroups.first.barRods.single.rodStackItems,
        hasLength(3),
      );
      expect(stacked.barGroups.first.barRods.single.toY, 17);
      expect(
        chartLabelled('stacked bar chart: Missions by state'),
        findsOneWidget,
      );

      await showView(tester, valid['line_chart']);
      expect(find.byType(LineChart), findsOneWidget);

      await showView(tester, valid['pie_chart']);
      final pie = tester.widget<PieChart>(find.byType(PieChart)).data;
      expect([for (final slice in pie.sections) slice.value], [2, 5, 0]);
      expect(find.text('worker: 5'), findsOneWidget, reason: 'the legend');

      final opened = <String>[];
      await showView(tester, valid['schedule'], onOpen: opened.add);
      final schedule = tester.widget<BarChart>(find.byType(BarChart)).data;
      expect(schedule.rotationQuarterTurns, 1, reason: 'horizontal bars');
      expect(schedule.barGroups, hasLength(2));
      final first = schedule.barGroups.first.barRods.single;
      expect(first.fromY, 0);
      expect(first.toY, 40 * 60, reason: 'forty minutes from the first start');
      expect(
        chartLabelled('schedule: Attempts of mission-7. 2 rows'),
        findsOneWidget,
      );
      await tester.tap(find.text('attempt 1'));
      expect(opened, ['app:/missions?operation_id=G&mission_id=mission-7']);

      opened.clear();
      await showView(tester, valid['graph'], onOpen: opened.add);
      for (final node in [
        'Operation G',
        'hq',
        'worker',
        'build-box',
        'mission-7',
      ]) {
        expect(find.text(node), findsOneWidget, reason: node);
      }
      expect(find.text('stalled'), findsOneWidget, reason: "a node's state");
      expect(
        chartLabelled('graph: Where G runs. 5 nodes and 4 edges'),
        findsOneWidget,
      );
      await tester.tap(find.text('hq'));
      expect(opened, ['app:/stations?station_id=hq']);

      await showView(tester, valid['tree_graph']);
      for (final node in [
        'Operation G',
        'mission-1',
        'mission-2',
        'session-1',
      ]) {
        expect(find.text(node), findsOneWidget, reason: node);
      }

      await showView(tester, valid['svg']);
      final drawing = tester.widget<SvgPicture>(find.byType(SvgPicture));
      expect(drawing.semanticsLabel, 'hq hands mission-7 to worker');
    },
  );

  testWidgets('a chart, schedule or graph shows the same values as a table', (
    tester,
  ) async {
    await showView(tester, valid['stacked_bar_chart']);
    expect(find.byType(DataTable), findsNothing);
    await tester.tap(find.text('Show as table'));
    await tester.pumpAndSettle();
    expect(find.byType(BarChart), findsNothing);
    final table = tester.widget<DataTable>(find.byType(DataTable));
    expect(
      [for (final column in table.columns) (column.label as Text).data],
      ['Operation', 'done', 'failed', 'working'],
    );
    List<String?> cells(DataRow row) => [
      for (final cell in row.cells) (cell.child as Text).data,
    ];
    expect(cells(table.rows[0]), ['G', '12', '2', '3']);
    expect(cells(table.rows[1]), [
      'Q',
      '4',
      '1',
      '—',
    ], reason: 'no point is not 0');
    await tester.tap(find.text('Show as stacked bar chart'));
    await tester.pumpAndSettle();
    expect(find.byType(BarChart), findsOneWidget);

    await showView(tester, valid['pie_chart']);
    await tester.tap(find.text('Show as table'));
    await tester.pumpAndSettle();
    expect(find.text('71.4%'), findsOneWidget);

    await showView(tester, valid['schedule']);
    await tester.tap(find.text('Show as table'));
    await tester.pumpAndSettle();
    expect(find.text('2026-10-29 08:00:00Z'), findsOneWidget);
    expect(find.text('2026-10-29 08:10:00Z'), findsOneWidget, reason: 'in UTC');
    expect(find.text('2026-10-29 08:55:30.500Z'), findsOneWidget);

    await showView(tester, valid['graph']);
    await tester.tap(find.text('Show as table'));
    await tester.pumpAndSettle();
    expect(find.text('worker, hq'), findsNothing);
    expect(find.text('hq, worker'), findsOneWidget, reason: 'where G leads');
    expect(
      find.text('build-box (ssh)'),
      findsOneWidget,
      reason: 'an edge label',
    );
  });

  testWidgets(
    'a graph keeps its nodes through a layout that is not a rebuild',
    (tester) async {
      await showView(tester, valid['graph']);
      expect(find.text('worker'), findsOneWidget);
      // A web font arriving relays out text without rebuilding the view.
      for (final element in find.text('worker').evaluate()) {
        element.renderObject!.markNeedsLayout();
      }
      tester.renderObject(find.byType(Stack).first).markNeedsLayout();
      await tester.pump();
      expect(find.text('worker'), findsOneWidget);
      expect(find.text('build-box'), findsOneWidget);
      expect(
        find.text('ssh'),
        findsNothing,
        reason: 'an edge label is painted, not a widget',
      );
    },
  );

  testWidgets('views are held to the limits Universal Exports sent', (
    tester,
  ) async {
    const tight = ViewLimits(graphNodes: 2, svgBytes: 64);
    expect(
      () => AssistantView.parse(valid['graph'], limits: tight),
      throwsA(
        isA<FormatException>().having(
          (error) => error.message,
          'message',
          contains('a graph has 1 to 2 nodes'),
        ),
      ),
    );
    expect(
      () => AssistantView.parse(valid['svg'], limits: tight),
      throwsA(isA<FormatException>()),
    );
    AssistantView.parse(valid['graph']);
    final wide = ViewLimits.fromJson({
      'graph_nodes': 600,
      'rows': 0,
      'svg_bytes': 'x',
    });
    expect(wide.graphNodes, 600);
    expect(
      wide.rows,
      const ViewLimits().rows,
      reason: 'below 1 keeps the default',
    );
    expect(wide.svgBytes, const ViewLimits().svgBytes);

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
      'view_limits': {'graph_nodes': 2},
      'entries': [
        {
          'role': 'view',
          'text': '',
          'at': 1.0,
          'data': {'id': 'view-1', 'view': valid['graph']},
        },
      ],
    });
    await tester.pumpAndSettle();
    expect(controller.viewLimits.graphNodes, 2);
    expect(find.text('View refused'), findsOneWidget);
  });

  testWidgets('what she keeps in her memory shows its diff in the chat', (
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
      'entries': [
        {
          'role': 'notice',
          'text': 'Moneypenny keeps in her memory: How they like answers',
          'at': 1.0,
          'data': {'diff': '--- before\n+++ after\n+Prefers\u200b tables.\n'},
        },
        {'role': 'notice', 'text': 'Stopped.', 'at': 2.0},
      ],
    });
    await tester.pumpAndSettle();
    expect(find.textContaining('keeps in her memory'), findsOneWidget);
    expect(find.text('+++ after'), findsOneWidget);
    expect(
      find.text('+Prefers<U+200B> tables.'),
      findsOneWidget,
      reason: 'a character that renders as nothing shows here as anywhere',
    );
    expect(find.text('Stopped.'), findsOneWidget);
  });

  testWidgets('the menu exports the conversation as Markdown', (tester) async {
    final channel = FakeChannel();
    final controller = AssistantController(
      token: 'user-token',
      connect: () => channel,
    );
    addTearDown(controller.dispose);
    final saved = <(String, String)>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AssistantPanel(
            controller: controller,
            saveText: (name, text) async {
              saved.add((name, text));
              return '/home/me/Downloads/$name';
            },
          ),
        ),
      ),
    );
    channel.answer({
      'type': 'ready',
      'conversation': 'c-1',
      'busy': false,
      'entries': [
        {'role': 'person', 'text': 'Which Stations are busy?', 'at': 1.0},
        {'role': 'moneypenny', 'text': '**worker** is busy.', 'at': 2.0},
      ],
    });
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Moneypenny menu'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Export conversation'));
    await tester.pumpAndSettle();
    expect(saved.single.$1, 'moneypenny-c-1.md');
    expect(saved.single.$2, contains('conversation: "c-1"'));
    expect(saved.single.$2, contains('## Moneypenny · '));
    expect(saved.single.$2, contains('**worker** is busy.'));
    expect(
      find.text('Saved to /home/me/Downloads/moneypenny-c-1.md'),
      findsOneWidget,
    );
  });

  test('an export keeps entry data in a fence its backticks cannot close', () {
    final text = conversationMarkdown(
      conversationId: 'c/1',
      entries: const [
        AssistantEntry(
          role: 'proposal',
          text: 'Set limits',
          at: 0,
          data: {'note': 'has ``` inside'},
        ),
      ],
      exportedAt: DateTime.utc(2026, 11, 3),
    );
    expect(text, startsWith('---\nconversation: "c/1"\n'));
    expect(text, contains('exported_at: 2026-11-03T00:00:00.000Z'));
    expect(text, contains('## Proposal · 1970-01-01T00:00:00.000Z'));
    expect(text, contains('````json'));
    expect(exportFileName('c/1'), 'moneypenny-c-1.md');
  });

  testWidgets('no view carries a strip, whatever an entry says', (
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
      'entries': [
        {
          'role': 'view',
          'text': '',
          'at': 1.0,
          'data': {'id': 'view-1', 'view': valid['table'], 'exposed': true},
        },
      ],
    });
    await tester.pumpAndSettle();
    expect(find.textContaining('outside AI6'), findsNothing);
    expect(find.byIcon(Icons.warning_amber_rounded), findsNothing);
  });

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
            route: const OoRoute(OoPage.stations),
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
    // Her portrait heads the panel; in the conversation only her messages carry it.
    final inConversation = find.descendant(
      of: find.byType(ListView),
      matching: find.byType(MoneypennyAvatar),
    );
    expect(find.byType(MoneypennyAvatar), findsOneWidget);
    expect(
      inConversation,
      findsNothing,
      reason: 'only her messages carry her avatar',
    );

    await tester.enterText(find.byType(TextField), 'Which Stations are busy?');
    await tester.tap(find.byTooltip('Send'));
    await tester.pump();
    expect(channel.sent.last, {
      'type': 'message',
      'text': 'Which Stations are busy?',
      'route': {'page': 'stations'},
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
    final copied = <String>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied.add((call.arguments as Map)['text'] as String);
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await tester.tap(find.byTooltip('Copy'));
    await tester.pump();
    expect(copied, ['**worker** is busy: 4 of 4.'], reason: 'her Markdown');
    expect(inConversation, findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(ListView),
        matching: find.image(const AssetImage(MoneypennyAvatar.asset)),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: find.byType(ListView),
        matching: find.byWidgetPredicate(
          (widget) =>
              widget is Semantics &&
              widget.properties.label == 'Moneypenny' &&
              widget.properties.image == true,
        ),
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
    expect(
      find.textContaining('needs station.configure on station:worker'),
      findsOneWidget,
    );
    await tester.tap(find.text('Confirm'));
    await tester.pump();
    expect(channel.sent.last, {'type': 'confirm', 'proposal': 'proposal-1'});

    channel.answer({
      'type': 'proposal',
      'proposal': {...proposal, 'status': 'done', 'result': 'AI6 answered 200'},
    });
    await tester.pumpAndSettle();
    expect(find.text('Applied'), findsOneWidget);
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

  test('a socket URL takes nothing of the page it was opened from', () {
    final page = Uri.parse('https://oo.example:8443/app/?x=1#/operations/G');
    expect(
      assistantSocketUri(page).toString(),
      'wss://oo.example:8443/v1/user/moneypenny',
    );
    expect(
      sessionSocketUri(page, 's 1', 7).toString(),
      'wss://oo.example:8443/v1/socket/s%201?after=7',
    );
    expect(
      sessionSocketUri(Uri.parse('http://127.0.0.1:7082'), 's', 0).toString(),
      'ws://127.0.0.1:7082/v1/socket/s?after=0',
    );
  });

  group('a socket that drops', () {
    late List<FakeChannel> channels;
    late AssistantController controller;

    Future<void> panel(WidgetTester tester) async {
      channels = [];
      controller = AssistantController(
        token: 'user-token',
        connect: () {
          final channel = FakeChannel();
          channels.add(channel);
          return channel;
        },
        retryDelay: const Duration(seconds: 1),
        retryMaxDelay: const Duration(seconds: 4),
      );
      addTearDown(controller.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: AssistantPanel(controller: controller)),
        ),
      );
      await tester.pump();
    }

    void ready(FakeChannel channel) => channel.answer({
      'type': 'ready',
      'conversation': 'c-1',
      'entries': const <Object>[],
    });

    testWidgets('comes back after a wait that doubles to its cap, and the '
        'panel says it is reconnecting', (tester) async {
      await panel(tester);
      ready(channels.last);
      await tester.pump();
      expect(controller.connected, isTrue);

      channels.last.dropped();
      await tester.pump();
      expect(find.text('Reconnecting…'), findsOneWidget);
      await tester.pump(const Duration(milliseconds: 999));
      expect(channels, hasLength(1));
      await tester.pump(const Duration(milliseconds: 1));
      expect(channels, hasLength(2));
      expect(channels.last.sent.single['conversation'], 'c-1');

      for (final wait in [2, 4, 4]) {
        channels.last.dropped();
        await tester.pump();
        final before = channels.length;
        await tester.pump(
          Duration(seconds: wait) - const Duration(milliseconds: 1),
        );
        expect(channels, hasLength(before), reason: 'still waiting $wait s');
        await tester.pump(const Duration(milliseconds: 1));
        expect(channels, hasLength(before + 1));
      }

      ready(channels.last);
      await tester.pump();
      await tester.pump();
      expect(find.text('Reconnecting…'), findsNothing);
      channels.last.dropped();
      await tester.pump();
      final before = channels.length;
      await tester.pump(const Duration(seconds: 1));
      expect(channels, hasLength(before + 1), reason: 'ready starts over');
    });

    testWidgets('a socket that never says ready counts as dropped', (
      tester,
    ) async {
      await panel(tester);
      await tester.pump(ChatSettings.defaultConnectTimeout);
      expect(find.text('Reconnecting…'), findsOneWidget);
      await tester.pump(const Duration(seconds: 1));
      expect(channels, hasLength(2));
    });

    testWidgets('a refused sign-in does not come back', (tester) async {
      await panel(tester);
      channels.last.answer({
        'type': 'error',
        'code': 'unauthorized',
        'message': 'Sign in again to talk to Moneypenny.',
      });
      await tester.pump();
      channels.last.dropped();
      await tester.pump();
      expect(find.text('Reconnecting…'), findsNothing);
      expect(find.text('Sign in again to talk to Moneypenny.'), findsOneWidget);
      await tester.pump(const Duration(hours: 1));
      expect(channels, hasLength(1));
      await tester.tap(find.text('Try again'));
      await tester.pump();
      expect(channels, hasLength(2), reason: 'the person may still ask');
    });

    testWidgets('another refusal does not stop it', (tester) async {
      await panel(tester);
      ready(channels.last);
      channels.last.answer({
        'type': 'error',
        'code': 'bad_request',
        'message': 'That frame is not understood.',
      });
      await tester.pump();
      channels.last.dropped();
      await tester.pump(const Duration(seconds: 1));
      expect(channels, hasLength(2));
    });

    testWidgets('a person can stop it, and it stays stopped', (tester) async {
      await panel(tester);
      ready(channels.last);
      await tester.pump();
      channels.last.dropped();
      await tester.pump();
      await tester.tap(find.text('Stop trying'));
      await tester.pump();
      expect(find.text('Reconnecting…'), findsNothing);
      expect(find.text('Stop trying'), findsNothing);
      await tester.pump(const Duration(hours: 1));
      expect(channels, hasLength(1));
      await tester.tap(find.text('Try again'));
      await tester.pump();
      expect(channels, hasLength(2));
    });

    testWidgets('one dropped while the person is stopping an answer stays '
        'down', (tester) async {
      await panel(tester);
      ready(channels.last);
      channels.last.answer({'type': 'busy', 'busy': true});
      await tester.pump();
      controller.interrupt();
      await tester.pump();
      channels.last.dropped();
      await tester.pump();
      await tester.pump(const Duration(hours: 1));
      expect(channels, hasLength(1));
      expect(find.text('Try again'), findsOneWidget);
    });

    testWidgets('a message asked while it waits goes at once and is sent when '
        'ready', (tester) async {
      await panel(tester);
      ready(channels.last);
      await tester.pump();
      channels.last.dropped();
      await tester.pump();
      controller.ask('still there?');
      await tester.pump();
      expect(channels, hasLength(2));
      ready(channels.last);
      await tester.pump();
      expect(channels.last.sent.last['text'], 'still there?');
      await tester.pump(const Duration(seconds: 8));
      expect(channels, hasLength(2), reason: 'no second attempt is left over');
    });

    testWidgets('a closed conversation does not come back', (tester) async {
      channels = [];
      final closing = AssistantController(
        token: 'user-token',
        connect: () {
          final channel = FakeChannel();
          channels.add(channel);
          return channel;
        },
        retryDelay: const Duration(seconds: 1),
      )..open();
      channels.last.dropped();
      await tester.pump();
      closing.dispose();
      await tester.pump(const Duration(hours: 1));
      expect(channels, hasLength(1));
    });
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
