import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:moneypenny/terminal/session_terminal.dart';
import 'package:xterm2/xterm.dart';

void main() {
  test(
    'terminal output decoder keeps UTF-8 sequences split between chunks',
    () async {
      final decoded = await decodeTerminalOutput(
        Stream<List<int>>.fromIterable([
          [0xe2],
          [0x82, 0xac],
        ]),
      ).toList();

      expect(decoded, ['€']);
    },
  );

  testWidgets(
    'terminal is read-only unless the current view holds a write lease',
    (tester) async {
      final output = StreamController<List<int>>();
      addTearDown(output.close);

      await tester.pumpWidget(
        MaterialApp(
          home: SizedBox(
            width: 640,
            height: 400,
            child: SessionTerminal(output: output.stream),
          ),
        ),
      );
      expect(
        tester.widget<TerminalView>(find.byType(TerminalView)).readOnly,
        isTrue,
      );

      await tester.pumpWidget(
        MaterialApp(
          home: SizedBox(
            width: 640,
            height: 400,
            child: SessionTerminal(
              output: output.stream,
              onInput: (_) {},
              writeLeaseHeld: true,
            ),
          ),
        ),
      );
      expect(
        tester.widget<TerminalView>(find.byType(TerminalView)).readOnly,
        isFalse,
      );
    },
  );

  testWidgets('terminal reports a normally closed stream', (tester) async {
    final output = StreamController<List<int>>();
    await tester.pumpWidget(
      MaterialApp(
        home: SizedBox(
          width: 640,
          height: 400,
          child: SessionTerminal(output: output.stream),
        ),
      ),
    );

    await output.close();
    await tester.pump();

    expect(find.text('Terminal stream ended.'), findsOneWidget);
  });

  testWidgets('terminal distinguishes stream errors from normal completion', (
    tester,
  ) async {
    final output = StreamController<List<int>>();
    await tester.pumpWidget(
      MaterialApp(
        home: SizedBox(
          width: 640,
          height: 400,
          child: SessionTerminal(output: output.stream),
        ),
      ),
    );

    output.addError(StateError('connection lost'));
    await output.close();
    await tester.pump();

    expect(find.text('Terminal stream disconnected.'), findsOneWidget);
  });
}
