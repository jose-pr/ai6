import 'dart:async';
import 'dart:collection';

import 'package:coqui_acp/coqui_acp.dart';

import 'connection.dart';
import 'limits.dart';
import 'notifier.dart';
import 'rendering.dart';
import 'session.dart';

/// A conversation that came to need the person, and what for.
typedef Attention = ({ChatSession session, AttentionKind kind});

/// The conversations of one chat window, which is the one that is chosen, and where the requests of every connection are put.
/// Transport, verified peers and persistence are the host's.
class ChatStore extends ChatNotifier implements ChatRequests {
  ChatStore({
    this.limits = const ChatLimits(),
    this.recorder,
    this.hostNoticesKey,
    DateTime Function()? clock,
  }) : _clock = clock;

  final ChatLimits limits;

  /// Told the kind and shape of each update any conversation here applies, never its text.
  final UpdateRecorder? recorder;

  /// See [ChatSession.hostNoticesKey].
  final String? hostNoticesKey;

  final DateTime Function()? _clock;
  final List<ChatSession> _sessions = [];
  final _attention = StreamController<Attention>.broadcast();
  int _nextId = 0;
  String? _selectedId;

  List<ChatSession> get sessions => UnmodifiableListView(_sessions);
  String? get selectedId => _selectedId;

  ChatSession? get selected {
    for (final session in _sessions) {
      if (session.id == _selectedId) return session;
    }
    return null;
  }

  /// Each time a conversation comes to need the person: a request or a question put to them, or a turn ended. Whether that reaches
  /// them outside the window is the host's to decide.
  Stream<Attention> get attention => _attention.stream;

  /// How many conversations wait for a person's answer.
  int get waiting => _sessions.where((session) => session.needsYou).length;

  /// Adds a conversation whose agent opened [sessionId], and chooses it. It has no connection until [ChatSession.attach].
  ChatSession add({
    required String title,
    required String agent,
    String? peerId,
    String? sessionId,
    String? cwd,
  }) {
    final session = ChatSession(
      id: 'local-${++_nextId}',
      title: title,
      agent: agent,
      peerId: peerId,
      sessionId: sessionId,
      cwd: cwd,
      limits: limits,
      recorder: recorder,
      hostNoticesKey: hostNoticesKey,
      clock: _clock,
    );
    session.onChange = notifyListeners;
    // Marked at once, so the listeners told of the same change already see it.
    session.onAttention = (kind) {
      if (kind == AttentionKind.finished && session.id != _selectedId) {
        session.unread = true;
      }
      if (!_attention.isClosed) {
        _attention.add((session: session, kind: kind));
      }
    };
    _sessions.insert(0, session);
    _selectedId = session.id;
    notifyListeners();
    return session;
  }

  /// Chooses a conversation, which is then no longer unread.
  void select(String id) {
    final chosen = _sessions.where((session) => session.id == id).firstOrNull;
    if (chosen == null) return;
    _selectedId = id;
    chosen.unread = false;
    notifyListeners();
  }

  ChatSession? _of(String sessionId, ChatConnection from) {
    for (final session in _sessions) {
      if (session.sessionId == sessionId &&
          identical(session.connection, from)) {
        return session;
      }
    }
    return null;
  }

  /// Puts a permission request to the person, in the conversation [from] serves under the request's session id. A request about no
  /// conversation here is refused.
  @override
  Future<AcpDecision> askPermission(
    AcpPermissionRequest request,
    ChatConnection from,
  ) {
    final target = _of(request.sessionId, from);
    if (target == null) {
      return Future.value(const AcpDeny('not a session here'));
    }
    return target.askPermission(request);
  }

  /// Puts an agent's question to the person, in the conversation [from] serves under the request's session id. A question about
  /// no conversation here is cancelled.
  @override
  Future<AcpElicitationAnswer> askQuestion(
    AcpElicitation request,
    ChatConnection from,
  ) {
    final target = _of(request.sessionId, from);
    if (target == null) return Future.value(const AcpElicitCancel());
    return target.askQuestion(request);
  }

  /// Takes a conversation out of the list. The agent still has it, and its connection stays with whoever else uses it. What the
  /// conversation still asked of the person is withdrawn.
  void forget(ChatSession session) {
    session.detach();
    session.onChange = null;
    session.onAttention = null;
    _sessions.remove(session);
    if (_selectedId == session.id) {
      _selectedId = _sessions.isEmpty ? null : _sessions.first.id;
    }
    notifyListeners();
  }

  /// Ends a conversation here: a request still open is answered cancelled, and its connection ends once no other conversation here
  /// uses it.
  Future<void> close(ChatSession session) async {
    final connection = session.connection;
    session.close();
    forget(session);
    if (connection != null &&
        !_sessions.any((other) => identical(other.connection, connection))) {
      await connection.close();
    }
  }

  @override
  void dispose() {
    for (final session in [..._sessions]) {
      session.onChange = null;
      session.onAttention = null;
      session.dispose();
    }
    _sessions.clear();
    unawaited(_attention.close());
    super.dispose();
  }
}
