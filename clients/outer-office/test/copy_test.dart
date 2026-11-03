import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/theme/oo_theme.dart';

void main() {
  testWidgets('every page and dialog of a window sits in one selection area', (
    tester,
  ) async {
    await tester.pumpWidget(
      OoMaterialApp(
        themeId: 'light',
        backdrop: false,
        title: 'test',
        home: Scaffold(
          body: Builder(
            builder: (context) => Column(
              children: [
                const Text('Mission G-42'),
                TextButton(
                  onPressed: () => showDialog<void>(
                    context: context,
                    builder: (_) =>
                        const AlertDialog(content: Text('In a dialog')),
                  ),
                  child: const Text('Open'),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    Finder within(String text) => find.ancestor(
      of: find.text(text),
      matching: find.byType(SelectionArea),
    );
    expect(within('Mission G-42'), findsOneWidget);
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(within('In a dialog'), findsOneWidget);
  });

  testWidgets('selected text is copied', (tester) async {
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
    await tester.pumpWidget(
      const OoMaterialApp(
        themeId: 'light',
        backdrop: false,
        title: 'test',
        home: Scaffold(body: Center(child: Text('Station worker is busy'))),
      ),
    );
    final text = find.text('Station worker is busy');
    final gesture = await tester.startGesture(
      tester.getTopLeft(text) + const Offset(1, 5),
      kind: PointerDeviceKind.mouse,
    );
    await tester.pump();
    await gesture.moveTo(tester.getTopRight(text) + const Offset(-1, 5));
    await gesture.up();
    await tester.pump();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.control);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.control);
    await tester.pump();
    expect(copied.single, 'Station worker is busy');
  });
}
