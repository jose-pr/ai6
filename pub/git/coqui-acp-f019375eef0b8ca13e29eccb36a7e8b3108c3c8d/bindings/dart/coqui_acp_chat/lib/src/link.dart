import 'dart:async';
import 'dart:convert';

/// A channel between a conversation and a view of it that lives somewhere else: another window, another engine, another page. It
/// carries JSON values only (maps, lists, strings, numbers, booleans, null), so any message channel can be one: a platform channel,
/// a `BroadcastChannel`, a pipe. It does not open, verify or retry anything: who may attach is the host's to decide before it
/// serves one.
abstract interface class ChatViewLink {
  /// What the other end sent, in order. It ends when the link does.
  Stream<Map<String, Object?>> get messages;

  /// Sends [message] to the other end. A message sent on a closed link is dropped.
  void send(Map<String, Object?> message);

  /// Whether nothing more can be sent or received.
  bool get isClosed;

  /// Closes this end. The other end's [messages] end too.
  Future<void> close();
}

/// Two ends of a link in memory, for tests and for views in one isolate. Each message is delivered after the sender's current
/// work, as a channel would deliver it, and goes through JSON when [wire] is true, so what it carries is proved to be JSON.
final class MemoryViewLink implements ChatViewLink {
  MemoryViewLink._(this._wire);

  /// The two ends: what one sends, the other receives.
  static (MemoryViewLink, MemoryViewLink) pair({bool wire = true}) {
    final a = MemoryViewLink._(wire), b = MemoryViewLink._(wire);
    a._other = b;
    b._other = a;
    return (a, b);
  }

  final bool _wire;
  late final MemoryViewLink _other;
  final _incoming = StreamController<Map<String, Object?>>();
  bool _closed = false;

  @override
  Stream<Map<String, Object?>> get messages => _incoming.stream;

  @override
  bool get isClosed => _closed;

  @override
  void send(Map<String, Object?> message) {
    if (_closed) return;
    final delivered = _wire
        ? (jsonDecode(jsonEncode(message)) as Map).cast<String, Object?>()
        : message;
    scheduleMicrotask(() {
      if (!_other._incoming.isClosed) _other._incoming.add(delivered);
    });
  }

  @override
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    unawaited(_incoming.close());
    // The other end hears of it after what was sent before, as it would on a channel.
    scheduleMicrotask(() {
      _other._closed = true;
      if (!_other._incoming.isClosed) unawaited(_other._incoming.close());
    });
  }
}
