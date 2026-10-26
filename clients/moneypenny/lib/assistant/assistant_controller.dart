import 'dart:async';
import 'dart:convert';

import 'package:material_ui/material_ui.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

/// A text channel to Universal Exports' Moneypenny socket; tests give a fake one.
abstract interface class AssistantChannel {
  Stream<String> get incoming;
  void send(String frame);
  Future<void> close();
}

class _SocketChannel implements AssistantChannel {
  _SocketChannel(this._socket);

  final WebSocketChannel _socket;

  @override
  Stream<String> get incoming =>
      _socket.stream.where((frame) => frame is String).cast();

  @override
  void send(String frame) => _socket.sink.add(frame);

  @override
  Future<void> close() => _socket.sink.close();
}

/// `/v1/user/moneypenny` on the gateway's origin, as a WebSocket URL.
Uri assistantSocketUri(Uri apiOrigin) => apiOrigin.replace(
  scheme: apiOrigin.scheme == 'https' ? 'wss' : 'ws',
  path: '/v1/user/moneypenny',
  query: null,
  fragment: null,
);

/// Opens the real socket.
AssistantChannel connectAssistant(Uri apiOrigin) =>
    _SocketChannel(WebSocketChannel.connect(assistantSocketUri(apiOrigin)));

/// One item of the conversation, as Universal Exports sent it.
@immutable
class AssistantEntry {
  const AssistantEntry({
    required this.role,
    required this.text,
    required this.at,
    this.data,
  });

  /// `person`, `moneypenny`, `tools`, `error`, `proposal` or `view`.
  final String role;
  final String text;
  final double at;
  final Map<String, Object?>? data;

  factory AssistantEntry.fromJson(Map<String, Object?> json) => AssistantEntry(
    role: json['role'] as String? ?? 'error',
    text: json['text'] as String? ?? '',
    at: (json['at'] as num?)?.toDouble() ?? 0,
    data: (json['data'] as Map?)?.cast<String, Object?>(),
  );

  AssistantEntry withData(Map<String, Object?> data) =>
      AssistantEntry(role: role, text: text, at: at, data: data);

  /// The proposal or view ID this entry carries.
  String? get id => data?['id'] as String?;
}

/// A conversation with Moneypenny: what was said, whether she is answering, and the
/// person's messages, confirmations and form submissions on their way.
class AssistantController extends ChangeNotifier {
  AssistantController({
    required this.token,
    required AssistantChannel Function() connect,
  }) : _openChannel = connect;

  /// The person's access token, sent in the socket's first frame only.
  final String token;
  final AssistantChannel Function() _openChannel;

  AssistantChannel? _channel;
  StreamSubscription<String>? _subscription;
  String? conversationId;
  List<AssistantEntry> entries = const [];
  bool busy = false;
  bool connected = false;
  String? problem;

  /// Opens the socket and says hello, resuming the last conversation when there is one.
  void open() {
    if (_channel != null) return;
    final channel = _openChannel();
    _channel = channel;
    _subscription = channel.incoming.listen(
      _receive,
      onError: (Object _) => _closed('The connection to Moneypenny failed.'),
      onDone: () => _closed(null),
    );
    channel.send(
      jsonEncode({
        'type': 'hello',
        'token': token,
        'conversation': ?conversationId,
      }),
    );
  }

  void _closed(String? why) {
    _subscription?.cancel();
    _subscription = null;
    _channel = null;
    connected = false;
    busy = false;
    problem = why ?? problem;
    notifyListeners();
  }

  void _receive(String frame) {
    final Map<String, Object?> message;
    try {
      message = (jsonDecode(frame) as Map).cast<String, Object?>();
    } on Object {
      return;
    }
    switch (message['type']) {
      case 'ready':
        connected = true;
        problem = null;
        conversationId = message['conversation'] as String?;
        busy = message['busy'] == true;
        entries = [
          for (final entry in message['entries'] as List? ?? const [])
            AssistantEntry.fromJson((entry as Map).cast<String, Object?>()),
        ];
      case 'entry':
        entries = [
          ...entries,
          AssistantEntry.fromJson(
            (message['entry'] as Map).cast<String, Object?>(),
          ),
        ];
      case 'proposal':
        final proposal = (message['proposal'] as Map).cast<String, Object?>();
        entries = [
          for (final entry in entries)
            entry.role == 'proposal' && entry.id == proposal['id']
                ? entry.withData(proposal)
                : entry,
        ];
      case 'busy':
        busy = message['busy'] == true;
      case 'error':
        problem = message['message'] as String? ?? 'Moneypenny refused that.';
      default:
        return;
    }
    notifyListeners();
  }

  void _send(Map<String, Object?> frame) {
    final channel = _channel;
    if (channel == null) {
      problem = 'Not connected to Moneypenny.';
      notifyListeners();
      return;
    }
    problem = null;
    channel.send(jsonEncode(frame));
    notifyListeners();
  }

  /// Sends a message, with the app page the person is on (`view`, `operation_id`, …).
  void say(String text, {Map<String, String> page = const {}}) {
    if (text.trim().isEmpty) return;
    _send({'type': 'message', 'text': text, if (page.isNotEmpty) 'page': page});
  }

  void confirm(String proposalId) =>
      _send({'type': 'confirm', 'proposal': proposalId});

  void cancel(String proposalId) =>
      _send({'type': 'cancel', 'proposal': proposalId});

  void submit(String viewId, Map<String, Object?> values) =>
      _send({'type': 'submit', 'view': viewId, 'values': values});

  /// Starts a new conversation.
  void startOver() => _send({'type': 'new'});

  @override
  void dispose() {
    _subscription?.cancel();
    unawaited(_channel?.close());
    super.dispose();
  }
}
