import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import '../routes/socket_uri.dart';

/// A text channel to Universal Exports' attach socket; tests give a fake one.
abstract interface class AttachChannel {
  Stream<String> get incoming;
  void send(String frame);
  Future<void> close();
}

class _SocketChannel implements AttachChannel {
  _SocketChannel(this._socket) {
    // A failed connection also fails `ready`; [incoming] reports it.
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

/// `/v1/socket/attach` on the gateway's origin, as a WebSocket URL.
Uri attachSocketUri(Uri apiOrigin) => socketUri(apiOrigin, '/v1/socket/attach');

/// Opens the real socket.
AttachChannel connectAttach(Uri apiOrigin) =>
    _SocketChannel(WebSocketChannel.connect(attachSocketUri(apiOrigin)));

/// The Run a person attaches to.
@immutable
sealed class AttachTarget {
  const AttachTarget();

  /// What the hello frame names the Run by.
  Map<String, Object?> get hello;

  /// A line for the view's heading.
  String get label;
}

/// A charge's Run, by scope: `m`, `operation:<codename>` or `station:<id>`.
class ChargeTarget extends AttachTarget {
  const ChargeTarget(this.scope);

  final String scope;

  @override
  Map<String, Object?> get hello => {'charge': scope};

  @override
  String get label => 'charge $scope';
}

/// The live Run of a Mission.
class MissionTarget extends AttachTarget {
  const MissionTarget(this.operation, this.mission);

  final String operation;
  final String mission;

  @override
  Map<String, Object?> get hello => {
    'mission': {'operation': operation, 'mission': mission},
  };

  @override
  String get label => 'Mission $mission';
}

/// One thing the Run showed, as the Station sent it.
@immutable
class AttachFrame {
  const AttachFrame({
    required this.seq,
    required this.at,
    required this.kind,
    required this.data,
  });

  factory AttachFrame.fromJson(Map<String, Object?> json) => AttachFrame(
    seq: (json['seq'] as num?)?.toInt() ?? 0,
    at: (json['at'] as num?)?.toDouble() ?? 0,
    kind: json['kind'] as String? ?? '',
    data: (json['data'] as Map?)?.cast<String, Object?>() ?? const {},
  );

  /// Its place in the Run's history; `0` for one that is for this client alone.
  final int seq;
  final double at;

  /// `event`, `prompt`, `turn`, `permission`, `answered`, `attached`, `detached`,
  /// `interrupted`, `ready`, `closed` or `error`; a person's view passes over others.
  final String kind;
  final Map<String, Object?> data;

  String text(String key) {
    final value = data[key];
    return value == null ? '' : '$value';
  }
}

/// One option of a permission request.
@immutable
class PermissionOption {
  const PermissionOption(this.id, this.label);

  final String id;
  final String label;
}

/// Where the attachment stands.
enum AttachPhase {
  /// Opening the socket and saying hello.
  connecting,

  /// The Run's history is being replayed.
  replaying,

  /// The history is replayed; frames are live.
  live,

  /// The Run closed: nothing more will come.
  ended,

  /// The socket ended while the Run had not closed; [Attachment.reconnect] resumes.
  dropped,

  /// The gateway refused the attachment or it was withdrawn; trying again would not help.
  refused,

  /// The person detached.
  detached,
}

/// One person's attachment to a Run: its history, then its live frames, with what the person
/// may send. It keeps the last `seq` it saw, so a reconnect asks only for what it missed.
class Attachment extends ChangeNotifier {
  Attachment({
    required this.token,
    required this.target,
    required AttachChannel Function() connect,
  }) : _open = connect;

  /// The person's access token, sent in the hello frame only.
  final String token;
  final AttachTarget target;
  final AttachChannel Function() _open;

  AttachChannel? _channel;
  StreamSubscription<String>? _subscription;
  bool _disposed = false;

  AttachPhase phase = AttachPhase.connecting;

  /// Everything the Run showed that this view draws, oldest first.
  final List<AttachFrame> frames = [];

  /// The last frame this attachment has, so a reconnect asks for those after it.
  int lastSeq = 0;

  /// The client ID the gateway gave this attachment.
  String? client;

  /// Whether a turn is under way, as the Run last said.
  bool busy = false;

  /// What the gateway last refused, in its words; cleared by the next thing that works.
  String? refusal;

  /// The answer to each permission request, by request ID, once one came.
  final Map<String, AttachFrame> answers = {};

  static const _drawn = {
    'event',
    'prompt',
    'turn',
    'permission',
    'answered',
    'attached',
    'detached',
    'interrupted',
    'ready',
    'closed',
    'error',
  };

  /// Opens the socket and says hello, asking for what follows [lastSeq].
  void connect() {
    if (_disposed) return;
    unawaited(_close());
    phase = AttachPhase.connecting;
    refusal = null;
    notifyListeners();
    final AttachChannel channel;
    try {
      channel = _open();
    } on Object {
      _dropped();
      return;
    }
    _channel = channel;
    _subscription = channel.incoming.listen(
      (message) => _receive(channel, message),
      onError: (Object _) => _ended(channel),
      onDone: () => _ended(channel),
      cancelOnError: true,
    );
    channel.send(
      jsonEncode({
        'type': 'hello',
        'token': token,
        ...target.hello,
        if (lastSeq > 0) 'after': lastSeq,
      }),
    );
  }

  /// Opens the socket again from the last frame this attachment saw.
  void reconnect() => connect();

  void _receive(AttachChannel channel, String message) {
    if (_disposed || !identical(channel, _channel)) return;
    final Object? decoded;
    try {
      decoded = jsonDecode(message);
    } on FormatException {
      return;
    }
    if (decoded is! Map) return;
    final body = decoded.cast<String, Object?>();
    switch (body['type']) {
      case 'attached':
        client = body['client'] as String?;
        phase = AttachPhase.replaying;
      case 'frame':
        final raw = body['frame'];
        if (raw is Map) _frame(AttachFrame.fromJson(raw.cast()));
      case 'error':
        refusal = body['message'] as String? ?? 'The gateway refused.';
        // A refusal before the Run is attached to, or of the right to be, is final.
        final code = body['code'];
        if (client == null ||
            code == 'forbidden' ||
            code == 'unauthorized' ||
            code == 'not_found') {
          phase = AttachPhase.refused;
        }
    }
    notifyListeners();
  }

  void _frame(AttachFrame frame) {
    if (frame.seq > 0) {
      if (frame.seq <= lastSeq) return;
      lastSeq = frame.seq;
    }
    switch (frame.kind) {
      case 'ready':
        busy = frame.data['busy'] == true;
        phase = AttachPhase.live;
      case 'turn':
        busy = frame.data['state'] == 'begun';
      case 'answered':
        answers[frame.text('request')] ??= frame;
      case 'closed':
        phase = AttachPhase.ended;
        busy = false;
      case 'error':
        refusal = frame.text('reason');
    }
    if (_drawn.contains(frame.kind)) frames.add(frame);
  }

  void _ended(AttachChannel channel) {
    if (_disposed || !identical(channel, _channel)) return;
    _channel = null;
    unawaited(_subscription?.cancel());
    _subscription = null;
    if (phase == AttachPhase.ended ||
        phase == AttachPhase.refused ||
        phase == AttachPhase.detached) {
      notifyListeners();
      return;
    }
    _dropped();
  }

  void _dropped() {
    phase = AttachPhase.dropped;
    busy = false;
    notifyListeners();
  }

  bool get _canSend =>
      _channel != null &&
      (phase == AttachPhase.replaying || phase == AttachPhase.live);

  /// Whether the person may type and answer now.
  bool get attached => _canSend;

  /// Sends [text] to the Run as this person's prompt.
  void prompt(String text) {
    if (!_canSend || text.trim().isEmpty) return;
    _channel!.send(jsonEncode({'type': 'prompt', 'text': text}));
  }

  /// Answers permission [request] with [option]; the Run takes the first answer.
  void answer(String request, String option) {
    if (!_canSend || answers.containsKey(request)) return;
    final id = int.tryParse(request);
    _channel!.send(
      jsonEncode({
        'type': 'answer',
        'request': id ?? request,
        'option': option,
      }),
    );
  }

  /// Interrupts the turn under way.
  void interrupt() {
    if (!_canSend) return;
    _channel!.send(jsonEncode({'type': 'interrupt'}));
  }

  /// Leaves the Run, which goes on.
  Future<void> detach() async {
    if (phase == AttachPhase.detached) return;
    final channel = _channel;
    if (channel != null && _canSend) {
      try {
        channel.send(jsonEncode({'type': 'detach'}));
      } on Object {
        // The socket is gone already; the gateway lets the attachment go.
      }
    }
    phase = AttachPhase.detached;
    await _close();
    if (!_disposed) notifyListeners();
  }

  Future<void> _close() async {
    final subscription = _subscription;
    final channel = _channel;
    _subscription = null;
    _channel = null;
    // Both start now: a socket that is slow to cancel is still told to close.
    await Future.wait([?subscription?.cancel(), ?channel?.close()]);
  }

  @override
  void dispose() {
    if (!_disposed) {
      final channel = _channel;
      if (channel != null && _canSend) {
        try {
          channel.send(jsonEncode({'type': 'detach'}));
        } on Object {
          // As in detach.
        }
      }
      _disposed = true;
      unawaited(_close());
    }
    super.dispose();
  }

  /// The options of a `permission` frame, as `{id, label}` entries.
  static List<PermissionOption> optionsOf(AttachFrame frame) => [
    for (final option in frame.data['options'] as List<Object?>? ?? const [])
      if (option is Map)
        PermissionOption(
          '${option['id'] ?? ''}',
          '${option['label'] ?? option['id'] ?? ''}',
        ),
  ];
}
