import 'images.dart';

/// No browser: nothing to paste from and no file to pick.
ImageInput imageInput() => const _None();

class _None implements ImageInput {
  const _None();

  @override
  bool get canPick => false;

  @override
  Future<List<OutgoingImage>> pick() async => const [];

  @override
  Stream<OutgoingImage> get pasted => const Stream.empty();
}
