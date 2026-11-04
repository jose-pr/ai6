import 'dart:async';
import 'dart:convert';

import 'package:material_ui/material_ui.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import '../routes/oo_route.dart';
import '../routes/socket_uri.dart';
import '../settings/oo_config.dart' show ChatSettings;
import 'assistant_view.dart' show ViewLimits;
import 'images.dart' show OutgoingImage;

/// A text channel to Universal Exports' Moneypenny socket; tests give a fake one.
abstract interface class AssistantChannel {
  Stream<String> get incoming;
  void send(String frame);
  Future<void> close();
}

class _SocketChannel implements AssistantChannel {
  _SocketChannel(this._socket) {
    // A failed connection also fails `ready`; [incoming] reports it, and left alone this
    // future would be an uncaught error.
    unawaited(_socket.ready.catchError((Object _) {}));
  }

  final WebSocketChannel _socket;

  @override
  Stream<String> get incoming =>
      _socket.stream.where((frame) => frame is String).cast();

  @override
  void send(String frame) => _socket.sink.add(frame);

  @override
  Future<void> close() async {
    try {
      await _socket.sink.close();
    } on Object {
      // Closing a socket that never opened has nothing to report.
    }
  }
}

/// `/v1/user/moneypenny` on the gateway's origin, as a WebSocket URL.
Uri assistantSocketUri(Uri apiOrigin) =>
    socketUri(apiOrigin, '/v1/user/moneypenny');

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

  /// `person`, `moneypenny`, `tools`, `error`, `proposal`, `view`, `navigation` or
  /// `notice`.
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

  /// The proposal, view or navigation ID this entry carries.
  String? get id => data?['id'] as String?;
}

/// A conversation with Moneypenny: what was said, whether she is answering, and the
/// person's messages, confirmations and form submissions on their way.
class AssistantController extends ChangeNotifier {
  AssistantController({
    required this.token,
    required AssistantChannel Function() connect,
    this.connectTimeout = ChatSettings.defaultConnectTimeout,
    this.retryDelay = ChatSettings.defaultRetryDelay,
    this.retryMaxDelay = ChatSettings.defaultRetryMaxDelay,
    this._lastConversation,
    this.onConversation,
  }) : _openChannel = connect;

  /// The person's access token, sent in the socket's first frame only.
  final String token;
  final AssistantChannel Function() _openChannel;

  /// How long the socket may take to say it is ready before the attempt counts as failed.
  final Duration connectTimeout;
  Timer? _connecting;

  /// The wait before the first reconnect after a socket fails; each next one doubles, up to
  /// [retryMaxDelay].
  final Duration retryDelay;

  /// The longest wait between reconnects.
  final Duration retryMaxDelay;
  Timer? _retry;
  late Duration _nextRetry = retryDelay;

  /// Universal Exports said this sign-in is no good, which trying again would not change.
  bool _refused = false;

  /// The conversation a new sign-in goes back to, until the first hello has asked for it.
  /// Universal Exports checks it is the person's, and opens a new one when it is not or is gone.
  Future<String?>? _lastConversation;

  /// Told the ID of each conversation Universal Exports opens here.
  final void Function(String id)? onConversation;

  AssistantChannel? _channel;
  StreamSubscription<String>? _subscription;
  String? conversationId;
  List<AssistantEntry> entries = const [];

  /// How many older entries Universal Exports kept but did not send.
  int earlier = 0;

  /// The bounds Universal Exports holds her views to, as it sent them.
  ViewLimits viewLimits = const ViewLimits();
  bool busy = false;

  /// A password Universal Exports made for an account the person confirmed resetting, until the
  /// panel has shown it: it is never an entry of the conversation and never reaches Moneypenny.
  String? _reveal;

  /// Takes the waiting password, once.
  String? takeReveal() {
    final secret = _reveal;
    _reveal = null;
    return secret;
  }

  bool get hasReveal => _reveal != null;

  /// The person asked to stop the answer, and it has not stopped yet.
  bool stopping = false;
  bool connected = false;
  String? problem;

  /// A socket is open or opening and has not said it is ready: neither connected nor failed.
  bool get connecting => _channel != null && !connected;

  /// The socket failed and it will be tried again, by itself, until it answers or the person says
  /// to stop.
  bool reconnecting = false;

  final _arrivals = StreamController<AssistantEntry>.broadcast(sync: true);

  /// Each entry as it arrives while the conversation is open; not those a reconnect sends
  /// again.
  Stream<AssistantEntry> get arrivals => _arrivals.stream;

  /// A message asked before the conversation was ready, sent when it is.
  ({String text, OoRoute? route})? _pending;

  /// Opens the socket and says hello, resuming the last conversation when there is one.
  void open() {
    if (_channel != null) return;
    problem = null;
    _attempt();
  }

  void _attempt() {
    _retry?.cancel();
    _retry = null;
    final AssistantChannel channel;
    try {
      channel = _openChannel();
    } on Object {
      problem = 'The connection to Moneypenny failed.';
      _retryLater();
      return;
    }
    _channel = channel;
    _subscription = channel.incoming.listen(
      _receive,
      onError: (Object _) => _closed('The connection to Moneypenny failed.'),
      onDone: () => _closed(null),
    );
    _connecting = Timer(connectTimeout, () {
      unawaited(channel.close());
      _closed('Moneypenny did not answer.');
    });
    final last = _lastConversation;
    if (last == null) {
      _hello(channel);
      return;
    }
    _lastConversation = null;
    unawaited(
      last
          .then<void>((stored) => conversationId ??= stored)
          .catchError((Object _) {})
          .whenComplete(() {
            if (identical(_channel, channel)) _hello(channel);
          }),
    );
  }

  void _hello(AssistantChannel channel) {
    channel.send(
      jsonEncode({
        'type': 'hello',
        'token': token,
        'conversation': ?conversationId,
      }),
    );
  }

  /// Tries the socket again now, whatever stopped it; a message asked meanwhile is still sent.
  void retry() {
    _refused = false;
    _nextRetry = retryDelay;
    open();
    notifyListeners();
  }

  /// Stops trying the socket again, until the person asks.
  void stopRetrying() {
    _retry?.cancel();
    _retry = null;
    reconnecting = false;
    final channel = _channel;
    if (channel != null && !connected) {
      _connecting?.cancel();
      _connecting = null;
      _subscription?.cancel();
      _subscription = null;
      _channel = null;
      unawaited(channel.close());
      problem ??= 'Not connected to Moneypenny.';
    }
    notifyListeners();
  }

  /// Waits, then tries again, unless the person or the sign-in said not to.
  void _retryLater() {
    reconnecting = !_refused;
    if (_refused) return;
    final wait = _nextRetry > retryMaxDelay ? retryMaxDelay : _nextRetry;
    _nextRetry = wait * 2;
    _retry = Timer(wait, () {
      _attempt();
      notifyListeners();
    });
  }

  void _closed(String? why) {
    _connecting?.cancel();
    _connecting = null;
    _subscription?.cancel();
    _subscription = null;
    _channel = null;
    connected = false;
    busy = false;
    final stopped = stopping;
    stopping = false;
    problem = why ?? problem ?? 'The connection to Moneypenny closed.';
    if (stopped) {
      reconnecting = false;
    } else {
      _retryLater();
    }
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
        _connecting?.cancel();
        _connecting = null;
        connected = true;
        problem = null;
        reconnecting = false;
        _nextRetry = retryDelay;
        conversationId = message['conversation'] as String?;
        if (conversationId case final id?) onConversation?.call(id);
        busy = message['busy'] == true;
        stopping = false;
        earlier = (message['earlier'] as num?)?.toInt() ?? 0;
        viewLimits = ViewLimits.fromJson(message['view_limits']);
        entries = [
          for (final entry in message['entries'] as List? ?? const [])
            AssistantEntry.fromJson((entry as Map).cast<String, Object?>()),
        ];
        if (_pending case final pending?) {
          _pending = null;
          _channel?.send(
            jsonEncode({
              'type': 'message',
              'text': pending.text,
              if (pending.route case final route?) 'route': route.toJson(),
            }),
          );
        }
      case 'entry':
        final entry = AssistantEntry.fromJson(
          (message['entry'] as Map).cast<String, Object?>(),
        );
        entries = [...entries, entry];
        notifyListeners();
        _arrivals.add(entry);
        return;
      case 'proposal':
        final proposal = (message['proposal'] as Map).cast<String, Object?>();
        entries = [
          for (final entry in entries)
            entry.role == 'proposal' && entry.id == proposal['id']
                ? entry.withData(proposal)
                : entry,
        ];
      case 'reveal':
        _reveal = message['secret'] as String?;
      case 'busy':
        busy = message['busy'] == true;
        if (!busy) stopping = false;
      case 'interrupt':
        // `stopping: false` means nothing was being answered any more.
        stopping = busy && message['stopping'] == true;
      case 'error':
        problem = message['message'] as String? ?? 'Moneypenny refused that.';
        _refused = _refused || message['code'] == 'unauthorized';
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

  /// Sends a message, with the [route] of the page the person is on and the [images] they
  /// added to it.
  void say(
    String text, {
    OoRoute? route,
    List<OutgoingImage> images = const [],
  }) {
    if (text.trim().isEmpty) return;
    _send({
      'type': 'message',
      'text': text,
      if (route != null) 'route': route.toJson(),
      if (images.isNotEmpty)
        'images': [for (final image in images) image.toJson()],
    });
  }

  /// Sends a message now when the conversation is ready, else opens it and sends the
  /// message once Universal Exports says it is ready.
  void ask(String text, {OoRoute? route}) {
    if (text.trim().isEmpty) return;
    if (connected) {
      say(text, route: route);
      return;
    }
    _pending = (text: text, route: route);
    open();
  }

  void confirm(String proposalId) =>
      _send({'type': 'confirm', 'proposal': proposalId});

  void cancel(String proposalId) =>
      _send({'type': 'cancel', 'proposal': proposalId});

  void submit(String viewId, Map<String, Object?> values) =>
      _send({'type': 'submit', 'view': viewId, 'values': values});

  /// Stops the answer Moneypenny is giving; the conversation shows when it has stopped.
  void interrupt() {
    if (!busy || stopping) return;
    stopping = _channel != null;
    _send({'type': 'interrupt'});
  }

  /// Starts a new conversation.
  void startOver() => _send({'type': 'new'});

  @override
  void dispose() {
    _connecting?.cancel();
    _retry?.cancel();
    _subscription?.cancel();
    unawaited(_arrivals.close());
    final channel = _channel;
    _channel = null;
    unawaited(channel?.close());
    super.dispose();
  }
}
