import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/images.dart';

import 'assistant_test.dart' show FakeChannel;

/// A 1x1 transparent PNG.
final Uint8List png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==',
);

/// Images the test pastes, and files it offers to pick.
class FakeImages implements ImageInput {
  final _pasted = StreamController<OutgoingImage>.broadcast();
  List<OutgoingImage> picked = const [];

  void paste(Uint8List bytes) => _pasted.add(OutgoingImage.sniff(bytes)!);

  @override
  bool get canPick => true;

  @override
  Future<List<OutgoingImage>> pick() async => picked;

  @override
  Stream<OutgoingImage> get pasted => _pasted.stream;
}

Future<(FakeChannel, FakeImages)> showPanel(
  WidgetTester tester, {
  ImageLoader? loader,
}) async {
  tester.view.physicalSize = const Size(800, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final channel = FakeChannel();
  final images = FakeImages();
  final controller = AssistantController(
    token: 'user-token',
    connect: () => channel,
  );
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: AssistantPanel(
          controller: controller,
          imageInput: images,
          imageLoader: loader,
        ),
      ),
    ),
  );
  channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
  await tester.pumpAndSettle();
  return (channel, images);
}

void main() {
  test('only the four formats are taken, by their bytes', () {
    expect(mediaTypeOf(png), 'image/png');
    expect(
      mediaTypeOf(Uint8List.fromList([0xFF, 0xD8, 0xFF, 0xE0])),
      'image/jpeg',
    );
    expect(mediaTypeOf(ascii.encode('GIF89a')), 'image/gif');
    expect(
      mediaTypeOf(ascii.encode('RIFF\x00\x00\x00\x00WEBPVP8 ')),
      'image/webp',
    );
    expect(
      mediaTypeOf(ascii.encode('<svg xmlns="http://www.w3.org/2000/svg"/>')),
      isNull,
    );
    expect(mediaTypeOf(ascii.encode('%PDF-1.7')), isNull);
  });

  testWidgets(
    'a pasted image shows as a thumbnail, enlarges and goes with the message',
    (tester) async {
      final (channel, images) = await showPanel(tester);
      images.paste(png);
      await tester.pumpAndSettle();
      expect(find.bySemanticsLabel('Image 1, enlarge'), findsOneWidget);

      await tester.tap(find.byKey(const ValueKey('composer-image-0')));
      await tester.pumpAndSettle();
      final viewer = find.byKey(const ValueKey('image-viewer'));
      expect(viewer, findsOneWidget);
      final zoom = tester.widget<InteractiveViewer>(viewer);
      expect(zoom.scaleEnabled && zoom.panEnabled && zoom.maxScale > 1, isTrue);
      expect(find.bySemanticsLabel('Image 1, enlarged'), findsOneWidget);
      await tester.tap(find.byTooltip('Close'));
      await tester.pumpAndSettle();
      expect(viewer, findsNothing);

      await tester.enterText(find.byType(TextField), 'What is wrong here?');
      await tester.tap(find.byTooltip('Send'));
      await tester.pumpAndSettle();
      final sent = channel.sent.last;
      expect(sent['type'], 'message');
      expect(sent['images'], [
        {'media_type': 'image/png', 'data': base64Encode(png)},
      ]);
      expect(
        find.bySemanticsLabel('Image 1, enlarge'),
        findsNothing,
        reason: 'sent',
      );
    },
  );

  testWidgets('a picked image can be removed before sending', (tester) async {
    final (channel, images) = await showPanel(tester);
    images.picked = [OutgoingImage.sniff(png)!];
    await tester.tap(find.byTooltip('Add an image'));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Remove Image 1'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Never mind.');
    await tester.tap(find.byTooltip('Send'));
    await tester.pumpAndSettle();
    expect(channel.sent.last.containsKey('images'), isFalse);
  });

  testWidgets(
    'a message of the conversation shows its stored images, which enlarge',
    (tester) async {
      final asked = <String>[];
      final (channel, _) = await showPanel(
        tester,
        loader: (sha256) async {
          asked.add(sha256);
          return png;
        },
      );
      channel.answer({
        'type': 'entry',
        'entry': {
          'role': 'person',
          'text': 'This one.',
          'at': 1,
          'data': {
            'images': [
              {'sha256': 'abc', 'media_type': 'image/png', 'bytes': png.length},
            ],
          },
        },
      });
      await tester.pumpAndSettle();
      expect(asked, ['abc']);
      expect(find.byType(ImageThumbnail), findsOneWidget);
      await tester.tap(find.byType(ImageThumbnail));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('image-viewer')), findsOneWidget);
    },
  );
}
