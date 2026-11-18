import 'dart:convert';
import 'dart:io';

import 'package:coqui_acp_chat/coqui_acp_chat.dart' show MessageRole;
import 'package:coqui_acp_chat_ui/coqui_acp_chat_ui.dart'
    show ChatTranscript, Composer, MessageBox;
// ignore: deprecated_member_use
import 'package:flutter/material.dart' as flutter show IconButton, TextField;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/assistant_view.dart'
    show AssistantViewCard;
import 'package:outer_office/assistant/chat_entries.dart';
import 'package:outer_office/assistant/chat_look.dart';
import 'package:outer_office/forms/form_host.dart'
    show flutterMaterialLocalizations;
import 'package:outer_office/theme/oo_theme.dart';

import 'contrast.dart';
import 'polish_test.dart' show FakeChannel;

// Outer Office keeps its own conversation and its own socket and draws through the shared
// chat's transcript and composer: its entries are adapted, its look is a preset over the shared
// tokens, and what is its own (views, proposals, cards) is passed in as renderers.

AssistantEntry _entry(
  String role,
  String text, [
  double at = 1,
  Map<String, Object?>? data,
]) => AssistantEntry(role: role, text: text, at: at, data: data);

Future<(AssistantController, FakeChannel)> _panel(
  WidgetTester tester,
  List<Map<String, Object?>> entries, {
  bool busy = false,
  double? textWidth,
  Size size = const Size(1000, 900),
  OoThemeId theme = OoThemeId.nightOps,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final channel = FakeChannel();
  final controller = AssistantController(token: 't', connect: () => channel);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    MaterialApp(
      theme: ooTheme(theme),
      localizationsDelegates: const [flutterMaterialLocalizations],
      home: Scaffold(
        body: AssistantPanel(controller: controller, textWidth: textWidth),
      ),
    ),
  );
  channel.answer({
    'type': 'ready',
    'conversation': 'c-1',
    'busy': busy,
    'entries': entries,
  });
  // Her working row pulses for as long as she answers, so nothing settles then.
  if (busy) {
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
  } else {
    await tester.pumpAndSettle();
  }
  return (controller, channel);
}

void main() {
  group('the look', () {
    for (final id in OoThemeId.values) {
      test(
        '${id.label} gives the chat its tokens, and each is readable on what it sits on',
        () {
          final t = id.tokens;
          final look = ooChatLook(t);
          expect(
            (look.bg, look.surface, look.raised, look.text, look.muted),
            (t.bg, t.surface, t.raised, t.text, t.muted),
          );
          expect(
            (look.accent, look.border, look.success, look.warning, look.danger),
            (t.accent, t.border, t.success, t.warning, t.danger),
          );
          expect(look.agentInBubble && look.userToTheRight, isTrue);
          for (final (name, text, ground) in [
            ('the person\'s words', look.text, look.user),
            ('her words', look.text, look.agent),
            ('a notice', look.muted, look.notice),
            ('a code box', look.text, look.code),
            ('the composer\'s hint', look.muted, look.composer),
            ('an added line', look.text, look.added),
            ('a removed line', look.text, look.removed),
            ('a failure on the chat', look.danger, look.surface),
          ]) {
            expect(
              contrast(text, ground),
              greaterThanOrEqualTo(4.5),
              reason: '$name in ${id.label}',
            );
          }
        },
      );
    }

    test(
      'a width sets the column, and the chat\'s own is as wide as it is',
      () {
        final t = OoThemeId.nightOps.tokens;
        expect(ooChatLook(t).messageWidth, double.infinity);
        expect(ooChatLook(t).centred, isFalse);
        expect(ooChatLook(t, width: 720, centred: true).messageWidth, 720);
        expect(ooChatLook(t, width: 720, centred: true).centred, isTrue);
      },
    );
  });

  group('her entries as the shared chat reads them', () {
    test('the person\'s words, hers and a quiet line are the shared roles; the rest are quiet lines', () {
      expect(OoEntry(_entry('person', 'hi')).role, MessageRole.user);
      expect(OoEntry(_entry('moneypenny', 'hello')).role, MessageRole.agent);
      expect(OoEntry(_entry('agent', 'hello')).role, MessageRole.agent);
      for (final role in [
        'notice',
        'error',
        'proposal',
        'view',
        'navigation',
        'page',
      ]) {
        expect(
          OoEntry(_entry(role, '')).role,
          MessageRole.notice,
          reason: role,
        );
      }
    });

    test('a character that draws as nothing shows as itself, and the time is the gateway\'s', () {
      final entry = OoEntry(_entry('person', 'a​b', 90.5));
      expect(entry.text, 'a<U+200B>b');
      expect(entry.at, DateTime.utc(1970, 1, 1, 0, 1, 30, 500));
    });

    test('a turn\'s tool names fold into one row, and older entries are noted first', () {
      final entries = ooChatEntries(
        [
          _entry('person', 'status?', 1),
          _entry('tools', 'board, board', 2),
          _entry('moneypenny', 'fine', 3),
        ],
        busy: false,
        earlier: 7,
      );
      expect(entries.map((entry) => entry.runtimeType), [
        OoNote,
        OoEntry,
        OoEntry,
        OoSources,
      ], reason: 'the answer leads, the sources follow it');
      expect(entries.first.text, '7 earlier entries are not shown.');
      expect((entries.last as OoSources).item.tools, ['board', 'board']);
    });

    test('only a view takes the whole width', () {
      expect(isWideEntry(OoEntry(_entry('view', ''))), isTrue);
      expect(isWideEntry(OoEntry(_entry('moneypenny', 'x'))), isFalse);
      expect(isWideEntry(OoNote('x', _epoch)), isFalse);
    });
  });

  group('the panel', () {
    testWidgets(
      'draws her conversation through the shared transcript and composer, and no field of its own',
      (tester) async {
        await _panel(tester, [
          {'role': 'person', 'text': 'Which Stations are busy?', 'at': 1.0},
          {'role': 'moneypenny', 'text': '**worker** is busy.', 'at': 2.0},
        ]);
        expect(find.byType(ChatTranscript), findsOneWidget);
        expect(find.byType(Composer), findsOneWidget);
        expect(find.byType(flutter.TextField), findsOneWidget);
        expect(find.byType(TextField), findsNothing);
        expect(
          find.ancestor(
            of: find.text('Which Stations are busy?'),
            matching: find.byType(MessageBox),
          ),
          findsOneWidget,
        );
      },
    );

    testWidgets(
      'while she answers there is Stop and no Send, and Enter sends nothing',
      (tester) async {
        final (_, channel) = await _panel(tester, [
          {'role': 'person', 'text': 'Which Stations are busy?', 'at': 1.0},
        ], busy: true);
        expect(find.byTooltip('Stop'), findsOneWidget);
        expect(find.byTooltip('Send'), findsNothing);
        expect(find.byTooltip('Queue'), findsNothing);
        await tester.enterText(find.byType(flutter.TextField), 'and G?');
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await tester.pump();
        expect(
          channel.sent.where((frame) => frame['type'] == 'message'),
          isEmpty,
        );
        await tester.tap(find.byTooltip('Stop'));
        await tester.pump();
        expect(channel.sent.last, {'type': 'interrupt'});
        expect(
          tester
              .widget<flutter.IconButton>(
                find.widgetWithIcon(
                  flutter.IconButton,
                  Icons.stop_circle_outlined,
                ),
              )
              .onPressed,
          isNull,
          reason: 'one interrupt is enough',
        );
      },
    );

    testWidgets('sending brings the newest into view', (tester) async {
      final (_, channel) = await _panel(tester, [
        for (var i = 0; i < 60; i++)
          {
            'role': i.isEven ? 'person' : 'moneypenny',
            'text': 'line $i',
            'at': i + 1.0,
          },
      ]);
      final scroll = tester.state<ScrollableState>(
        find
            .descendant(
              of: find.byType(ChatTranscript),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      await tester.drag(find.byType(ChatTranscript), const Offset(0, 2000));
      await tester.pumpAndSettle();
      expect(scroll.position.pixels, lessThan(scroll.position.maxScrollExtent));
      await tester.enterText(find.byType(flutter.TextField), 'now');
      await tester.tap(find.byTooltip('Send'));
      await tester.pumpAndSettle();
      expect(channel.sent.last['text'], 'now');
      expect(scroll.position.pixels, scroll.position.maxScrollExtent);
    });

    testWidgets(
      'where the chat takes the page, messages keep to a centred column and a view takes the whole width',
      (tester) async {
        final table = jsonDecode(
          File('test/fixtures/assistant_views.json').readAsStringSync(),
        )['valid']['table'];
        await _panel(
          tester,
          [
            {'role': 'person', 'text': 'Tell me about the Stations', 'at': 1.0},
            {'role': 'moneypenny', 'text': 'Here they are.', 'at': 2.0},
            {
              'role': 'view',
              'text': '',
              'at': 3.0,
              'data': {'id': 'v1', 'view': table},
            },
          ],
          textWidth: 500,
          size: const Size(1400, 900),
        );
        final answer = tester.getRect(
          find
              .ancestor(
                of: find.text('Here they are.'),
                matching: find.byType(MessageBox),
              )
              .first,
        );
        expect(answer.width, lessThanOrEqualTo(500));
        expect(
          answer.left,
          greaterThan(400),
          reason: 'the column is in the middle of 1400',
        );
        final view = tester.getRect(find.byType(AssistantViewCard));
        expect(view.width, greaterThan(1000));
      },
    );

    testWidgets('the six themes each draw the conversation without an error', (
      tester,
    ) async {
      for (final id in OoThemeId.values) {
        await _panel(tester, [
          {'role': 'person', 'text': 'hello', 'at': 1.0},
          {
            'role': 'moneypenny',
            'text': 'hello, `code` and a [link](app:/missions)',
            'at': 2.0,
          },
          {
            'role': 'notice',
            'text': 'kept',
            'at': 3.0,
            'data': {'diff': '--- a\n+++ b\n+x\n-y\n'},
          },
        ], theme: id);
        expect(tester.takeException(), isNull, reason: id.label);
        await tester.pumpWidget(const SizedBox());
      }
    });

    testWidgets(
      'switching theme with the chat open blends the look without an error',
      (tester) async {
        final channel = FakeChannel();
        final controller = AssistantController(
          token: 't',
          connect: () => channel,
        );
        addTearDown(controller.dispose);
        Widget app(OoThemeId id) => MaterialApp(
          theme: ooTheme(id),
          localizationsDelegates: const [flutterMaterialLocalizations],
          home: Scaffold(body: AssistantPanel(controller: controller)),
        );
        await tester.pumpWidget(app(OoThemeId.nightOps));
        channel.answer({
          'type': 'ready',
          'conversation': 'c-1',
          'entries': [
            {'role': 'person', 'text': 'hello', 'at': 1.0},
          ],
        });
        await tester.pumpAndSettle();
        await tester.pumpWidget(app(OoThemeId.light));
        await tester.pump(const Duration(milliseconds: 100));
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
      },
    );
  });
}

final _epoch = DateTime.utc(1970);
