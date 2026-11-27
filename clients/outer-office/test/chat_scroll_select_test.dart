import 'package:coqui_acp_chat_ui/coqui_acp_chat_ui.dart' show ChatTranscript;
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/forms/form_host.dart'
    show flutterMaterialLocalizations;
import 'package:outer_office/theme/oo_theme.dart';

import 'polish_test.dart' show FakeChannel;

Map<String, Object?> _said(String role, String text, double at) => {
  'role': role,
  'text': text,
  'at': at,
};

Future<FakeChannel> _open(
  WidgetTester tester,
  List<Map<String, Object?>> entries, {
  bool busy = false,
}) async {
  tester.view.physicalSize = const Size(1000, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final channel = FakeChannel();
  final controller = AssistantController(token: 't', connect: () => channel);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    MaterialApp(
      theme: ooTheme(OoThemeId.nightOps),
      localizationsDelegates: const [flutterMaterialLocalizations],
      home: Scaffold(body: AssistantPanel(controller: controller)),
    ),
  );
  channel.answer({
    'type': 'ready',
    'conversation': 'c-1',
    'busy': busy,
    'entries': entries,
  });
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 300));
  return channel;
}

ScrollPosition _position(WidgetTester tester) => tester
    .widget<ListView>(
      find.descendant(
        of: find.byType(ChatTranscript),
        matching: find.byType(ListView),
      ),
    )
    .controller!
    .position;

void main() {
  testWidgets('scrolled up while an answer streams, the view stays where the '
      'person left it', (tester) async {
    final channel = await _open(tester, [
      for (var i = 0; i < 60; i++)
        _said(i.isEven ? 'person' : 'moneypenny', 'message $i', 1.0 + i),
    ], busy: true);
    expect(_position(tester).extentAfter, 0);

    final list = find.descendant(
      of: find.byType(ChatTranscript),
      matching: find.byType(ListView),
    );
    await tester.drag(list, const Offset(0, 120));
    await tester.pump();
    final offset = _position(tester).pixels;
    expect(offset, lessThan(_position(tester).maxScrollExtent));

    for (var i = 0; i < 4; i++) {
      channel.answer({
        'type': 'entry',
        'entry': _said('moneypenny', 'streamed\nlines\nhere\n$i', 100.0 + i),
      });
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      expect(
        _position(tester).pixels,
        offset,
        reason: 'a view the person scrolled is not moved by what arrives',
      );
    }
  });

  testWidgets(
    'a selection dragged across two messages copies both with Ctrl+C',
    (tester) async {
      String? copied;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied = (call.arguments as Map)['text'] as String;
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
      await _open(tester, [
        _said('person', 'first question', 1),
        _said('moneypenny', 'the first answer', 2),
        _said('person', 'second question', 3),
      ]);
      final start =
          tester.getTopLeft(find.text('first question')) + const Offset(1, 5);
      final end =
          tester.getBottomRight(find.text('second question')) -
          const Offset(1, 5);
      final gesture = await tester.startGesture(
        start,
        kind: PointerDeviceKind.mouse,
      );
      await tester.pump();
      await gesture.moveTo(end);
      await tester.pump();
      await gesture.up();
      await tester.pump();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pump();
      expect(copied, contains('first question'));
      expect(copied, contains('the first answer'));
      expect(copied, contains('second question'));
    },
  );
}
