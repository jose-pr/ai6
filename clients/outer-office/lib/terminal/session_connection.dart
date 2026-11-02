import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:guatiao/wire.dart' as wire;
import 'package:web_socket_channel/web_socket_channel.dart';

import '../main.dart' show MissionBoard, SessionSummary;
import '../routes/socket_uri.dart';
import 'socket_connector.dart';

/// What the terminal shows where events of its Session cannot be had: the gateway trimmed them from
/// its history or lost them restarting, or the Station dropped them while the gateway was away.
const missingOutputNotice =
    '\r\n\x1b[2m[AI6: output is missing here]\x1b[0m\r\n';

/// Owns one read-only-by-default attachment to an existing provider Session.
/// The socket of [sessionId] on the gateway's origin, from the output after [after].
Uri sessionSocketUri(Uri apiOrigin, String sessionId, int after) => socketUri(
  apiOrigin,
  '/v1/socket/${Uri.encodeComponent(sessionId)}',
  {'after': '$after'},
);

class SessionConnection extends ChangeNotifier {
  SessionConnection({required this.board, required this.session});

  final MissionBoard board;
  final SessionSummary session;
  final StreamController<List<int>> _output =
      StreamController<List<int>>.broadcast(sync: true);

  Stream<List<int>> get output => _output.stream;
  String? get runtimeError => board.terminalRuntimeError;
  bool get isConnected => _connected;
  bool get writeLeaseRequested => _writeRequested;
  bool get writeLeaseHeld => _connected && _writeRequested;

  String get status {
    if (runtimeError != null) return runtimeError!;
    if (_connected) {
      return writeLeaseHeld ? 'Write lease held' : 'Connected · read only';
    }
    if (_writeRequested) return 'Requesting input lease · reconnecting';
    return _error ? 'Disconnected · reconnecting' : 'Connecting · read only';
  }

  WebSocketChannel? _channel;
  StreamSubscription<dynamic>? _subscription;
  wire.Receiver? _receiver;
  Timer? _retry;
  late Duration _retryDelay = board.limits.terminal.retryDelay;

  /// The last event this connection received, so a reconnect replays what it has not shown. The
  /// board's sequence says nothing of that: it moves on with events this terminal never saw.
  int _cursor = 0;
  int _inputSequence = 0;
  int _generation = 0;
  bool _connected = false;
  bool _writeRequested = false;
  bool _error = false;
  bool _disposed = false;

  void start() {
    if (runtimeError != null || board.demo || board.accessToken == null) {
      return;
    }
    unawaited(_connect(++_generation));
  }

  Future<void> setWriteLease(bool requested) async {
    if (_disposed || requested == _writeRequested) {
      return;
    }
    _writeRequested = requested;
    _error = false;
    _retry?.cancel();
    final generation = ++_generation;
    await _closeCurrent();
    if (!_disposed) notifyListeners();
    unawaited(_connect(generation));
  }

  Future<void> _connect(int generation) async {
    if (_disposed || runtimeError != null) return;
    _connected = false;
    if (!_disposed) notifyListeners();
    wire.Receiver? receiver;
    WebSocketChannel? channel;
    try {
      final cookie = await board.prepareSessionSocket(
        session,
        write: _writeRequested,
      );
      if (_disposed || generation != _generation) return;
      channel = connectSessionSocket(
        sessionSocketUri(board.apiOrigin, session.sessionId, _cursor),
        protocols: const ['ai6.guatiao'],
        origin: board.apiOrigin.origin,
        cookie: cookie,
      );
      await channel.ready.timeout(board.limits.terminal.connectTimeout);
      if (_disposed || generation != _generation) {
        await channel.sink.close();
        return;
      }
      if (channel.protocol != 'ai6.guatiao') {
        throw StateError(
          'The gateway did not negotiate the AI6 wire protocol.',
        );
      }
      receiver = wire.Receiver();
      _channel = channel;
      _receiver = receiver;
      _connected = true;
      _error = false;
      _retryDelay = board.limits.terminal.retryDelay;
      notifyListeners();
      _subscription = channel.stream.listen(
        (message) => _receive(generation, message),
        onError: (Object _) => _connectionEnded(generation),
        onDone: () => _connectionEnded(generation),
        cancelOnError: true,
      );
    } on Object {
      receiver?.close();
      if (channel != null && !identical(channel, _channel)) {
        unawaited(channel.sink.close());
      }
      if (_disposed || generation != _generation) return;
      _connected = false;
      _error = true;
      notifyListeners();
      _scheduleRetry(generation);
    }
  }

  void _receive(int generation, Object? message) {
    if (_disposed || generation != _generation) return;
    if (message is! List<int>) {
      _connectionEnded(generation);
      return;
    }
    try {
      final event = _receiver!.accept(message);
      final value = event.value;
      if (value == null) return;
      try {
        if (event.kind == wire.Frame.schema) return;
        final raw = value.toDart();
        if (raw is! Map) return;
        final frame = Map<String, Object?>.from(raw);
        if (frame['session_id'] != session.sessionId) return;
        if (event.channel == 5) {
          _gap(frame['before']);
          return;
        }
        final sequence = frame['sequence'];
        if (sequence is! int) return;
        if (sequence > _cursor) {
          _cursor = sequence;
        }
        if (event.channel == 1) {
          final state = frame['state'];
          if (state is String) {
            final reason = frame['reason'];
            board.applyLifecycle(
              session.sessionId,
              state,
              sequence,
              reason: reason is String ? reason : null,
            );
          }
        } else if (event.channel == 2 && frame['direction'] == 'output') {
          final bytes = frame['bytes'];
          if (bytes is List<int> && bytes.isNotEmpty) {
            _output.add(Uint8List.fromList(bytes));
          }
        }
      } finally {
        value.close();
      }
    } on Object {
      _connectionEnded(generation);
    }
  }

  /// Shows that events before [before] are gone, unless this connection has what came before.
  void _gap(Object? before) {
    if (before is! int || before - 1 <= _cursor) return;
    _cursor = before - 1;
    _output.add(utf8.encode(missingOutputNotice));
  }

  void sendInput(List<int> bytes) {
    if (!_canSend) return;
    _send(2, {
      'session_id': session.sessionId,
      'sequence': ++_inputSequence,
      'direction': 'input',
      'bytes': Uint8List.fromList(bytes),
    });
  }

  void sendResize(int columns, int rows) {
    if (!_canSend) return;
    _send(3, {
      'session_id': session.sessionId,
      'sequence': ++_inputSequence,
      'columns': columns,
      'rows': rows,
    });
  }

  bool get _canSend => _connected && _writeRequested && _channel != null;

  void _send(int channel, Object value) {
    final socket = _channel;
    if (!_connected || !_writeRequested || socket == null) return;
    try {
      socket.sink.add(wire.send(channel, value));
    } on Object {
      _connectionEnded(_generation);
    }
  }

  void _connectionEnded(int generation) {
    if (_disposed || generation != _generation || !_connected) {
      return;
    }
    _connected = false;
    _receiver?.close();
    _receiver = null;
    _channel = null;
    unawaited(_subscription?.cancel());
    _subscription = null;
    notifyListeners();
    _scheduleRetry(generation);
  }

  void _scheduleRetry(int generation) {
    if (_disposed || generation != _generation || _retry?.isActive == true) {
      return;
    }
    final delay = _retryDelay;
    final longest = board.limits.terminal.retryMaxDelay;
    final doubled = _retryDelay * 2;
    _retryDelay = doubled > longest ? longest : doubled;
    _retry = Timer(delay, () {
      if (!_disposed && generation == _generation) {
        unawaited(_connect(++_generation));
      }
    });
  }

  Future<void> _closeCurrent() async {
    _connected = false;
    final subscription = _subscription;
    _subscription = null;
    await subscription?.cancel();
    _receiver?.close();
    _receiver = null;
    final channel = _channel;
    _channel = null;
    if (channel != null) await channel.sink.close();
  }

  @override
  void dispose() {
    _disposed = true;
    _retry?.cancel();
    ++_generation;
    unawaited(_closeCurrent());
    unawaited(_output.close());
    super.dispose();
  }
}
