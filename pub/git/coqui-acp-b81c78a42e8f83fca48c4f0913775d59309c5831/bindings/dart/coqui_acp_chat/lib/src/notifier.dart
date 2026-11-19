/// Something that tells its listeners when it changed, with the shape of Flutter's `ChangeNotifier` so a host can wrap it in one,
/// and nothing of Flutter's.
class ChatNotifier {
  final List<void Function()> _listeners = [];
  bool _disposed = false;

  /// Whether [dispose] has run.
  bool get isDisposed => _disposed;

  /// Whether anyone listens.
  bool get hasListeners => _listeners.isNotEmpty;

  void addListener(void Function() listener) {
    if (!_disposed) _listeners.add(listener);
  }

  void removeListener(void Function() listener) => _listeners.remove(listener);

  /// Tells every listener. A listener that throws does not stop the others: the first error is rethrown after all have run.
  void notifyListeners() {
    if (_disposed) return;
    Object? error;
    StackTrace? trace;
    for (final listener in [..._listeners]) {
      // One removed by an earlier listener of this round is not told.
      if (!_listeners.contains(listener)) continue;
      try {
        listener();
      } catch (thrown, stack) {
        error ??= thrown;
        trace ??= stack;
      }
    }
    if (error != null) Error.throwWithStackTrace(error, trace!);
  }

  void dispose() {
    _disposed = true;
    _listeners.clear();
  }
}
