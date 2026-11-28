import 'dart:async';
import 'dart:convert';

import 'package:coqui_acp/coqui_acp.dart';

import 'connection.dart';
import 'content.dart' show ToolDiff;
import 'limits.dart';
import 'link.dart';
import 'session.dart';

// EXPERIMENTAL. A conversation shown in more than one place, such as a window popped out of the one that holds it: one copy owns it
// (it talks to the agent, queues, settles requests), and each view shows a read-only copy of it that follows. The wire is
// provisional and this API may change or go. It is exported by `package:coqui_acp_chat/experimental.dart` only.
//
// Every message is a JSON map with a `t`. A view sends `attach`, `send`, `cancel`, `permission`, `question` and `detach`; the
// owner sends `state` (the whole conversation as `replay` makes it, and what waits for an answer, each time it changes, at most
// once per interval), `ack` (to a `send`), `refused` and `ended`.

/// The wire version, in the `attach` and in every `state`.
const int chatViewWireVersion = 1;

Map<String, Object?> _map(Object? value) => value is Map
    ? value.cast<String, Object?>()
    : throw const FormatException('Not a map');

List<Object?> _list(Object? value) =>
    value is List ? value : throw const FormatException('Not a list');

String? _string(Object? value) => value is String ? value : null;

String _need(Object? value) =>
    value is String ? value : throw const FormatException('Not a string');

int _int(Object? value) =>
    value is int ? value : throw const FormatException('Not an integer');

DateTime? _time(Object? value) =>
    value is String ? DateTime.tryParse(value) : null;

/// A request for permission as a view is told of it.
final class SharedPermission {
  const SharedPermission({
    required this.id,
    required this.title,
    this.kind,
    this.detail,
    this.reason,
    this.diff,
    this.options = const [],
    this.forPerson = false,
    this.bypassPassed = false,
    this.toolCallId,
    this.askedAt,
  });

  /// The owner's number for it, which an answer names.
  final int id;
  final String title;
  final String? kind;
  final String? detail, reason;
  final ToolDiff? diff;
  final List<AcpPermissionOption> options;
  final bool forPerson, bypassPassed;
  final String? toolCallId;
  final DateTime? askedAt;

  factory SharedPermission.of(PermissionQuestion q) => SharedPermission(
    id: q.id,
    title: q.title,
    kind: q.kind,
    detail: q.detail,
    reason: q.reason,
    diff: q.diff,
    options: q.options,
    forPerson: q.forPerson,
    bypassPassed: q.bypassPassed,
    toolCallId: q.toolCallId,
    askedAt: q.askedAt,
  );

  Map<String, Object?> toJson() => {
    'id': id,
    'title': title,
    'kind': ?kind,
    'detail': ?detail,
    'reason': ?reason,
    if (diff case final change?)
      'diff': {
        'path': change.path,
        'oldText': ?change.oldText,
        'newText': change.newText,
      },
    'options': [
      for (final o in options)
        {'id': o.optionId, 'name': o.name, 'kind': o.kind},
    ],
    'forPerson': forPerson,
    'bypassPassed': bypassPassed,
    'toolCallId': ?toolCallId,
    'askedAt': ?askedAt?.toUtc().toIso8601String(),
  };

  factory SharedPermission.fromJson(Object? json) {
    final m = _map(json);
    return SharedPermission(
      id: _int(m['id']),
      title: _string(m['title']) ?? 'A tool',
      kind: _string(m['kind']),
      detail: _string(m['detail']),
      reason: _string(m['reason']),
      diff: switch (m['diff']) {
        final Map<dynamic, dynamic> change when change['newText'] is String =>
          ToolDiff(
            path: _string(change['path']) ?? '',
            oldText: _string(change['oldText']),
            newText: change['newText'] as String,
          ),
        _ => null,
      },
      options: [
        for (final o in _list(m['options'] ?? const []))
          AcpPermissionOption(
            optionId: _need(_map(o)['id']),
            name: _string(_map(o)['name']) ?? '',
            kind: _string(_map(o)['kind']) ?? '',
          ),
      ],
      forPerson: m['forPerson'] == true,
      bypassPassed: m['bypassPassed'] == true,
      toolCallId: _string(m['toolCallId']),
      askedAt: _time(m['askedAt']),
    );
  }
}

/// An agent's question as a view is told of it.
final class SharedQuestion {
  const SharedQuestion({required this.id, required this.request, this.askedAt});
  final int id;
  final AcpElicitation request;
  final DateTime? askedAt;

  Map<String, Object?> toJson() => {
    'id': id,
    'request': elicitationToJson(request),
    'askedAt': ?askedAt?.toUtc().toIso8601String(),
  };

  factory SharedQuestion.fromJson(Object? json) {
    final m = _map(json);
    return SharedQuestion(
      id: _int(m['id']),
      request: elicitationFromJson(m['request']),
      askedAt: _time(m['askedAt']),
    );
  }
}

/// How a request ended, for the views that still show it waiting.
final class SharedOutcome {
  const SharedOutcome({
    required this.id,
    required this.how,
    this.by,
    this.answer,
  });

  final int id;

  /// `answered`, `cancelled` or `withdrawn` ([Settlement] by name).
  final String how;

  /// The client that answered, [bypassClient] for Bypass.
  final String? by;

  /// What the agent was told: a decision (`decisionToJson`) or an answer (`answerToJson`).
  final Map<String, Object?>? answer;

  Map<String, Object?> toJson() => {
    'id': id,
    'how': how,
    'by': ?by,
    'answer': ?answer,
  };

  factory SharedOutcome.fromJson(Object? json) {
    final m = _map(json);
    return SharedOutcome(
      id: _int(m['id']),
      how: _string(m['how']) ?? 'cancelled',
      by: _string(m['by']),
      answer: m['answer'] is Map ? _map(m['answer']) : null,
    );
  }
}

/// A queued message as a view is told of it.
typedef SharedQueued = ({String text, String? from});

/// Everything a view shows of a conversation, as JSON-ready values. [ChatSnapshot.of] reads it from a [ChatSession]; an app that
/// keeps its conversation some other way builds one from what it holds.
final class ChatSnapshot {
  const ChatSnapshot({
    this.sessionId,
    this.agent = '',
    this.cwd,
    required this.status,
    this.replay = const [],
    this.queue = const [],
    this.queueHeld,
    this.clients = const [],
    this.permissions = const [],
    this.questions = const [],
    this.settledPermissions = const [],
    this.settledQuestions = const [],
  });

  final String? sessionId, agent, cwd;

  /// `disconnected`, `connected` or `working` ([ChatStatus] by name).
  final String status;

  /// The updates that make the conversation again: what [ChatSessionHub.replay] gives, as their `raw` maps.
  final List<Map<String, Object?>> replay;
  final List<SharedQueued> queue;
  final String? queueHeld;

  /// The clients attached, with their names.
  final List<({String id, String? name})> clients;
  final List<SharedPermission> permissions;
  final List<SharedQuestion> questions;

  /// The requests most recently settled, so a view that still shows one waiting learns how it ended.
  final List<SharedOutcome> settledPermissions, settledQuestions;

  /// What [session] holds now.
  factory ChatSnapshot.of(ChatSession session) => ChatSnapshot(
    sessionId: session.sessionId,
    agent: session.agent,
    cwd: session.cwd,
    status: session.status.name,
    replay: [for (final update in session.replay()) update.raw],
    queue: [for (final m in session.queue) (text: m.text, from: m.from)],
    queueHeld: session.queueHeld,
    clients: [for (final c in session.clients) (id: c.id, name: c.name)],
    permissions: [
      for (final q in session.pendingPermissions) SharedPermission.of(q),
    ],
    questions: [
      for (final q in session.pendingQuestions)
        SharedQuestion(id: q.id, request: q.request, askedAt: q.askedAt),
    ],
    settledPermissions: [
      for (final q in session.settledPermissions)
        if (q.outcome case final o?)
          SharedOutcome(
            id: q.id,
            how: o.how.name,
            by: o.by,
            answer: o.answer == null ? null : decisionToJson(o.answer!),
          ),
    ],
    settledQuestions: [
      for (final q in session.settledQuestions)
        if (q.outcome case final o?)
          SharedOutcome(
            id: q.id,
            how: o.how.name,
            by: o.by,
            answer: o.answer == null ? null : answerToJson(o.answer!),
          ),
    ],
  );

  Map<String, Object?> toJson() => {
    't': 'state',
    'v': chatViewWireVersion,
    'sessionId': ?sessionId,
    'agent': agent,
    'cwd': ?cwd,
    'status': status,
    'replay': replay,
    'queue': [
      for (final q in queue) {'text': q.text, 'from': ?q.from},
    ],
    'queueHeld': ?queueHeld,
    'clients': [
      for (final c in clients) {'id': c.id, 'name': ?c.name},
    ],
    'permissions': [for (final p in permissions) p.toJson()],
    'questions': [for (final q in questions) q.toJson()],
    'settledPermissions': [for (final o in settledPermissions) o.toJson()],
    'settledQuestions': [for (final o in settledQuestions) o.toJson()],
  };

  factory ChatSnapshot.fromJson(Object? json) {
    final m = _map(json);
    if (m['t'] != 'state') throw const FormatException('Not a state');
    if (m['v'] != chatViewWireVersion) {
      throw FormatException(
        'Wire version ${m['v']} is not $chatViewWireVersion',
      );
    }
    return ChatSnapshot(
      sessionId: _string(m['sessionId']),
      agent: _string(m['agent']) ?? '',
      cwd: _string(m['cwd']),
      status: _string(m['status']) ?? 'disconnected',
      replay: [for (final u in _list(m['replay'] ?? const [])) _map(u)],
      queue: [
        for (final q in _list(m['queue'] ?? const []))
          (text: _need(_map(q)['text']), from: _string(_map(q)['from'])),
      ],
      queueHeld: _string(m['queueHeld']),
      clients: [
        for (final c in _list(m['clients'] ?? const []))
          (id: _need(_map(c)['id']), name: _string(_map(c)['name'])),
      ],
      permissions: [
        for (final p in _list(m['permissions'] ?? const []))
          SharedPermission.fromJson(p),
      ],
      questions: [
        for (final q in _list(m['questions'] ?? const []))
          SharedQuestion.fromJson(q),
      ],
      settledPermissions: [
        for (final o in _list(m['settledPermissions'] ?? const []))
          SharedOutcome.fromJson(o),
      ],
      settledQuestions: [
        for (final o in _list(m['settledQuestions'] ?? const []))
          SharedOutcome.fromJson(o),
      ],
    );
  }
}

/// A permission decision as JSON.
Map<String, Object?> decisionToJson(AcpDecision decision) => switch (decision) {
  AcpAllow() => {'d': 'allow'},
  AcpDeny(:final reason) => {'d': 'deny', 'reason': reason},
  AcpCancel() => {'d': 'cancel'},
  AcpSelect(:final optionId) => {'d': 'select', 'option': optionId},
};

AcpDecision decisionFromJson(Map<String, Object?> json) => switch (json['d']) {
  'allow' => const AcpAllow(),
  'deny' => AcpDeny(_string(json['reason']) ?? 'the person refused'),
  'select' when json['option'] is String => AcpSelect(json['option'] as String),
  _ => const AcpCancel(),
};

/// A question's answer as JSON.
Map<String, Object?> answerToJson(AcpElicitationAnswer answer) =>
    switch (answer) {
      AcpElicitAccept(:final content) => {'a': 'accept', 'content': content},
      AcpElicitDecline() => {'a': 'decline'},
      AcpElicitCancel() => {'a': 'cancel'},
    };

AcpElicitationAnswer answerFromJson(Map<String, Object?> json) =>
    switch (json['a']) {
      'accept' => AcpElicitAccept(
        json['content'] is Map ? _map(json['content']) : const {},
      ),
      'decline' => const AcpElicitDecline(),
      _ => const AcpElicitCancel(),
    };

/// An agent's question as JSON.
Map<String, Object?> elicitationToJson(AcpElicitation request) => {
  'sessionId': request.sessionId,
  'message': request.message,
  'title': ?request.title,
  'description': ?request.description,
  'toolCallId': ?request.toolCallId,
  'fields': [for (final f in request.fields) _fieldToJson(f)],
};

AcpElicitation elicitationFromJson(Object? json) {
  final m = _map(json);
  return AcpElicitation(
    sessionId: _string(m['sessionId']) ?? '',
    message: _string(m['message']) ?? '',
    title: _string(m['title']),
    description: _string(m['description']),
    toolCallId: _string(m['toolCallId']),
    fields: [for (final f in _list(m['fields'] ?? const [])) _fieldFromJson(f)],
  );
}

List<Map<String, Object?>> _choicesToJson(List<AcpChoice> options) => [
  for (final o in options)
    {'value': o.value, 'title': o.title, 'description': ?o.description},
];

List<AcpChoice> _choicesFromJson(Object? json) => [
  for (final o in _list(json ?? const []))
    AcpChoice(
      _need(_map(o)['value']),
      _string(_map(o)['title']) ?? _need(_map(o)['value']),
      description: _string(_map(o)['description']),
    ),
];

Map<String, Object?> _fieldToJson(AcpField field) {
  final base = <String, Object?>{
    'name': field.name,
    'title': ?field.title,
    'description': ?field.description,
    'required': field.required,
  };
  return switch (field) {
    final AcpTextField f => {
      ...base,
      'type': 'text',
      'minLength': ?f.minLength,
      'maxLength': ?f.maxLength,
      'format': ?f.format,
      'default': ?f.defaultValue,
    },
    final AcpChoiceField f => {
      ...base,
      'type': 'choice',
      'options': _choicesToJson(f.options),
      'default': ?f.defaultValue,
    },
    final AcpChoicesField f => {
      ...base,
      'type': 'choices',
      'options': _choicesToJson(f.options),
      'minItems': ?f.minItems,
      'maxItems': ?f.maxItems,
      'default': ?f.defaultValue,
    },
    final AcpNumberField f => {
      ...base,
      'type': 'number',
      'integer': f.integer,
      'minimum': ?f.minimum,
      'maximum': ?f.maximum,
      'default': ?f.defaultValue,
    },
    final AcpBooleanField f => {
      ...base,
      'type': 'boolean',
      'default': ?f.defaultValue,
    },
    final AcpUnsupportedField f => {
      ...base,
      'type': 'unsupported',
      'schemaType': f.type,
    },
  };
}

AcpField _fieldFromJson(Object? json) {
  final m = _map(json);
  final name = _need(m['name']);
  final title = _string(m['title']);
  final description = _string(m['description']);
  final required = m['required'] == true;
  return switch (m['type']) {
    'text' => AcpTextField(
      name,
      title: title,
      description: description,
      required: required,
      minLength: m['minLength'] as int?,
      maxLength: m['maxLength'] as int?,
      format: _string(m['format']),
      defaultValue: _string(m['default']),
    ),
    'choice' => AcpChoiceField(
      name,
      _choicesFromJson(m['options']),
      title: title,
      description: description,
      required: required,
      defaultValue: _string(m['default']),
    ),
    'choices' => AcpChoicesField(
      name,
      _choicesFromJson(m['options']),
      title: title,
      description: description,
      required: required,
      minItems: m['minItems'] as int?,
      maxItems: m['maxItems'] as int?,
      defaultValue: m['default'] is List
          ? [for (final v in m['default'] as List) _need(v)]
          : null,
    ),
    'number' => AcpNumberField(
      name,
      integer: m['integer'] == true,
      title: title,
      description: description,
      required: required,
      minimum: m['minimum'] as num?,
      maximum: m['maximum'] as num?,
      defaultValue: m['default'] as num?,
    ),
    'boolean' => AcpBooleanField(
      name,
      title: title,
      description: description,
      required: required,
      defaultValue: m['default'] as bool?,
    ),
    _ => AcpUnsupportedField(
      name,
      _string(m['schemaType']) ?? '',
      title: title,
      description: description,
      required: required,
    ),
  };
}

/// What a [ChatSessionShare] needs of whoever owns the conversation. [ModelSharedChat] is one over a [ChatSession]; an app that
/// keeps its conversation in a state of its own implements it with the same answers.
abstract interface class SharedChat {
  /// What the conversation is now.
  ChatSnapshot snapshot();

  /// Calls [onChange] after each change of the conversation; the result stops it.
  void Function() listen(void Function() onChange);

  /// Makes [id] a client that may send and answer as itself; throws when it cannot.
  void attachClient(String id, {String? name});
  void detachClient(String id);

  /// Sends a message as client [from], queueing it while a turn runs, as the conversation's own composer would.
  Future<void> send(String text, {required String from});

  /// Ends the running turn.
  Future<void> cancel({required String from});

  /// Answers permission request [id] with the agent's option [optionId], or cancelled when none; the first answer settles it.
  /// Returns whether this one did.
  bool settlePermission(int id, {String? optionId, required String by});

  /// Answers question [id]; the first answer settles it. Returns whether this one did.
  bool settleQuestion(
    int id,
    AcpElicitationAnswer answer, {
    required String by,
  });
}

/// A [ChatSession] as what a [ChatSessionShare] shares.
final class ModelSharedChat implements SharedChat {
  ModelSharedChat(this.session);
  final ChatSession session;

  @override
  ChatSnapshot snapshot() => ChatSnapshot.of(session);

  @override
  void Function() listen(void Function() onChange) {
    session.addListener(onChange);
    return () => session.removeListener(onChange);
  }

  @override
  void attachClient(String id, {String? name}) =>
      session.attachClient(id, name: name);

  @override
  void detachClient(String id) => session.detachClient(id);

  @override
  Future<void> send(String text, {required String from}) =>
      session.send(text, from: from);

  @override
  Future<void> cancel({required String from}) => session.cancel(from: from);

  @override
  bool settlePermission(int id, {String? optionId, required String by}) {
    final question = session.pendingPermissions
        .where((q) => q.id == id)
        .firstOrNull;
    if (question == null) return false;
    final known =
        optionId != null && question.options.any((o) => o.optionId == optionId);
    return session.settle(
      question,
      known ? AcpSelect(optionId) : const AcpCancel(),
      by: by,
    );
  }

  @override
  bool settleQuestion(
    int id,
    AcpElicitationAnswer answer, {
    required String by,
  }) {
    final question = session.pendingQuestions
        .where((q) => q.id == id)
        .firstOrNull;
    return question != null && session.settleQuestion(question, answer, by: by);
  }
}

/// A view came to the conversation or left it.
final class ChatViewEvent {
  const ChatViewEvent(this.clientId, {required this.attached});
  final String clientId;
  final bool attached;
}

final class _Served {
  _Served(this.link);
  final ChatViewLink link;
  String? clientId;
  String? lastSent;
  StreamSubscription<Map<String, Object?>>? subscription;
}

/// The owner's side of a conversation shown elsewhere. [serve] a link to each view that may attach; each gets the conversation as it
/// is, then again after each change (at most once per [minInterval], longer when building it takes long), and what a view asks
/// (a message, Stop, an answer) is done as that view's client: the first answer to a request settles it, and every view is then
/// told so. Experimental.
final class ChatSessionShare {
  ChatSessionShare(
    this.chat, {
    this.minInterval = const Duration(milliseconds: 80),
  }) {
    _unlisten = chat.listen(_changed);
  }

  /// A share of [session].
  factory ChatSessionShare.of(
    ChatSession session, {
    Duration minInterval = const Duration(milliseconds: 80),
  }) => ChatSessionShare(ModelSharedChat(session), minInterval: minInterval);

  final SharedChat chat;

  /// The least time between two states sent to a view.
  final Duration minInterval;

  late final void Function() _unlisten;
  final _served = <_Served>[];
  final _events = StreamController<ChatViewEvent>.broadcast();
  Timer? _timer;
  DateTime _nextAllowed = DateTime.fromMillisecondsSinceEpoch(0);
  bool _closed = false;

  /// The clients attached through this share.
  List<String> get views => [
    for (final s in _served)
      if (s.clientId != null) s.clientId!,
  ];

  /// Views coming and going, which a host shows (a window is open) and acts on (a window closed is docked back).
  Stream<ChatViewEvent> get events => _events.stream;

  /// Starts serving [link], whose far end is a [ChatSessionView.attach]: it waits for that end's `attach`.
  void serve(ChatViewLink link) {
    if (_closed) {
      unawaited(link.close());
      return;
    }
    final served = _Served(link);
    _served.add(served);
    served.subscription = link.messages.listen(
      (message) => _onMessage(served, message),
      onDone: () => _drop(served),
    );
  }

  void _onMessage(_Served served, Map<String, Object?> message) {
    switch (message['t']) {
      case 'attach':
        _attach(served, message);
      case 'detach':
        _drop(served);
      case final String t when served.clientId != null:
        _request(served, t, message);
    }
  }

  void _refuse(_Served served, String reason) {
    served.link.send({'t': 'refused', 'reason': reason});
    _drop(served);
  }

  void _attach(_Served served, Map<String, Object?> message) {
    if (served.clientId != null) return;
    final id = _string(message['client']);
    if (message['v'] != chatViewWireVersion) {
      return _refuse(
        served,
        'Wire version ${message['v']} is not $chatViewWireVersion',
      );
    }
    if (id == null || id.isEmpty) return _refuse(served, 'No client id');
    if (_served.any((s) => s.clientId == id)) {
      return _refuse(served, '$id is attached already');
    }
    try {
      chat.attachClient(id, name: _string(message['name']));
    } on Object catch (error) {
      return _refuse(served, '$error');
    }
    served.clientId = id;
    _events.add(ChatViewEvent(id, attached: true));
    // The attach changed the conversation (a client came), which has its own push; this one is the view's first.
    _send(served, jsonEncode(chat.snapshot().toJson()), force: true);
  }

  void _request(_Served served, String t, Map<String, Object?> message) {
    final from = served.clientId!;
    switch (t) {
      case 'send':
        final ref = message['ref'];
        final text = _string(message['text']);
        if (text == null) return;
        chat
            .send(text, from: from)
            .then(
              (_) => served.link.send({'t': 'ack', 'ref': ?ref}),
              onError: (Object error) => served.link.send({
                't': 'ack',
                'ref': ?ref,
                'error': '$error',
              }),
            );
      case 'cancel':
        unawaited(chat.cancel(from: from).catchError((Object _) {}));
      case 'permission':
        if (message['id'] case final int id) {
          chat.settlePermission(
            id,
            optionId: _string(message['option']),
            by: from,
          );
        }
      case 'question':
        if (message['id'] case final int id when message['answer'] is Map) {
          chat.settleQuestion(
            id,
            answerFromJson(_map(message['answer'])),
            by: from,
          );
        }
    }
    // What was done changed the conversation, which tells the listener; nothing more to do here.
  }

  void _drop(_Served served) {
    if (!_served.remove(served)) return;
    unawaited(served.subscription?.cancel());
    final id = served.clientId;
    served.clientId = null;
    unawaited(served.link.close());
    if (id != null) {
      try {
        chat.detachClient(id);
      } on Object catch (_) {}
      _events.add(ChatViewEvent(id, attached: false));
    }
  }

  void _changed() {
    if (_closed || _timer != null || !_served.any((s) => s.clientId != null)) {
      return;
    }
    final wait = _nextAllowed.difference(DateTime.now());
    _timer = Timer(wait.isNegative ? Duration.zero : wait, flush);
  }

  /// Sends every view what the conversation is now, if it changed since it was last sent to it, without waiting for the interval.
  void flush() {
    _timer?.cancel();
    _timer = null;
    if (_closed) return;
    final built = Stopwatch()..start();
    final json = jsonEncode(chat.snapshot().toJson());
    for (final served in [..._served]) {
      if (served.clientId != null) _send(served, json);
    }
    built.stop();
    final cost = built.elapsed * 4;
    _nextAllowed = DateTime.now().add(cost > minInterval ? cost : minInterval);
  }

  void _send(_Served served, String json, {bool force = false}) {
    if (!force && served.lastSent == json) return;
    served.lastSent = json;
    served.link.send((jsonDecode(json) as Map).cast<String, Object?>());
  }

  /// Tells every view the conversation is over for them, and detaches them.
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    _timer?.cancel();
    _unlisten();
    for (final served in [..._served]) {
      served.link.send({'t': 'ended', 'reason': 'The conversation is closed'});
      _drop(served);
    }
    await _events.close();
  }
}

/// The connection of a view's conversation: it is never asked anything, since the owner talks to the agent.
final class _ViewConnection extends ChatConnection {
  final _updates = StreamController<AcpSessionUpdate>.broadcast();
  final _events = StreamController<AcpEvent>.broadcast();
  bool _closed = false;

  @override
  Stream<AcpSessionUpdate> get updates => _updates.stream;
  @override
  Stream<AcpEvent> get events => _events.stream;
  @override
  bool get isClosed => _closed;
  @override
  int? get protocolVersion => 2;

  Never _no() =>
      throw UnsupportedError('A view asks through its ChatSessionView');

  @override
  Future<String?> prompt(
    String sessionId,
    String text, {
    List<AcpAttachment> attachments = const [],
  }) => _no();
  @override
  Future<void> cancel(String sessionId) => _no();
  @override
  Future<void> setMode(String sessionId, String modeId) => _no();
  @override
  Future<Map<String, dynamic>> setConfigOption(
    String sessionId,
    String configId,
    String value,
  ) => _no();

  @override
  Future<void> close() async {
    _closed = true;
    await _updates.close();
    await _events.close();
  }
}

/// A view of a conversation another client owns: [session] shows it, read-only, and follows it; what the person does in it goes to
/// the owner. Dock it back with [detach]; it ends by itself when the owner closes or the link goes. Experimental.
final class ChatSessionView {
  ChatSessionView._(this._link, this.clientId, this.session);

  final ChatViewLink _link;

  /// The name this view is attached under.
  final String clientId;

  /// The conversation as the owner last said. Its own methods that talk to an agent (`send`, `cancel`, ...) are not for a view:
  /// use this class's.
  final ChatSession session;

  final _acks = <int, Completer<void>>{};
  int _refs = 0;
  _ViewConnection? _connection;
  bool _open = true;
  final _done = Completer<String?>();
  StreamSubscription<Map<String, Object?>>? _subscription;

  /// Attaches to the conversation a [ChatSessionShare] serves on the far end of [link] as [clientId], and completes with the first
  /// state. Throws [StateError] when the owner refuses or says nothing within [timeout], and the link is closed then.
  static Future<ChatSessionView> attach(
    ChatViewLink link, {
    required String clientId,
    String? name,
    ChatLimits limits = const ChatLimits(),
    Duration timeout = const Duration(seconds: 10),
  }) async {
    final session = ChatSession(id: clientId, limits: limits);
    final view = ChatSessionView._(link, clientId, session);
    final first = Completer<void>();
    view._subscription = link.messages.listen(
      (message) {
        try {
          view._onMessage(message, first);
        } on Object catch (error) {
          if (!first.isCompleted) first.completeError(StateError('$error'));
        }
      },
      onDone: () {
        if (!first.isCompleted) {
          first.completeError(StateError('The link closed'));
        }
        view._end(null);
      },
    );
    link.send({
      't': 'attach',
      'v': chatViewWireVersion,
      'client': clientId,
      'name': ?name,
    });
    try {
      await first.future.timeout(
        timeout,
        onTimeout: () => throw StateError('The owner did not answer'),
      );
    } on Object {
      await view.detach();
      rethrow;
    }
    return view;
  }

  /// Whether the view still follows the conversation.
  bool get isOpen => _open;

  /// Completes when the view ends, with the owner's reason when it gave one.
  Future<String?> get done => _done.future;

  void _onMessage(Map<String, Object?> message, Completer<void> first) {
    switch (message['t']) {
      case 'state':
        _applyState(ChatSnapshot.fromJson(message));
        if (!first.isCompleted) first.complete();
      case 'ack':
        final ref = message['ref'];
        final waiting = ref is int ? _acks.remove(ref) : null;
        if (message['error'] case final String error) {
          waiting?.completeError(StateError(error));
        } else {
          waiting?.complete();
        }
      case 'refused':
        final reason = _string(message['reason']) ?? 'refused';
        if (!first.isCompleted) first.completeError(StateError(reason));
        _end(reason);
      case 'ended':
        _end(_string(message['reason']));
    }
  }

  void _applyState(ChatSnapshot s) {
    if (!_open) return;
    session
      ..agent = s.agent ?? ''
      ..cwd = s.cwd
      ..sessionId = s.sessionId ?? clientId;
    final id = session.sessionId!;
    if (s.status != 'disconnected' && session.connection == null) {
      session.attach(_connection = _ViewConnection());
    }
    // Clients first, so an answer that names one has it.
    final named = {for (final c in s.clients) c.id};
    for (final c in s.clients) {
      try {
        session.attachClient(c.id, name: c.name);
      } on StateError catch (_) {}
    }
    for (final c in [...session.clients]) {
      if (!named.contains(c.id)) session.detachClient(c.id);
    }
    session.resetFromReplay(
      [
        for (final raw in s.replay)
          AcpSessionUpdate(id, Map<String, dynamic>.of(raw)),
      ],
      queue: s.queue,
      queueHeld: s.queueHeld,
    );
    _requests(s, named);
    if (s.status == 'disconnected' && session.connection != null) {
      session.detach();
      _connection = null;
    }
  }

  void _requests(ChatSnapshot s, Set<String> named) {
    String? by(SharedOutcome? o) =>
        o?.by != null && (named.contains(o!.by) || o.by == bypassClient)
        ? o.by
        : null;
    final waitingPermissions = {for (final p in s.permissions) p.id};
    for (final question in [...session.pendingPermissions]) {
      if (waitingPermissions.contains(question.id)) continue;
      final outcome = s.settledPermissions
          .where((o) => o.id == question.id)
          .firstOrNull;
      final decision = outcome?.how == 'answered' && outcome?.answer != null
          ? decisionFromJson(outcome!.answer!)
          : const AcpCancel();
      session.settle(question, decision, by: by(outcome));
    }
    final settledHere = {for (final q in session.settledPermissions) q.id};
    for (final p in s.permissions) {
      if (settledHere.contains(p.id)) continue;
      session.adoptPermission(
        id: p.id,
        title: p.title,
        kind: p.kind,
        detail: p.detail,
        reason: p.reason,
        diff: p.diff,
        options: p.options,
        bypassPassed: p.bypassPassed,
        forPerson: p.forPerson,
        toolCallId: p.toolCallId,
        askedAt: p.askedAt,
      );
    }
    final waitingQuestions = {for (final q in s.questions) q.id};
    for (final question in [...session.pendingQuestions]) {
      if (waitingQuestions.contains(question.id)) continue;
      final outcome = s.settledQuestions
          .where((o) => o.id == question.id)
          .firstOrNull;
      final answer = outcome?.how == 'answered' && outcome?.answer != null
          ? answerFromJson(outcome!.answer!)
          : const AcpElicitCancel();
      session.settleQuestion(question, answer, by: by(outcome));
    }
    final questionsHere = {for (final q in session.settledQuestions) q.id};
    for (final q in s.questions) {
      if (questionsHere.contains(q.id)) continue;
      session.adoptQuestion(id: q.id, request: q.request, askedAt: q.askedAt);
    }
  }

  void _end(String? reason) {
    if (!_open) return;
    _open = false;
    session.detach();
    unawaited(_connection?.close());
    unawaited(_subscription?.cancel());
    for (final waiting in _acks.values) {
      waiting.completeError(
        StateError(reason ?? 'The conversation is not followed any more'),
      );
    }
    _acks.clear();
    session.notifyListeners();
    if (!_done.isCompleted) _done.complete(reason);
  }

  void _requireOpen() {
    if (!_open) throw StateError('This view is not attached any more');
  }

  /// Sends [text] to the conversation as this view's client. It completes when the owner has taken it, as [ChatSession.send]
  /// does (queued while a turn runs), and throws when the owner could not.
  Future<void> send(String text) {
    _requireOpen();
    final ref = _refs++;
    final waiting = _acks[ref] = Completer<void>();
    _link.send({'t': 'send', 'ref': ref, 'text': text});
    return waiting.future;
  }

  /// Asks the owner to end the running turn.
  void cancel() {
    _requireOpen();
    _link.send({'t': 'cancel'});
  }

  /// Answers [question] with one of its options, or cancelled when [option] is null. The first answer from any view or the owner
  /// settles it; this view is told how it ended with the next state.
  void choosePermission(
    PermissionQuestion question,
    AcpPermissionOption? option,
  ) {
    _requireOpen();
    _link.send({
      't': 'permission',
      'id': question.id,
      'option': ?option?.optionId,
    });
  }

  /// Answers [question].
  void answerQuestion(AgentQuestion question, AcpElicitationAnswer answer) {
    _requireOpen();
    _link.send({
      't': 'question',
      'id': question.id,
      'answer': answerToJson(answer),
    });
  }

  /// Leaves the conversation: the owner detaches this client and goes on as it was. What it held stays here to read.
  Future<void> detach() async {
    if (_open) {
      _link.send({'t': 'detach'});
      _end(null);
    }
    await _link.close();
  }
}
