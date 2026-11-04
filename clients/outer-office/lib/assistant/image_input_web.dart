import 'dart:async';
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

import 'images.dart';

/// The browser's: images pasted anywhere in the page, and files picked with its own dialog.
ImageInput imageInput() => _Browser();

class _Browser implements ImageInput {
  @override
  bool get canPick => true;

  @override
  Future<List<OutgoingImage>> pick() {
    final done = Completer<List<OutgoingImage>>();
    final input = web.HTMLInputElement()
      ..type = 'file'
      ..accept = 'image/png,image/jpeg,image/webp,image/gif'
      ..multiple = true;
    input.onchange = ((web.Event _) {
      unawaited(_read(input.files).then(done.complete));
    }).toJS;
    input.oncancel = ((web.Event _) {
      if (!done.isCompleted) done.complete(const []);
    }).toJS;
    input.click();
    return done.future;
  }

  @override
  Stream<OutgoingImage> get pasted {
    late final StreamController<OutgoingImage> images;
    JSFunction? listener;
    images = StreamController<OutgoingImage>(
      onListen: () {
        listener = ((web.ClipboardEvent event) {
          unawaited(
            _read(event.clipboardData?.files).then((found) {
              for (final image in found) {
                images.add(image);
              }
            }),
          );
        }).toJS;
        web.document.addEventListener('paste', listener);
      },
      onCancel: () {
        if (listener case final listener?) {
          web.document.removeEventListener('paste', listener);
        }
      },
    );
    return images.stream;
  }
}

/// The images among [files], by their bytes.
Future<List<OutgoingImage>> _read(web.FileList? files) async {
  final found = <OutgoingImage>[];
  if (files == null) return found;
  for (var index = 0; index < files.length; index++) {
    final file = files.item(index);
    if (file == null) continue;
    final buffer = await file.arrayBuffer().toDart;
    if (OutgoingImage.sniff(Uint8List.view(buffer.toDart)) case final image?) {
      found.add(image);
    }
  }
  return found;
}
