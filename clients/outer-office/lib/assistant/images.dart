import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:material_ui/material_ui.dart';

import 'image_input_stub.dart'
    if (dart.library.js_interop) 'image_input_web.dart'
    as platform;

/// An image the person adds to a message, before it is sent.
class OutgoingImage {
  const OutgoingImage(this.mediaType, this.bytes);

  /// `image/png`, `image/jpeg`, `image/webp` or `image/gif`.
  final String mediaType;
  final Uint8List bytes;

  /// The message frame's form: `{media_type, data}`, the bytes as base64.
  Map<String, Object?> toJson() => {
    'media_type': mediaType,
    'data': base64Encode(bytes),
  };

  /// [bytes] as an image when they are one of the four formats, by their first bytes rather
  /// than a name or a type someone gave them.
  static OutgoingImage? sniff(Uint8List bytes) => switch (mediaTypeOf(bytes)) {
    final type? => OutgoingImage(type, bytes),
    null => null,
  };
}

/// The format of [bytes] by their signature; `null` for anything but PNG, JPEG, WebP and GIF.
String? mediaTypeOf(Uint8List bytes) {
  bool starts(List<int> prefix, [int at = 0]) =>
      bytes.length >= at + prefix.length &&
      [for (var i = 0; i < prefix.length; i++) bytes[at + i]].join(',') ==
          prefix.join(',');
  if (starts(const [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])) {
    return 'image/png';
  }
  if (starts(const [0xFF, 0xD8, 0xFF])) return 'image/jpeg';
  if (starts(const [0x47, 0x49, 0x46, 0x38])) return 'image/gif';
  if (starts(const [0x52, 0x49, 0x46, 0x46]) &&
      starts(const [0x57, 0x45, 0x42, 0x50], 8)) {
    return 'image/webp';
  }
  return null;
}

/// Where images come from besides the app itself: what the person pastes, and the files they
/// pick. On the web both are the browser's; elsewhere there is neither.
abstract interface class ImageInput {
  /// Whether [pick] can offer a file.
  bool get canPick;

  /// Asks the person for image files; empty when they chose none.
  Future<List<OutgoingImage>> pick();

  /// Images pasted into the page, as they come.
  Stream<OutgoingImage> get pasted;
}

/// The platform's [ImageInput].
ImageInput platformImageInput() => platform.imageInput();

/// Reads an image of the conversation by its digest; `null` when it is not there any more.
typedef ImageLoader = Future<Uint8List?> Function(String sha256);

/// [image] as a thumbnail that opens [ImageViewer] on a tap.
class ImageThumbnail extends StatelessWidget {
  const ImageThumbnail({
    required this.image,
    this.label = 'Image',
    this.size = 96,
    this.onRemove,
    super.key,
  });

  final ImageProvider image;
  final String label;
  final double size;

  /// Shows a control that takes the image away, before it is sent.
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) {
    final thumbnail = Semantics(
      button: true,
      label: '$label, enlarge',
      child: InkWell(
        onTap: () => ImageViewer.show(context, image, label: label),
        borderRadius: BorderRadius.circular(8),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(8),
          child: Image(
            image: image,
            width: size,
            height: size,
            fit: BoxFit.cover,
            excludeFromSemantics: true,
            errorBuilder: (_, _, _) => SizedBox(
              width: size,
              height: size,
              child: const Icon(Icons.broken_image_outlined),
            ),
          ),
        ),
      ),
    );
    final remove = onRemove;
    if (remove == null) return thumbnail;
    return Stack(
      children: [
        thumbnail,
        Positioned(
          top: 2,
          right: 2,
          child: IconButton.filledTonal(
            // Small, so most of the thumbnail still opens the image.
            iconSize: 14,
            padding: EdgeInsets.zero,
            constraints: const BoxConstraints.tightFor(width: 24, height: 24),
            style: IconButton.styleFrom(
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
            tooltip: 'Remove $label',
            onPressed: remove,
            icon: const Icon(Icons.close),
          ),
        ),
      ],
    );
  }
}

/// An image of the conversation, stored at the gateway, read through [loader].
class StoredImage extends StatefulWidget {
  const StoredImage({
    required this.sha256,
    required this.loader,
    this.label = 'Image',
    this.size = 96,
    super.key,
  });

  final String sha256;
  final ImageLoader loader;
  final String label;
  final double size;

  @override
  State<StoredImage> createState() => _StoredImageState();
}

class _StoredImageState extends State<StoredImage> {
  late Future<Uint8List?> _bytes = widget.loader(widget.sha256);

  @override
  void didUpdateWidget(StoredImage old) {
    super.didUpdateWidget(old);
    if (old.sha256 != widget.sha256) _bytes = widget.loader(widget.sha256);
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<Uint8List?>(
    future: _bytes,
    builder: (context, snapshot) => switch (snapshot.data) {
      final bytes? => ImageThumbnail(
        image: MemoryImage(bytes),
        label: widget.label,
        size: widget.size,
      ),
      null => SizedBox.square(
        dimension: widget.size,
        child: Center(
          child: snapshot.connectionState == ConnectionState.done
              ? Tooltip(
                  message: 'This image is no longer kept.',
                  child: const Icon(Icons.hide_image_outlined),
                )
              : const CircularProgressIndicator(),
        ),
      ),
    },
  );
}

/// An image enlarged over the app, panned and zoomed with the pointer, the wheel or two fingers.
class ImageViewer extends StatelessWidget {
  const ImageViewer({required this.child, this.label = 'Image', super.key});

  /// What is enlarged: an [Image], or a drawing.
  final Widget child;
  final String label;

  /// Opens [image] enlarged; Escape or the close control ends it.
  static Future<void> show(
    BuildContext context,
    ImageProvider image, {
    String label = 'Image',
  }) => showWidget(
    context,
    Image(image: image, semanticLabel: '$label, enlarged'),
    label: label,
  );

  /// Opens [child] enlarged, as [show] does an image.
  static Future<void> showWidget(
    BuildContext context,
    Widget child, {
    String label = 'Image',
  }) => showDialog<void>(
    context: context,
    barrierLabel: 'Close the enlarged image',
    builder: (context) => ImageViewer(label: label, child: child),
  );

  @override
  Widget build(BuildContext context) => Dialog.fullscreen(
    backgroundColor: Colors.black87,
    child: Stack(
      children: [
        Positioned.fill(
          child: InteractiveViewer(
            key: const ValueKey('image-viewer'),
            minScale: 0.5,
            maxScale: 8,
            child: Center(child: child),
          ),
        ),
        Positioned(
          top: 8,
          right: 8,
          child: IconButton.filled(
            tooltip: 'Close',
            onPressed: () => Navigator.pop(context),
            icon: const Icon(Icons.close),
          ),
        ),
      ],
    ),
  );
}
