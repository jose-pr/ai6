import 'dart:async';
import 'dart:collection';

import 'package:coqui_acp/coqui_acp.dart';

import 'children.dart';
import 'connection.dart';
import 'content.dart';
import 'limits.dart';
import 'notifier.dart';
import 'options.dart';
import 'rendering.dart';
import 'text.dart';
import 'usage.dart';

part 'hub.dart';

/// Where a conversation stands with its agent.
enum ChatStatus { disconnected, connected, working }

/// Who a conversation entry is from, or what it is.
enum MessageRole { user, agent, notice, thought, tool, compaction }

/// What a conversation needs a person for: a permission request, a question, or a turn it finished.
enum AttentionKind { permission, question, finished }

/// How a turn ended.
enum TurnOutcome { completed, cancelled, failed }

/// The name a request is settled under when the chat's own Bypass answered it.
const String bypassClient = 'bypass';

const String _clientMetaKey = 'dev.boriken.coqui.acp/client';
const String _atMetaKey = 'dev.boriken.coqui.acp/at';
const String _namesMetaKey = 'dev.boriken.coqui.acp/agent_names';

/// One entry of a conversation: a message, a notice, a tool call or a compaction. Only the conversation changes one.
final class ChatMessage {
  ChatMessage._(
    this.role,
    ChatLimits limits,
    String text,
    this.at, {
    String? messageId,
    this.tool,
    this.compaction,
    String? sentBy,
    int? turn,
  }) : _text = BoundedText.head(limits.messageText),
       _messageId = messageId,
       _sentBy = sentBy,
       _turn = turn {
    _text.set(text);
  }

  final MessageRole role;
  final BoundedText _text;
  final DateTime at;
  String? _messageId;
  String? _sentBy;
  int? _turn;
  final ToolCall? tool;
  final Compaction? compaction;

  String get text => _text.value;

  /// The agent's id for the message, once it is known.
  String? get messageId => _messageId;

  /// The client a person's message was sent from, when it is known.
  String? get sentBy => _sentBy;

  /// The [ChatTurn.id] of the turn this belongs to: the one that was running when it was added, or the one the prompt started.
  int? get turn => _turn;
}

/// One turn of the agent: from when it started working until it was idle again.
final class ChatTurn {
  ChatTurn._(this.id, this.startedAt);
  final int id;
  final DateTime startedAt;
  DateTime? _endedAt;
  TurnOutcome? _outcome;

  DateTime? get endedAt => _endedAt;
  TurnOutcome? get outcome => _outcome;

  /// How long it ran, or has run so far.
  Duration duration(DateTime now) => (_endedAt ?? now).difference(startedAt);

  bool get isRunning => _endedAt == null;
}

/// A client attached to a conversation: a window, a device, a person watching it.
final class ChatClient {
  ChatClient._(this.id, this.name, this.attachedAt);
  final String id;
  final String? name;
  final DateTime attachedAt;
}

/// How a request the agent made of the person came to its end.
enum Settlement {
  /// A client answered it, [Settled.by] when it said which.
  answered,

  /// It was answered cancelled because the turn was stopped or the conversation closed.
  cancelled,

  /// The connection it came by ended or was replaced, or the conversation was let go: nobody can answer it, and the agent, if it
  /// is still there, was told it was cancelled.
  withdrawn,
}

/// What became of a request.
final class Settled<T> {
  const Settled._(this.how, this.at, {this.by, this.answer});
  final Settlement how;
  final DateTime at;

  /// The client that answered it, [bypassClient] for Bypass; null when nobody did.
  final String? by;

  /// What the agent was told.
  final T? answer;
}

/// A tool an agent wants to use, waiting on the person. Nothing answers it for them but Bypass, which answers a request that
/// offers an allow before it would wait here. The first client to answer it settles it; the rest find [outcome] set.
final class PermissionQuestion {
  PermissionQuestion._(
    this.id,
    this.title,
    this.kind, {
    this.detail,
    this.reason,
    this.diff,
    this.options = const [],
    this.bypassPassed = false,
    this.forPerson = false,
    this.toolCallId,
    this.hostData,
    required this.askedAt,
  });

  /// Its place in the order the conversation was asked, from 1.
  final int id;
  final String title;
  final String? kind;
  final DateTime askedAt;

  /// The tool call this asks about, whose line in the conversation is marked stopped when the request can no longer be answered.
  final String? toolCallId;

  /// What it would run or touch, when the agent said.
  final String? detail;

  /// Why the agent says it needs this, when it said (v2's `description`).
  final String? reason;

  /// The change an edit would make, when the request carries one.
  final ToolDiff? diff;

  /// The answers the agent offers, in its order.
  final List<AcpPermissionOption> options;

  /// Whether Bypass was on and left this to the person.
  final bool bypassPassed;

  /// Whether it is a question or a plan for the person, which Bypass never answers.
  final bool forPerson;

  /// What the host's [PermissionPolicy] attached to it, which the model does not read.
  final Object? hostData;

  final Completer<AcpDecision> _answer = Completer();
  Settled<AcpDecision>? _outcome;

  /// Null while it waits.
  Settled<AcpDecision>? get outcome => _outcome;

  bool get isOpen => _outcome == null;
}

/// What a host decides about a permission request before a person is asked, given to a conversation as
/// [ChatSession.permissionPolicy]. With one, requests are decided one after another in the order they came.
abstract interface class PermissionPolicy {
  /// Awaited, in the order the requests came, before [review]: a host reads here what its rules need. A conversation that is
  /// stopped, closed or whose connection ended meanwhile answers the request cancelled and decides nothing.
  Future<void> prepare(AcpPermissionRequest request);

  /// How [request] is put, read after [prepare] with the conversation as it is then: whether it is the person's alone, and what
  /// the host wants kept with it. [forPerson] says it is already, by what it asks (a question or a plan).
  PermissionReview review(
    AcpPermissionRequest request, {
    required bool forPerson,
  });

  /// The host's own answer to [request], asked when Bypass did not answer it; null leaves it to the person.
  PermissionAnswer? answer(
    AcpPermissionRequest request,
    PermissionReview review,
  );
}

/// How a host has [PermissionPolicy.review] a request put.
final class PermissionReview {
  const PermissionReview({
    this.forPerson = false,
    this.label = '',
    this.ownCall = true,
    this.hostData,
  });

  /// Whether it is the person's alone, as a plan is: Bypass never answers it, and neither does [PermissionPolicy.answer].
  final bool forPerson;

  /// Words said after the request's title in what is noted of it (` for a task`).
  final String label;

  /// Whether the tool call it asks about is this conversation's own, so its line shows before the card; false for another
  /// thread's.
  final bool ownCall;

  /// Kept on the [PermissionQuestion] as [PermissionQuestion.hostData].
  final Object? hostData;
}

/// The host's own answer to a request, and what is noted in the conversation of it.
final class PermissionAnswer {
  const PermissionAnswer(this.decision, this.note);
  final AcpDecision decision;
  final String note;
}

/// An agent's question, waiting on the person. Nothing answers it for them; Bypass is for permission requests only.
final class AgentQuestion {
  AgentQuestion._(this.id, this.request, this.askedAt);
  final int id;
  final AcpElicitation request;
  final DateTime askedAt;
  final Completer<AcpElicitationAnswer> _answer = Completer();
  Settled<AcpElicitationAnswer>? _outcome;
  Settled<AcpElicitationAnswer>? get outcome => _outcome;
  bool get isOpen => _outcome == null;
}

/// A message written while the agent was working, sent when its turn ends.
final class QueuedMessage {
  QueuedMessage._(
    this._text,
    List<AcpAttachment> attachments, {
    this.from,
    this.origin,
    bool steered = false,
  }) : _attachments = List.of(attachments),
       _steered = steered {
    // Whoever does not wait for it is not told of a failure it has no use for.
    _handed.future.ignore();
  }
  String _text;
  String get text => _text;
  List<AcpAttachment> _attachments;
  List<AcpAttachment> get attachments => UnmodifiableListView(_attachments);

  /// The client that wrote it.
  final String? from;

  /// What the host that wrote it keeps with it, such as what its attachments were before they were read. The conversation
  /// never looks at it; the host keeps it right when it joins messages.
  Object? origin;

  final bool _steered;

  /// A steer still waiting for its turn to end.
  bool get _goingFirst => _steered && !_handed.isCompleted;
  final Completer<void> _handed = Completer<void>();

  /// Completes when the agent has been handed the message. A message written to steer that could not be sent fails here with
  /// the reason, and is no longer in the queue, so a host can give it back to the person.
  Future<void> get handedOver => _handed.future;
}

/// How a queue goes on when a turn ends as it should.
enum QueueDelivery {
  /// One prompt holds everything that waits, joined with a blank line between the messages.
  merge,

  /// One message goes with each turn.
  each,
}

/// Something a conversation tells beside its state: it is delivered once, to whoever listens when it happens.
sealed class ChatEvent {
  const ChatEvent();
}

/// The conversation came to need the person.
final class AttentionEvent extends ChatEvent {
  const AttentionEvent(this.kind);
  final AttentionKind kind;
}

/// A turn ended, however it ended, cancelled included. History a conversation is opened again with ends no turn in this sense.
final class TurnEvent extends ChatEvent {
  const TurnEvent(this.turn);
  final ChatTurn turn;
}

/// A permission request was settled, by whom [PermissionQuestion.outcome] says: the others attached see it settled.
final class PermissionSettledEvent extends ChatEvent {
  const PermissionSettledEvent(this.question);
  final PermissionQuestion question;
}

/// A question was settled.
final class QuestionSettledEvent extends ChatEvent {
  const QuestionSettledEvent(this.question);
  final AgentQuestion question;
}

/// A client attached or detached.
final class ClientEvent extends ChatEvent {
  const ClientEvent(this.client, {required this.attached});
  final ChatClient client;
  final bool attached;
}

/// Why [error] happened, in the words it carries for a person: for an agent's own error, the message it gave.
String reasonOf(Object error) => switch (error) {
  StateError(:final message) => message,
  AcpFailure(:final message) =>
    RegExp(
          r'^Agent error: \{.*\bmessage: (.*)\}$',
          dotAll: true,
        ).firstMatch(message)?.group(1) ??
        message,
  _ => '$error',
};

/// The title a conversation takes from its first message when it was given none.
String titleFrom(String text, [ChatLimits limits = const ChatLimits()]) {
  final line = text.trim().split('\n').first.trim();
  final max = limits.titleLength;
  if (line.length <= max) return line;
  return max == 0 ? '' : '${headOf(line, max - 1).trimRight()}…';
}

/// [entries] in runs of consecutive tool calls: a transcript can fold each run into one line, such as "Edited calc.js, ran a
/// command", and open one that failed or waits. Every other entry is a run of its own.
List<List<ChatMessage>> toolRuns(Iterable<ChatMessage> entries) {
  final runs = <List<ChatMessage>>[];
  for (final entry in entries) {
    if (entry.role == MessageRole.tool &&
        runs.isNotEmpty &&
        runs.last.last.role == MessageRole.tool) {
      runs.last.add(entry);
    } else {
      runs.add([entry]);
    }
  }
  return runs;
}

/// One conversation with an agent, built from the updates it sends and the requests it makes, and the person's side of it:
/// prompts, the queue, answers. It has no widgets and no transport; a [ChatConnection] is attached to it. Its state is read through
/// getters and changed only by its methods.
///
/// Several clients can be attached ([attachClient]): a prompt is stamped with the one that sent it, and of several answers to a
/// request the first settles it.
class ChatSession extends ChatNotifier {
  ChatSession({
    required this.id,
    String title = 'New conversation',
    this.agent = '',
    this.peerId,
    this.sessionId,
    this.cwd,
    this.limits = const ChatLimits(),
    this.recorder,
    this.hostNoticesKey,
    DateTime Function()? clock,
    bool agentNames = true,
  }) : _title = title,
       _agentNames = agentNames,
       _clock = clock ?? DateTime.now,
       _plan = const [],
       _options = const [],
       _commands = const [];

  /// This view's own id, which the agent's session id is not.
  final String id;

  /// The agent's name as shown. The host owns it.
  String agent;

  /// Who the agent is, as the host proved it. The host owns it.
  String? peerId;

  /// The agent's id for the conversation, which its updates and the requests sent for it name. Null until the agent has opened it.
  String? sessionId;

  /// Where the agent works, on its machine, when the host knows.
  String? cwd;

  final ChatLimits limits;

  /// Told the kind and shape of each update this conversation applies, never its text.
  final UpdateRecorder? recorder;

  /// The `_meta` key under which a host lists, in its answer to `session/new`, what a person should know about an agent. Null reads
  /// none.
  final String? hostNoticesKey;

  final DateTime Function() _clock;
  DateTime? _updateAt;

  /// Whether a turn ended here since the person last chose this conversation: set by [ChatStore], which the host reads.
  bool unread = false;

  /// Told, before the conversation's listeners are, each time it comes to need the person.
  void Function(AttentionKind kind)? onAttention;

  /// Called after every change of state; [ChatStore] sets it to pass the change on.
  void Function()? onChange;

  /// Told where a mode the person or the agent set is to be kept, such as in a ledger, with whether Bypass is on. Not told while the
  /// conversation is being opened again.
  void Function(String? mode, bool bypass)? onModeKept;

  /// Whether Bypass answers only with an answer that allows once, where an agent offers both that and one that allows always.
  bool bypassOnceOnly = false;

  /// What the host decides about a request before the person is asked; see [PermissionPolicy].
  PermissionPolicy? permissionPolicy;

  /// Told, as it settles, of each permission request: who answered it and how is in [PermissionQuestion.outcome].
  void Function(PermissionQuestion question)? onPermissionSettled;

  /// Told, before a prompt is handed to the agent and after the choices made earlier are sent, of its text and what goes with it.
  void Function(String text, List<AcpAttachment> attachments)? onPrompt;

  /// Told when the agent took a choice a person made, such as the model: its role and the value.
  void Function(OptionRole role, String value)? onChoice;

  /// Told after the agent's options change by an update.
  void Function()? onOptions;

  /// Told when the agent names the conversation, with the name.
  void Function(String title)? onTitle;

  /// Told when the agent changed its own mode, with the mode it was in and the one it is in.
  void Function(String before, String now)? onAgentMode;

  /// Whether a mode the agent moved to by itself counts as the conversation's chosen mode, kept for it to be put back when it is
  /// opened again. Off, it is shown and nothing more.
  bool agentModeIsChosen = true;

  /// Words a mode for a person where the agent's own name is not what to show, in the notes and menus the conversation makes.
  String Function(Choice choice)? modeLabel;

  int _askEpoch = 0;
  Future<void> _askTail = Future.value();
  final List<Completer<AcpDecision>> _askWaiters = [];

  String _title;
  bool _agentNames;
  ChatStatus _status = ChatStatus.disconnected;
  final List<ChatMessage> _messages = [];
  final List<ChatMessage> _heldFirst = [];
  final Map<String, ChatMessage> _toolEntries = {};
  late final ChildRegistry _childRegistry = ChildRegistry(limits);
  final Map<String, ChatMessage> _compactions = {};
  final Map<String, TerminalView> _terminals = {};
  final Set<String> _attachmentsNamed = {};
  List<PlanEntry> _plan;
  List<ConfigOption> _options;
  List<AgentCommandInfo> _commands;
  SessionUsage _usage = const SessionUsage();
  UsageReport? _usageReport;
  bool _reportsUsage = false;
  UsageSpend? _spend;
  final List<PermissionQuestion> _pendingPermissions = [];
  final List<AgentQuestion> _pendingQuestions = [];
  final List<PermissionQuestion> _settledPermissions = [];
  final List<AgentQuestion> _settledQuestions = [];
  final List<QueuedMessage> _queue = [];
  String? _queueHeld;
  final Map<String, PendingChoice> _pending = {};
  bool _bypass = false;
  bool _opening = false;
  String? _chosenMode;
  String? _targetMode;
  final Set<String> _refusedModes = {};
  int _modesWaiting = 0;
  Future<void> _modeQueue = Future.value();
  final List<ChatTurn> _turns = [];
  int _nextTurn = 1;
  int _nextRequest = 1;

  /// User messages sent here whose id the agent has not yet given.
  final List<ChatMessage> _unacked = [];

  /// How many updates were applied, to tell whether a prompt that failed got any answer at all.
  int _applied = 0;

  final _events = StreamController<ChatEvent>.broadcast();
  final Map<String, ChatClient> _clients = {};

  ChatConnection? _connection;
  StreamSubscription<AcpSessionUpdate>? _updatesSub;
  StreamSubscription<AcpEvent>? _eventsSub;
  StreamSubscription<UsageNotice>? _usageSub;
  bool _steering = false;

  /// The message and the note of attachments shown for the v1 prompt now under way, and how many updates there had been, so a
  /// prompt the agent refuses before it says anything can take them back.
  ChatMessage? _v1Local;
  ChatMessage? _v1Note;
  int _v1Before = 0;

  /// The turns of v1 prompts whose answer has not come, oldest first: an end the connection reports belongs to the first of them.
  final List<ChatTurn> _v1Awaiting = [];
  bool _replaying = false;

  /// Whether a change is being made in pieces that listeners should hear of once, when it is done.
  bool _quiet = false;

  /// Whether a message is being handed to the agent.
  bool _delivering = false;

  /// How a turn ended while a message was being handed over, whose queue then goes on once it has been.
  ({TurnOutcome outcome, bool steered})? _deferredEnd;

  // What can be read.

  String get title => _title;

  /// Whether the agent may name the conversation, which it may until the person has ([rename]).
  bool get agentNames => _agentNames;
  set agentNames(bool value) {
    if (_agentNames == value) return;
    _agentNames = value;
    notifyListeners();
  }

  ChatStatus get status => _status;

  List<ChatMessage> get messages => UnmodifiableListView(_messages);
  Map<String, ChatMessage> get toolEntries => UnmodifiableMapView(_toolEntries);
  Map<String, ChatMessage> get compactions => UnmodifiableMapView(_compactions);

  /// Commands the agent ran, which a tool's entry shows.
  Map<String, TerminalView> get terminals => UnmodifiableMapView(_terminals);

  /// The agent's plan, as it last said.
  List<PlanEntry> get plan => UnmodifiableListView(_plan);

  /// What the agent lets a person choose, its commands and what it has used, each as it last said.
  List<ConfigOption> get options => UnmodifiableListView(_options);
  List<AgentCommandInfo> get commands => UnmodifiableListView(_commands);
  SessionUsage get usage => _usage;

  /// What the agent last said of the account's allowances; null while it has not.
  UsageReport? get usageReport => _usageReport;

  /// Whether this conversation's agent reports usage.
  bool get reportsUsage => _reportsUsage;

  /// What this conversation has spent, as the agent last said.
  UsageSpend? get spend => _spend;

  /// What the agent is waiting for the person to answer, oldest first.
  List<PermissionQuestion> get pendingPermissions =>
      UnmodifiableListView(_pendingPermissions);

  /// The agent's questions waiting for the person, oldest first.
  List<AgentQuestion> get pendingQuestions =>
      UnmodifiableListView(_pendingQuestions);

  /// The requests most recently settled, oldest first, for a client that attaches late to see who answered.
  List<PermissionQuestion> get settledPermissions =>
      UnmodifiableListView(_settledPermissions);
  List<AgentQuestion> get settledQuestions =>
      UnmodifiableListView(_settledQuestions);

  /// Messages written while a turn ran, oldest first. The first is sent when a turn ends as it should.
  List<QueuedMessage> get queue => UnmodifiableListView(_queue);

  /// Why the queue waits for the person rather than going on: a turn that was stopped or failed. Null while it goes on.
  String? get queueHeld => _queueHeld;

  /// Whether the queue goes with one message to a turn or all at once when a turn ends as it should.
  QueueDelivery queueDelivery = QueueDelivery.each;

  /// Choices made while a turn ran, by option id: sent before the next prompt.
  Map<String, PendingChoice> get pendingChoices =>
      UnmodifiableMapView(_pending);

  /// Whether this chat answers the conversation's permission requests that offer an allow, as the Bypass mode of an agent that has
  /// none of its own. Off unless a person chose it ([setBypass]).
  bool get bypass => _bypass;

  /// Whether the conversation is being opened again and its agent has not loaded it yet.
  bool get opening => _opening;

  /// The mode the person, the host or the agent last set, as the agent names it. Never the agent's starting default.
  String? get chosenMode => _chosenMode;

  /// The mode last asked for, until the agent has taken it or refused it, so a quick second change goes on from it.
  String? get targetMode => _targetMode;

  /// Modes the agent refused, which cycling then skips.
  Set<String> get refusedModes => UnmodifiableSetView(_refusedModes);

  /// The turns of this conversation, oldest first, up to [ChatLimits.turns].
  List<ChatTurn> get turns => UnmodifiableListView(_turns);

  /// The turn running now; null between turns.
  ChatTurn? get currentTurn =>
      _turns.isNotEmpty && _turns.last.isRunning ? _turns.last : null;

  /// Whether something here waits for the person's answer.
  bool get needsYou =>
      _pendingPermissions.isNotEmpty || _pendingQuestions.isNotEmpty;

  /// What happened beside the state: attention, turns ending, settled requests, clients coming and going.
  Stream<ChatEvent> get events => _events.stream;

  /// The clients attached now, oldest first.
  List<ChatClient> get clients =>
      UnmodifiableListView(_clients.values.toList());

  /// The connection this conversation is with; null when it has none.
  ChatConnection? get connection => _connection;

  /// Whether the agent is sending the conversation so far, which is not news to anyone.
  bool get isReplaying => _replaying;

  DateTime _now() => _updateAt ?? _clock();

  @override
  void notifyListeners() {
    if (_quiet) return;
    try {
      super.notifyListeners();
    } finally {
      onChange?.call();
    }
  }

  void _emit(ChatEvent event) {
    if (!_events.isClosed) _events.add(event);
  }

  void _needs(AttentionKind kind) {
    if (_replaying) return;
    onAttention?.call(kind);
    _emit(AttentionEvent(kind));
  }

  /// Puts a notice in the conversation, which is the chat's word and not the agent's.
  void note(String text) {
    _note(text);
    _trim();
    notifyListeners();
  }

  /// Puts a message the host holds in the conversation, such as what a kept copy of it says, at [at] when it is given.
  void addMessage(MessageRole role, String text, {DateTime? at}) {
    _add(_entry(role, text, at: at));
    _trim();
    notifyListeners();
  }

  /// Shows the first message of a conversation whose agent has not started, as the person's own, before [send] can take it.
  /// [attachments] names what goes with it. [send] replaces it with the message it sends, at the time shown here; [releaseFirst]
  /// takes it back out when nothing is to be sent.
  void holdFirst(String text, {List<String> attachments = const []}) {
    releaseFirst(notify: false);
    final message = _entry(MessageRole.user, text);
    _heldFirst.add(message);
    _add(message);
    if (attachments.isNotEmpty) {
      final note = _entry(
        MessageRole.notice,
        'Sent with this: ${attachments.join(', ')}',
      );
      _heldFirst.add(note);
      _add(note);
    }
    _trim();
    notifyListeners();
  }

  /// The first message shown by [holdFirst] and not yet sent; null when there is none.
  ChatMessage? get heldFirst => _heldFirst.firstOrNull;

  /// Takes out what [holdFirst] showed.
  void releaseFirst({bool notify = true}) {
    if (_heldFirst.isEmpty) return;
    _messages.removeWhere(_heldFirst.contains);
    _heldFirst.clear();
    if (notify) notifyListeners();
  }

  /// Takes out the messages [where] names, such as a host's copy of what was said that it puts another in place of.
  void removeMessages(bool Function(ChatMessage message) where) {
    final gone = _messages.where(where).toList();
    if (gone.isEmpty) return;
    for (final message in gone) {
      if (message.tool != null) _toolEntries.remove(message.tool!.id);
      if (message.compaction != null) {
        _compactions.remove(message.compaction!.id);
      }
      _unacked.remove(message);
    }
    _messages.removeWhere(gone.contains);
    notifyListeners();
  }

  /// Changes the conversation's name to [name] without ending the agent's naming of it, as a name made from the first message.
  void retitle(String name) {
    final shown = shortText(name, limits);
    if (shown == null) return;
    _title = shown;
    notifyListeners();
  }

  /// Puts Bypass on or off silently, before an agent is there to be told or to say: a draft that starts in it.
  void presetBypass(bool on) {
    if (_bypass == on) return;
    _bypass = on;
    notifyListeners();
  }

  /// Takes back the choices still waiting that [where] names, or all of them.
  void dropChoices({bool Function(PendingChoice choice)? where}) {
    final before = _pending.length;
    _pending.removeWhere((_, choice) => where?.call(choice) ?? true);
    if (_pending.length != before) notifyListeners();
  }

  /// Sets the person's name for the conversation, which the agent then no longer replaces.
  void rename(String name) {
    final shown = shortText(name, limits);
    if (shown == null) return;
    _title = shown;
    _agentNames = false;
    notifyListeners();
  }

  // Clients.

  /// Attaches [clientId], a name the host gives each client, which then may send and answer as itself. Attaching one that is
  /// attached already changes nothing. Throws when [ChatLimits.clients] are attached.
  ChatClient attachClient(String clientId, {String? name}) {
    final held = _clients[clientId];
    if (held != null) return held;
    if (clientId.isEmpty || clientId == bypassClient) {
      throw ArgumentError.value(clientId, 'clientId', 'Not a usable client id');
    }
    if (_clients.length >= limits.clients) {
      throw StateError('${limits.clients} clients are attached already');
    }
    final client = ChatClient._(clientId, shortText(name, limits), _now());
    _clients[clientId] = client;
    _emit(ClientEvent(client, attached: true));
    notifyListeners();
    return client;
  }

  /// Detaches [clientId]. What it sent and answered stays; the conversation goes on without it.
  void detachClient(String clientId) {
    final client = _clients.remove(clientId);
    if (client == null) return;
    _emit(ClientEvent(client, attached: false));
    notifyListeners();
  }

  void _requireClient(String? clientId) {
    if (clientId != null &&
        clientId != bypassClient &&
        !_clients.containsKey(clientId)) {
      throw StateError('$clientId is not attached to this conversation');
    }
  }

  // The connection.

  /// Binds [connection], which the agent's updates for [sessionId] arrive on. A conversation bound to another connection first
  /// ends its turn and withdraws what that one asked, since nothing can answer it now. Throws when the connection is closed or no
  /// session id is known.
  void attach(ChatConnection connection) {
    if (sessionId == null || connection.isClosed) {
      throw StateError('An open connection and a session id are required');
    }
    unawaited(_updatesSub?.cancel());
    unawaited(_eventsSub?.cancel());
    unawaited(_usageSub?.cancel());
    _usageSub = null;
    final previous = _connection;
    if (previous != null && !identical(previous, connection)) {
      _closeTurn(TurnOutcome.failed);
      _v1Awaiting.clear();
      _withdraw();
    }
    _connection = connection;
    _status = ChatStatus.connected;
    _updatesSub = connection.updates.listen((update) {
      if (update.sessionId == sessionId) apply(update);
    }, onDone: () => _connectionEnded(connection));
    _eventsSub = connection.events.listen(
      (event) => _onEvent(connection, event),
    );
    _watchUsage(connection);
    notifyListeners();
  }

  void _connectionEnded(ChatConnection connection) {
    if (!identical(_connection, connection)) return;
    _lose();
    notifyListeners();
  }

  void _lose() {
    _connection = null;
    _status = ChatStatus.disconnected;
    _closeTurn(TurnOutcome.failed);
    _deferredEnd = null;
    _unacked.clear();
    _v1Awaiting.clear();
    _withdraw();
  }

  /// Stops listening to the connection without closing it: it stays with whoever else uses it. What it asked the person is
  /// withdrawn.
  void detach() {
    unawaited(_updatesSub?.cancel());
    unawaited(_eventsSub?.cancel());
    unawaited(_usageSub?.cancel());
    _updatesSub = _eventsSub = _usageSub = null;
    if (_connection != null) {
      _lose();
      notifyListeners();
    }
  }

  void _onEvent(ChatConnection connection, AcpEvent event) {
    if (!identical(_connection, connection)) return;
    if (event is AcpOutputLost) {
      // Not about a session: the gap may be in any that shares the connection.
      final messages = event.lines == 1
          ? '1 message'
          : '${event.lines} messages';
      _note(
        'While nobody was connected, $messages from the agent (${event.bytes} bytes) were dropped, '
        'so something it said or did is missing here.',
      );
      notifyListeners();
      return;
    }
    if (event is AcpBlocked) {
      if (event.sessionId != null && event.sessionId != sessionId) return;
      // The agent was told the question was cancelled, and its turn goes on until it ends it.
      _note(
        'A question from the agent was answered cancelled: ${event.message}.',
      );
      notifyListeners();
      return;
    }
    if (connection.protocolVersion != 1) return;
    final named = switch (event) {
      AcpCompleted(:final sessionId) => sessionId,
      AcpCancelled(:final sessionId) => sessionId,
      AcpFailed(:final sessionId) => sessionId,
      _ => null,
    };
    if (named != null && named != sessionId) return;
    // A turn end on a connection shared with other sessions is this one's only if this one is working.
    if (_status != ChatStatus.working) return;
    // It belongs to the first prompt not yet answered: if that turn has ended another way, this is its second end, not the end of the
    // one running now.
    final owner = _v1Awaiting.firstOrNull;
    if (owner != null && !identical(owner, currentTurn)) return;
    switch (event) {
      case AcpCompleted():
        _status = ChatStatus.connected;
        _needs(AttentionKind.finished);
        _turnEnded(TurnOutcome.completed);
      case AcpCancelled():
        _status = ChatStatus.connected;
        _note('Stopped.');
        _turnEnded(TurnOutcome.cancelled);
      case AcpFailed(:final message):
        _status = ChatStatus.connected;
        _dropV1Prompt();
        _add(_entry(MessageRole.notice, message));
        _needs(AttentionKind.finished);
        _turnEnded(TurnOutcome.failed);
      default:
        return;
    }
    notifyListeners();
  }

  // Usage.

  void _watchUsage(ChatConnection connection) {
    _usageReport = null;
    _reportsUsage = connection.reportsUsage;
    if (!_reportsUsage) return;
    _usageSub = connection.usageNotices.listen(
      (notice) => _usageArrived(notice.sessionId, notice.report),
    );
    unawaited(refreshUsage());
  }

  /// Asks the agent for its report now, as it is after each turn.
  Future<void> refreshUsage() async {
    final connection = _connection;
    if (connection == null || connection.isClosed || !_reportsUsage) return;
    final report = await connection.fetchUsage(sessionId: sessionId);
    if (report != null && identical(_connection, connection)) {
      _usageArrived(sessionId, report);
    }
  }

  /// Takes [report] for this conversation: the newest by revision wins, and the spend is kept only when it is this one's.
  void _usageArrived(String? named, UsageReport report) {
    final mine = sessionId;
    final own =
        report.spend != null &&
        (named == mine || report.spend!.sessionId == mine);
    if (own) _spend = report.spend;
    final held = _usageReport;
    if (held != null && held.revision > report.revision) {
      notifyListeners();
      return;
    }
    _usageReport = report.supported ? report.withSpend(null) : null;
    if (!report.supported) _reportsUsage = false;
    notifyListeners();
  }

  // Applying what the agent sends.

  /// What the agent said when it opened this conversation: its options, commands and the notices a host lists.
  void applyOpened(AcpOpenedSession opened) {
    _setOptions(opened.raw['configOptions'], opened.raw['modes']);
    _commands =
        parseCommands(opened.raw['availableCommands'], limits) ?? _commands;
    final key = hostNoticesKey;
    final meta = opened.raw['_meta'];
    if (key != null && meta is Map && meta[key] is List) {
      // Any agent can send these, not only the host, so each is shown as the agent's word, never as the chat's.
      for (final notice in (meta[key] as List).take(limits.hostNotices)) {
        final text = shortText(notice, limits);
        if (text != null) _note('The agent says: $text');
      }
    }
    notifyListeners();
  }

  /// Opens this conversation again through [open], which asks the agent to send it. When the agent sends the conversation, it
  /// replaces what is held, so nothing is there twice; when it only resumes, what is held stays. What arrives while it does is
  /// history, not news, and a tool line its earlier run left waiting or running is marked stopped, since that run ended with its
  /// process. The mode the conversation was left in ([keptMode], [keptBypass]) is put back before anything is sent.
  Future<AcpOpenedSession> reopen(
    Future<AcpOpenedSession> Function() open, {
    String? keptMode,
    bool keptBypass = false,
  }) async {
    if (_opening) throw StateError('The conversation is being opened already');
    // A turn running now is over with the process that held it: it ends here, and a queue behind it waits for the person.
    final ending = currentTurn;
    if (ending != null) _closeTurn(TurnOutcome.cancelled);
    if (ending != null && _queue.isNotEmpty) {
      _queueHeld ??= 'the turn was stopped';
    }
    if (_status == ChatStatus.working) _status = ChatStatus.connected;
    _deferredEnd = null;
    _v1Awaiting.clear();
    final wasHeld = _queueHeld != null;
    // What the agent sends is history, not news.
    _replaying = true;
    final held = (
      messages: List.of(_messages),
      tools: Map.of(_toolEntries),
      compactions: Map.of(_compactions),
      terminals: Map.of(_terminals),
      named: Set.of(_attachmentsNamed),
      turns: List.of(_turns),
      plan: _plan,
    );
    void restore() {
      _messages
        ..clear()
        ..addAll(held.messages);
      _toolEntries
        ..clear()
        ..addAll(held.tools);
      _compactions
        ..clear()
        ..addAll(held.compactions);
      _terminals
        ..clear()
        ..addAll(held.terminals);
      _attachmentsNamed
        ..clear()
        ..addAll(held.named);
      _turns
        ..clear()
        ..addAll(held.turns);
      _plan = held.plan;
    }

    _messages.clear();
    _heldFirst.clear();
    _toolEntries.clear();
    _compactions.clear();
    _terminals.clear();
    _attachmentsNamed.clear();
    _turns.clear();
    _plan = const [];
    _replaying = true;
    _opening = true;
    _bypass = keptBypass;
    _chosenMode = keptMode;
    var opened0 = false;
    String? modeHold;
    try {
      final AcpOpenedSession opened;
      try {
        opened = await open();
      } catch (_) {
        restore();
        rethrow;
      }
      if (!opened.replayed) restore();
      applyOpened(opened);
      for (final entry in _toolEntries.values) {
        entry.tool?.stop();
      }
      if (!opened.replayed) {
        _note('The agent resumed this without sending what was said before.');
      }
      if (keptMode != null) {
        final option = modeOptionIn(_options);
        final id = option?.id ?? OptionRole.mode.name;
        _pending[id] = PendingChoice(id, OptionRole.mode, keptMode);
      }
      try {
        await _applyPending();
        final now = modeOptionIn(_options)?.current;
        if (keptMode != null && keptMode != 'custom' && now != keptMode) {
          // The agent is in another mode than the one it was left in, so what waits is the person's to send.
          _refusedModes.add(keptMode);
          _chosenMode = now;
          modeHold =
              'the mode it was left in (${shortText(keptMode, limits) ?? keptMode}) was not taken';
        }
      } catch (error) {
        _note(
          'The mode it was left in could not be put back: ${reasonOf(error)}',
        );
        _chosenMode = modeOptionIn(_options)?.current;
        modeHold = 'the mode it was left in could not be put back';
      }
      opened0 = true;
      return opened;
    } finally {
      _replaying = false;
      _opening = false;
      _keepMode();
      // Messages written while it opened go now, after the kept mode is back; if it did not open they wait for the person.
      if (_queue.isNotEmpty) {
        if (!opened0) {
          _queueHeld = 'the conversation could not be opened';
        } else if (modeHold != null) {
          _queueHeld = modeHold;
        } else if (!wasHeld) {
          _queueHeld = null;
          _scheduleQueue();
        }
      }
      notifyListeners();
    }
  }

  void _setOptions(Object? configOptions, Object? modes) {
    final parsed = parseConfigOptions(configOptions, limits);
    final mode = parseModes(modes, limits);
    if (parsed != null && parsed.isNotEmpty) {
      // An agent with both says the same thing twice, and ACP prefers the options. A replay carries both, apart.
      _options = _replaying && mode != null ? [...parsed, mode] : parsed;
    } else if (mode != null) {
      _options = [mode];
    }
  }

  void _setCurrent(String id, String value) {
    _options = [
      for (final option in _options)
        option.id == id ? option.withCurrent(value) : option,
    ];
  }

  ChatMessage _entry(
    MessageRole role,
    String text, {
    String? messageId,
    ToolCall? tool,
    Compaction? compaction,
    String? sentBy,
    DateTime? at,
  }) => ChatMessage._(
    role,
    limits,
    text,
    at ?? _now(),
    messageId: messageId,
    tool: tool,
    compaction: compaction,
    sentBy: sentBy == null ? null : shortText(sentBy, limits),
    turn: currentTurn?.id,
  );

  void _add(ChatMessage message) => _messages.add(message);

  void _note(String text) => _add(_entry(MessageRole.notice, text));

  void _trim() {
    final over = _messages.length - limits.entries;
    if (over <= 0) return;
    for (final gone in _messages.take(over)) {
      if (gone.tool != null) _toolEntries.remove(gone.tool!.id);
      if (gone.compaction != null) _compactions.remove(gone.compaction!.id);
      _unacked.remove(gone);
    }
    _messages.removeRange(0, over);
  }

  void _applyTool(Map<String, dynamic> raw) {
    final id = idText(raw['toolCallId'], limits);
    if (id == null) return;
    var entry = _toolEntries[id];
    if (entry == null) {
      entry = _entry(MessageRole.tool, '', tool: ToolCall(id));
      _toolEntries[id] = entry;
      _add(entry);
    }
    entry.tool!.apply(raw, limits);
  }

  void _applyTerminal(Map<String, dynamic> raw) {
    final id = idText(raw['terminalId'], limits);
    if (id == null) return;
    if (!_terminals.containsKey(id) && _terminals.length >= limits.terminals) {
      // The oldest that has ended goes; one still running never does, and a new one with no room is not kept.
      final gone = _terminals.entries
          .where((entry) => !entry.value.isRunning)
          .map((entry) => entry.key)
          .firstOrNull;
      if (gone == null) return;
      _terminals.remove(gone);
    }
    (_terminals[id] ??= TerminalView(limits)).apply(raw, limits);
  }

  /// A compaction's entry stays where its first update put it, and later ones change it there.
  void _applyCompaction(Map<String, dynamic> raw) {
    final id = idText(raw['compactionId'], limits);
    if (id == null) return;
    var entry = _compactions[id];
    if (entry == null) {
      entry = _entry(
        MessageRole.compaction,
        '',
        compaction: Compaction(id, limits),
      );
      _compactions[id] = entry;
      _add(entry);
    }
    entry.compaction!.apply(raw, limits);
  }

  /// Names what went with a person's message, once per message.
  void _nameAttachments(String? messageId, List<String> names) {
    if (names.isEmpty) return;
    if (messageId != null) {
      if (!_attachmentsNamed.add(messageId)) return;
      while (_attachmentsNamed.length > limits.entries) {
        _attachmentsNamed.remove(_attachmentsNamed.first);
      }
    }
    _note('Sent with this: ${names.join(', ')}');
  }

  void _beginTurn() {
    if (currentTurn != null) return;
    final turn = ChatTurn._(_nextTurn++, _now());
    _turns.add(turn);
    if (_turns.length > limits.turns) {
      _turns.removeRange(0, _turns.length - limits.turns);
    }
    // The prompt belongs to the turn it starts, however much earlier than the agent's word it was written down.
    for (final message in _messages.reversed) {
      if (message._turn != null ||
          (message.role != MessageRole.user &&
              message.role != MessageRole.notice)) {
        break;
      }
      message._turn = turn.id;
    }
  }

  /// Ends the running turn, if one runs, whoever's it is; any steering with it is over too.
  void _closeTurn(TurnOutcome outcome) {
    _steering = false;
    _v1Local = _v1Note = null;
    final turn = currentTurn;
    if (turn == null) return;
    turn
      .._endedAt = _now()
      .._outcome = outcome;
    if (!_replaying) _emit(TurnEvent(turn));
  }

  /// Applies one `session/update` of this conversation. A connection's updates arrive here by themselves; a host that feeds the
  /// chat from a channel of its own calls it.
  void apply(AcpSessionUpdate update) {
    final meta = update.raw['_meta'];
    // A time an update names is believed only when a conversation is being rebuilt.
    _updateAt = _replaying && meta is Map && meta[_atMetaKey] is String
        ? DateTime.tryParse(meta[_atMetaKey] as String)
        : null;
    try {
      _apply(update);
    } finally {
      _updateAt = null;
    }
  }

  /// The child agents (subagents) the agent named, in the order it first did, each with the state, task and result it reported and
  /// its own updates where it reports them. A child's updates are kept apart from this conversation's messages and tool calls.
  List<ChatChild> get children => _childRegistry.all;

  /// The child agent called [id], or null.
  ChatChild? child(String id) => _childRegistry[id];

  void _apply(AcpSessionUpdate update) {
    _applied++;
    final childMark = childMarkOf(update.raw);
    if (childMark != null) {
      _childRegistry.apply(update, childMark);
      notifyListeners();
      return;
    }
    final rendering = renderingOf(update, limits);
    recorder?.record(update, rendering, limits);
    switch (rendering) {
      case Rendering.toolCard:
        _applyTool(update.raw);
      case Rendering.terminal:
        _applyTerminal(update.raw);
      case Rendering.plan:
        final entries = update.kind == 'plan_update'
            ? planItems(update.raw['plan'])
            : update.raw['entries'];
        _plan = parsePlan(entries, limits) ?? _plan;
      case Rendering.notice:
        final title = shortText(update.raw['title'], limits);
        if (title != null) {
          // Any agent can send these, so each is shown as the agent's word, never as the chat's.
          final from = switch (update.raw['severity']) {
            'warning' => 'Warning from the agent',
            'error' => 'Error from the agent',
            _ => 'From the agent',
          };
          final detail = shortText(update.raw['description'], limits);
          _note('$from: $title${detail == null ? '' : '\n$detail'}');
        }
      case Rendering.compaction:
        _applyCompaction(update.raw);
      case Rendering.commands:
        _commands =
            parseCommands(update.raw['availableCommands'], limits) ?? _commands;
      case Rendering.usage:
        _usage = _usage.applied(update.raw, limits);
      case Rendering.choices when update.kind == 'config_option_update':
        final before = modeOptionIn(_options)?.current;
        _setOptions(
          update.raw['configOptions'],
          _replaying ? update.raw['modes'] : null,
        );
        _agentChangedMode(before);
        onOptions?.call();
      case Rendering.choices:
        if (update.raw['currentModeId'] case final String mode) {
          final before = modeOptionIn(_options)?.current;
          for (final option in _options) {
            if (option.isMode) _setCurrent(option.id, mode);
          }
          _agentChangedMode(before);
          onOptions?.call();
        }
      case Rendering.title:
        final title = shortText(update.raw['title'], limits);
        if (title != null && _agentNames) {
          _title = title;
          onTitle?.call(title);
        }
        final meta = update.raw['_meta'];
        if (_replaying && meta is Map && meta[_namesMetaKey] == false) {
          _agentNames = false;
        }
      case Rendering.status:
        _applyState(update);
      case Rendering.message || Rendering.thought:
        _applyMessage(update, rendering);
      case Rendering.unshown:
        break;
    }
    _trim();
    notifyListeners();
  }

  void _applyState(AcpSessionUpdate update) {
    if (update.state == 'running' || update.state == 'requires_action') {
      _status = ChatStatus.working;
      _beginTurn();
    } else if (update.state == 'idle') {
      // An idle state outside a turn ends nothing.
      final ended = _status == ChatStatus.working;
      _status = ChatStatus.connected;
      final outcome = switch (update.reason) {
        'cancelled' => TurnOutcome.cancelled,
        'error' || 'refusal' => TurnOutcome.failed,
        _ => TurnOutcome.completed,
      };
      // A turn the person stopped is not news to them.
      if (ended && outcome != TurnOutcome.cancelled) {
        _needs(AttentionKind.finished);
      }
      // What is said of how it ended is part of the turn that ended, so it is written before the turn is closed.
      if (update.reason == 'cancelled') {
        _note('Stopped.');
      } else if (update.reason == 'error') {
        // The agent's own account of the failure, when it gives one, says more than the fixed sentence.
        final error = update.raw['error'];
        final said = error is Map ? shortText(error['message'], limits) : null;
        _note(
          said == null
              ? 'Agent reported an operation failure.'
              : 'Agent reported an operation failure: $said',
        );
      }
      if (ended) _turnEnded(outcome);
    }
  }

  void _applyMessage(AcpSessionUpdate update, Rendering rendering) {
    final chunk = update.kind.endsWith('_chunk');
    // A v2 message or thought without content patches its other fields and keeps its text; null content clears it.
    final text = !chunk && !update.raw.containsKey('content')
        ? null
        : update.text ?? '';
    final role = update.kind.startsWith('user')
        ? MessageRole.user
        : rendering == Rendering.thought
        ? MessageRole.thought
        : MessageRole.agent;
    final id = update.messageId;
    if (text != null) {
      ChatMessage? existing;
      if (id != null) {
        for (final message in _messages) {
          if (message.messageId == id && message.role == role) {
            existing = message;
            break;
          }
        }
      }
      // A message this chat sent and the agent echoes before it has answered the prompt is the same message.
      if (existing == null && id != null && role == MessageRole.user) {
        final mine = _unacked
            .where((message) => message.text.trim() == text.trim())
            .firstOrNull;
        if (mine != null) {
          _unacked.remove(mine);
          mine._messageId = id;
          existing = mine;
        }
      }
      // A v1 chunk carries no message id: it continues the last message of its role.
      if (existing == null &&
          id == null &&
          chunk &&
          _messages.isNotEmpty &&
          _messages.last.role == role &&
          _messages.last.messageId == null &&
          !_unacked.contains(_messages.last)) {
        existing = _messages.last;
      }
      final meta = update.raw['_meta'];
      final sentBy =
          role == MessageRole.user &&
              meta is Map &&
              meta[_clientMetaKey] is String
          ? shortText(meta[_clientMetaKey], limits)
          : null;
      if (existing != null) {
        if (chunk) {
          existing._text.append(text);
        } else {
          existing._text.set(text);
        }
        existing._sentBy ??= sentBy;
      } else if (text.isNotEmpty) {
        _add(_entry(role, text, messageId: id, sentBy: sentBy));
      }
    }
    // A replayed message carries its attachments as content blocks, which have no text of their own.
    if (role == MessageRole.user) {
      _nameAttachments(id, attachedNames(update.raw['content'], limits));
    }
  }

  // Requests from the agent.

  /// Puts a permission request to the person. Bypass answers one that offers an allow; one past [ChatLimits.pendingPermissions]
  /// is cancelled. The first answer to it, from whichever client, is the one the agent gets.
  Future<AcpDecision> askPermission(AcpPermissionRequest request) {
    final policy = permissionPolicy;
    if (policy == null) return _ask(request, const PermissionReview(), null);
    // Each request waits for the one before it, so one that needs the host's rules read again cannot be passed by one that does
    // not.
    final epoch = _askEpoch;
    final prior = _askTail;
    final gate = Completer<void>();
    final waiter = Completer<AcpDecision>();
    _askTail = gate.future;
    _askWaiters.add(waiter);
    unawaited(() async {
      try {
        await prior;
        await policy.prepare(request);
        // Stop, closing the conversation or a connection that ended while this waited: nothing is answered but cancelled, and
        // what the mode, Bypass and the waiting cards are now is read after this point, not before it.
        if (!waiter.isCompleted) {
          waiter.complete(
            _askEpoch != epoch
                ? const AcpCancel()
                : _ask(
                    request,
                    policy.review(request, forPerson: _isForPerson(request)),
                    policy,
                  ),
          );
        }
      } on Object catch (error, stack) {
        if (!waiter.isCompleted) waiter.completeError(error, stack);
      } finally {
        _askWaiters.remove(waiter);
        gate.complete();
      }
    }());
    return waiter.future;
  }

  /// Ends what the conversation had in hand: a request still waiting on its policy is answered cancelled now, and one that is not
  /// told so by the time it is decided finds it so by the count.
  void _cancelWaiters() {
    _askEpoch++;
    for (final waiter in [..._askWaiters]) {
      if (!waiter.isCompleted) waiter.complete(const AcpCancel());
    }
    _askWaiters.clear();
  }

  Future<AcpDecision> _ask(
    AcpPermissionRequest request,
    PermissionReview review,
    PermissionPolicy? policy,
  ) {
    final title =
        shortText(request.title, limits) ??
        shortText(request.kind, limits) ??
        'A tool';
    final forPerson = _isForPerson(request) || review.forPerson;
    if (_bypass && !forPerson) {
      final allow = bypassAnswer(request.options, onceOnly: bypassOnceOnly);
      if (allow != null) {
        if (request.toolCall != null && review.ownCall) {
          _applyTool(request.toolCall!);
        }
        _note(
          'Bypass allowed: $title${review.label} (${shortText(allow.name, limits) ?? allow.kind}).',
        );
        _trim();
        notifyListeners();
        return Future.value(AcpSelect(allow.optionId));
      }
    }
    if (_pendingPermissions.length >= limits.pendingPermissions) {
      _note(
        'The agent made more requests than the ${limits.pendingPermissions} a conversation holds waiting, so the rest '
        'were answered cancelled.',
      );
      notifyListeners();
      return Future.value(const AcpCancel());
    }
    // ACP has the tool call applied before the prompt shows, so the conversation shows what is being asked about.
    if (request.toolCall != null && review.ownCall) {
      _applyTool(request.toolCall!);
    }
    final decided = forPerson ? null : policy?.answer(request, review);
    if (decided != null) {
      _note(decided.note);
      _trim();
      notifyListeners();
      return Future.value(decided.decision);
    }
    final question = PermissionQuestion._(
      _nextRequest++,
      title,
      shortText(request.kind, limits),
      detail: describeToolInput(request.rawInput, limits)?.trim(),
      reason: shortText(request.description, limits),
      diff: describeToolDiff(request.toolCall, request.rawInput, limits),
      options: List.unmodifiable(request.options),
      bypassPassed: _bypass,
      forPerson: forPerson,
      toolCallId: idText(request.toolCall?['toolCallId'], limits),
      hostData: review.hostData,
      askedAt: _now(),
    );
    _pendingPermissions.add(question);
    _needs(AttentionKind.permission);
    _trim();
    notifyListeners();
    return question._answer.future;
  }

  /// Whether a request is a question or a plan for the person, whatever mode: Bypass leaves it to them.
  static bool _isForPerson(AcpPermissionRequest request) {
    if (request.kind == 'switch_mode') return true;
    final input = request.rawInput;
    return input is Map && input['questions'] is List;
  }

  /// Puts an agent's question to the person. One past [ChatLimits.pendingQuestions] is cancelled.
  Future<AcpElicitationAnswer> askQuestion(AcpElicitation request) {
    if (_pendingQuestions.length >= limits.pendingQuestions) {
      _note(
        'The agent asked more questions than the ${limits.pendingQuestions} a conversation holds waiting, so the rest '
        'were answered cancelled.',
      );
      notifyListeners();
      return Future.value(const AcpElicitCancel());
    }
    final question = AgentQuestion._(_nextRequest++, request, _now());
    _pendingQuestions.add(question);
    _needs(AttentionKind.question);
    notifyListeners();
    return question._answer.future;
  }

  void _settle(PermissionQuestion question, Settled<AcpDecision> outcome) {
    question._outcome = outcome;
    _pendingPermissions.remove(question);
    _settledPermissions.add(question);
    if (_settledPermissions.length > limits.settledRequests) {
      _settledPermissions.removeRange(
        0,
        _settledPermissions.length - limits.settledRequests,
      );
    }
    onPermissionSettled?.call(question);
    _emit(PermissionSettledEvent(question));
  }

  void _settleQuestion(
    AgentQuestion question,
    Settled<AcpElicitationAnswer> outcome,
  ) {
    question._outcome = outcome;
    _pendingQuestions.remove(question);
    _settledQuestions.add(question);
    if (_settledQuestions.length > limits.settledRequests) {
      _settledQuestions.removeRange(
        0,
        _settledQuestions.length - limits.settledRequests,
      );
    }
    _emit(QuestionSettledEvent(question));
  }

  /// Answers [question] with [decision] as client [by]. The first answer settles it, and it answers whether this one did: a
  /// client that comes second, or answers one the connection took away, changes nothing.
  bool settle(PermissionQuestion question, AcpDecision decision, {String? by}) {
    _requireClient(by);
    if (!question.isOpen || !_pendingPermissions.contains(question)) {
      return false;
    }
    _settle(
      question,
      Settled._(Settlement.answered, _now(), by: by, answer: decision),
    );
    question._answer.complete(decision);
    notifyListeners();
    return true;
  }

  /// The oldest waiting request's answer: the agent's first option that allows once, or refuses once. Whether this settled it.
  bool answerPermission({required bool allow, String? by}) {
    if (_pendingPermissions.isEmpty) return false;
    return settle(
      _pendingPermissions.first,
      allow ? const AcpAllow() : const AcpDeny('the person refused'),
      by: by,
    );
  }

  /// A client's choice of one of the agent's own answers to the oldest waiting request. What an "always" answer remembers is the
  /// agent's to keep. Whether this settled it.
  bool choosePermission(AcpPermissionOption option, {String? by}) {
    if (_pendingPermissions.isEmpty) return false;
    final question = _pendingPermissions.first;
    if (!question.options.contains(option)) return false;
    return settle(question, AcpSelect(option.optionId), by: by);
  }

  /// An answer to a request offering nothing a person could choose: it is answered cancelled.
  bool cancelPermission({String? by}) {
    if (_pendingPermissions.isEmpty) return false;
    return settle(_pendingPermissions.first, const AcpCancel(), by: by);
  }

  /// Answers [question], a form, as client [by]; the first answer settles it.
  bool settleQuestion(
    AgentQuestion question,
    AcpElicitationAnswer answer, {
    String? by,
  }) {
    _requireClient(by);
    if (!question.isOpen || !_pendingQuestions.contains(question)) return false;
    _settleQuestion(
      question,
      Settled._(Settlement.answered, _now(), by: by, answer: answer),
    );
    question._answer.complete(answer);
    notifyListeners();
    return true;
  }

  /// The answer to the oldest of the agent's questions.
  bool answerQuestion(AcpElicitationAnswer answer, {String? by}) {
    if (_pendingQuestions.isEmpty) return false;
    return settleQuestion(_pendingQuestions.first, answer, by: by);
  }

  /// Takes away what this conversation was still asking of the person when its connection ended or was replaced, or it was let go:
  /// the requests go, the tool line each was about is marked stopped, and the agent, if it is still there to hear, is told each
  /// was cancelled, so it never waits for an answer nobody can give.
  void _withdraw() {
    _cancelWaiters();
    for (final question in [..._pendingPermissions]) {
      if (question.toolCallId case final id?) _toolEntries[id]?.tool?.stop();
      const answer = AcpCancel();
      _settle(
        question,
        Settled._(Settlement.withdrawn, _now(), answer: answer),
      );
      question._answer.complete(answer);
    }
    for (final question in [..._pendingQuestions]) {
      const answer = AcpElicitCancel();
      _settleQuestion(
        question,
        Settled._(Settlement.withdrawn, _now(), answer: answer),
      );
      question._answer.complete(answer);
    }
  }

  /// Answers everything still waiting: its permission requests with [answer], and its questions cancelled.
  void _endWaiting(AcpDecision answer) {
    _cancelWaiters();
    for (final question in [..._pendingPermissions]) {
      _settle(
        question,
        Settled._(Settlement.cancelled, _now(), answer: answer),
      );
      question._answer.complete(answer);
    }
    for (final question in [..._pendingQuestions]) {
      const cancel = AcpElicitCancel();
      _settleQuestion(
        question,
        Settled._(Settlement.cancelled, _now(), answer: cancel),
      );
      question._answer.complete(cancel);
    }
  }

  // Bypass, modes and options.

  /// The mode shown: the chat's own Bypass, else a choice still waiting, else the agent's mode option.
  String? get mode {
    if (_bypass) return ownBypass;
    final waiting = _pending.values
        .where((choice) => choice.role == OptionRole.mode)
        .firstOrNull;
    return waiting?.value ?? modeOptionIn(_options)?.current;
  }

  /// The agent's own mode, which the chat's Bypass sits on top of: what the agent says it is in now, never a choice waiting.
  String? get underlyingMode => modeOptionIn(_options)?.current;

  /// Whether the conversation is in a mode that asks for nothing: the chat's own Bypass, or the agent's as it says it is in it
  /// now. A choice still waiting is not yet in effect.
  bool get inBypass {
    if (_bypass) return true;
    final value = underlyingMode;
    if (value == null) return false;
    final label = modeOptionIn(
      _options,
    )?.choices.where((choice) => choice.value == value).firstOrNull?.label;
    return isBypassMode(value, label);
  }

  /// Turns the chat's own Bypass on or off. On, it answers every request that offers an allow, those already waiting included; a
  /// request with no allow, a question or a plan is still the person's.
  void setBypass(bool on, {String? by}) {
    _requireClient(by);
    if (_bypass == on || _opening) return;
    _bypass = on;
    if (on) {
      _note(
        'Mode: Bypass. A request that offers to allow is allowed without asking you.',
      );
      _answerWaiting();
    } else {
      _note('Bypass is off. Requests are asked again.');
    }
    _keepMode();
    notifyListeners();
  }

  void _answerWaiting() {
    for (final question in [..._pendingPermissions]) {
      final allow = question.forPerson
          ? null
          : bypassAnswer(question.options, onceOnly: bypassOnceOnly);
      if (allow == null) continue;
      settle(question, AcpSelect(allow.optionId), by: bypassClient);
      _note(
        'Bypass allowed: ${question.title} (${shortText(allow.name, limits) ?? allow.kind}).',
      );
    }
  }

  /// Whether the mode can be changed now: not while the conversation is being opened again and its agent has not loaded it.
  bool get canChangeMode => !_opening;

  /// Runs one change of mode after those before it, whether a menu, a key or a banner asked, so each starts from where the one
  /// before ended; a failure of one leaves the next able to run.
  Future<void> _inMode(Future<void> Function() run) {
    _modesWaiting++;
    // The first runs at once, so it is as direct as it was before they queued; the rest wait for those before them.
    final idle = _modesWaiting == 1;
    Future<void> go() async {
      try {
        await run();
      } finally {
        _modesWaiting--;
      }
    }

    final next = idle ? go() : _modeQueue.then((_) => go());
    _modeQueue = next.then<void>((_) {}, onError: (Object _) {});
    return next;
  }

  /// Puts the conversation in [entry]'s mode. The chat's own Bypass answers every request that offers an allow, those already
  /// waiting included, and sits on the agent's mode, which stays.
  Future<void> chooseMode(ModeEntry entry) =>
      canChangeMode ? _inMode(() => _chooseMode(entry)) : Future.value();

  Future<void> _chooseMode(ModeEntry entry) async {
    if (!entry.selectable) return;
    final wasAnswering = _bypass;
    if (entry.own) {
      _pending.removeWhere((_, choice) => choice.role == OptionRole.mode);
      _bypass = true;
    } else {
      _bypass = false;
      final option = modeOptionIn(_options);
      if (option != null) {
        // Only an agent that refused it is not asked for it again: not one that could not be reached.
        final refused = await choose(option, entry.value);
        if (refused) {
          _refusedModes.add(entry.value);
        } else {
          _chosenMode = entry.value;
        }
      }
    }
    if (_bypass && !wasAnswering) {
      _note(
        'Mode: Bypass. A request that offers to allow is allowed without asking you.',
      );
      _answerWaiting();
    } else if (!_bypass && wasAnswering) {
      _note(
        'Mode: ${shortText(entry.label, limits) ?? entry.value}. Requests are asked again.',
      );
    }
    _keepMode();
    notifyListeners();
  }

  /// Puts the conversation in the next of its modes. Presses queue against the mode last asked for, not the one the agent still
  /// shows, and a mode the agent refused is skipped.
  Future<void> cycleMode() {
    if (!canChangeMode) return Future.value();
    final entries = modeEntries(modeOptionIn(_options), label: modeLabel);
    final next = nextMode(entries, _targetMode ?? mode, skip: _refusedModes);
    if (next == null) return Future.value();
    _targetMode = next.value;
    return _inMode(() => _chooseMode(next)).whenComplete(() {
      if (_targetMode == next.value) _targetMode = null;
    });
  }

  /// Takes the conversation out of Bypass. The chat's own sat on the agent's mode, which holds again, and can be left even while
  /// the conversation is opened again, since requests are then only asked; an agent's own goes to Manual, or the first mode it has
  /// that asks, which waits until the conversation is open.
  Future<void> leaveBypass() => _inMode(() async {
    if (_bypass) {
      _bypass = false;
      _note('Bypass is off. Requests are asked again.');
      _keepMode();
      notifyListeners();
      return;
    }
    if (!canChangeMode) return;
    final entries = [
      for (final entry in modeEntries(modeOptionIn(_options), label: modeLabel))
        if (!entry.bypass && entry.selectable) entry,
    ];
    if (entries.isEmpty) return;
    final preferred =
        entries
            .where((entry) => const {'manual', 'default'}.contains(entry.value))
            .firstOrNull ??
        entries.first;
    await _chooseMode(preferred);
  });

  /// Tells [onModeKept] the mode that was set and whether Bypass is on, to put both back when the conversation is opened again.
  /// The agent's starting default was set by nobody and is not kept.
  void _keepMode() {
    // A conversation whose connection was lost still has its mode kept: the person was told it changed.
    if (sessionId == null || _opening) return;
    onModeKept?.call(_chosenMode, _bypass);
  }

  /// The agent changed its own mode, which is where the conversation now is.
  void _agentChangedMode(String? before) {
    final now = modeOptionIn(_options)?.current;
    if (before == null || now == null || now == before || now == 'custom') {
      return;
    }
    onAgentMode?.call(before, now);
    if (!agentModeIsChosen) return;
    _chosenMode = now;
    _keepMode();
  }

  /// Holds [choice] to be sent to the agent before the next prompt.
  void holdChoice(PendingChoice choice) {
    _pending[choice.id] = choice;
    notifyListeners();
  }

  /// A client's choice of one of the agent's options. It goes to the agent now when it can take it; while a turn runs, or the
  /// conversation is opened again, it waits and goes before the next prompt. [value] null takes back a choice still waiting.
  /// Answers whether the agent refused it: one still waiting, or an agent that could not be reached, has refused nothing.
  Future<bool> choose(ConfigOption option, String? value) async {
    if (value == null) {
      _pending.remove(option.id);
      notifyListeners();
      return false;
    }
    if (_status == ChatStatus.working || _opening || _connection == null) {
      _pending[option.id] = PendingChoice(
        option.id,
        roleOf(option),
        value,
        byPerson: true,
      );
      notifyListeners();
      return false;
    }
    _pending.remove(option.id);
    final refused = await _set(option, value);
    if (_options.where((each) => each.id == option.id).firstOrNull?.current ==
        value) {
      onChoice?.call(roleOf(option), value);
    }
    return refused;
  }

  /// Sets one of the agent's choices. It stays as it was unless the agent agrees; a refusal is said in the conversation, with the
  /// agent's reason.
  Future<bool> _set(ConfigOption option, String value) async {
    final connection = _connection;
    final remote = sessionId;
    if (connection == null || remote == null) return false;
    var refused = false;
    try {
      if (option.isMode) {
        await connection.setMode(remote, value);
        _setCurrent(option.id, value);
      } else {
        final reply = await connection.setConfigOption(
          remote,
          option.id,
          value,
        );
        final told = parseConfigOptions(reply['configOptions'], limits);
        if (told != null && told.isNotEmpty) {
          _options = told;
        } else {
          _setCurrent(option.id, value);
        }
      }
    } on AcpFailure catch (error) {
      final shown = option.choices
          .where((choice) => choice.value == value)
          .firstOrNull
          ?.label;
      _note(
        'The agent did not change ${option.label} to ${shortText(shown ?? value, limits) ?? 'that'}: ${reasonOf(error)}',
      );
      refused = true;
    }
    notifyListeners();
    return refused;
  }

  /// Sends each choice made while a turn ran, and says what the agent would not take. The mode and the model go together; the
  /// effort and the other options follow once the model has answered, since the choices an option offers can depend on the model.
  /// Only a choice a person made is reported through [onChoice].
  Future<void> _applyPending() async {
    if (_pending.isEmpty) return;
    final waiting = [..._pending.values];
    _pending.clear();
    Future<void> send(PendingChoice choice) async {
      // `custom` only says what the agent is in: it is never chosen.
      if (choice.role == OptionRole.mode && choice.value == 'custom') return;
      final option =
          _options
              .where(
                (option) =>
                    option.id == choice.id &&
                    option.choices.any((each) => each.value == choice.value),
              )
              .firstOrNull ??
          _options
              .where(
                (option) =>
                    roleOf(option) == choice.role &&
                    option.choices.any((each) => each.value == choice.value),
              )
              .firstOrNull;
      if (option == null) {
        final name = choice.role == OptionRole.other
            ? choice.id
            : choice.role.name;
        _note(
          'The agent offers no $name of ${shortText(choice.value, limits) ?? 'that'} now, so it was not set.',
        );
        return;
      }
      // A mode is sent even when it is the one shown: what shows may not be what is in effect.
      if (option.current != choice.value || choice.role == OptionRole.mode) {
        await _set(option, choice.value);
      }
      final took =
          _options.where((each) => each.id == option.id).firstOrNull?.current ==
          choice.value;
      if (took && choice.role == OptionRole.mode) _chosenMode = choice.value;
      if (took && choice.byPerson) onChoice?.call(choice.role, choice.value);
    }

    bool first(PendingChoice choice) =>
        choice.role == OptionRole.mode || choice.role == OptionRole.model;
    await Future.wait([
      for (final choice in waiting)
        if (first(choice)) send(choice),
    ]);
    await Future.wait([
      for (final choice in waiting)
        if (!first(choice)) send(choice),
    ]);
    _keepMode();
    notifyListeners();
  }

  // Prompts and the queue.

  /// Sends [text] from client [from]. It completes when the agent has been handed the message: under v2 when it has acknowledged
  /// it, under v1, where its answer is the whole turn, when the prompt is under way and was not refused at once. A prompt is never
  /// retried after a disconnect. Written while a turn runs, the message is queued instead, and sent when the turn ends; past
  /// [ChatLimits.queued] this throws.
  Future<void> send(
    String text, {
    List<AcpAttachment> attachments = const [],
    String? from,
    Object? origin,
  }) async {
    if (text.trim().isEmpty && attachments.isEmpty) return;
    _requireClient(from);
    if (_status == ChatStatus.working ||
        _delivering ||
        _opening ||
        _queue.isNotEmpty) {
      _enqueue(
        QueuedMessage._(text, attachments, from: from, origin: origin),
        last: true,
      );
      notifyListeners();
      // A queue waiting on nothing, after a turn that ended as it should, goes on at once.
      if (_status != ChatStatus.working &&
          !_delivering &&
          !_opening &&
          _queueHeld == null) {
        await sendQueued();
      }
      return;
    }
    await _deliver(text, attachments, from);
  }

  /// Puts [message] in the queue. A message that steers is never refused for want of room: it is the one that goes next.
  void _enqueue(QueuedMessage message, {required bool last}) {
    if (last && _queue.length >= limits.queued) {
      throw StateError(
        '${limits.queued} messages already wait for this turn to end',
      );
    }
    if (last) {
      _queue.add(message);
    } else {
      _queue.insert(0, message);
    }
  }

  /// Sends [text] ahead of everything queued and ends the running turn to do it: the agent is stopped, then takes this message
  /// first, and the rest of the queue goes on after it as the turns end. With no turn running it is an ordinary send.
  ///
  /// Completes when the turn has been asked to end, with the message, whose [QueuedMessage.handedOver] says when the agent has
  /// it (null when it went at once). If the agent cannot be asked, the message is not kept and this throws; one that cannot be
  /// sent after the turn ended fails [QueuedMessage.handedOver] and is not kept either.
  Future<QueuedMessage?> steer(
    String text, {
    List<AcpAttachment> attachments = const [],
    String? from,
    Object? origin,
  }) async {
    if (text.trim().isEmpty && attachments.isEmpty) return null;
    _requireClient(from);
    if (_status != ChatStatus.working) {
      if (_delivering || _opening) {
        // No turn runs yet, so there is nothing to end: it goes next, ahead of the rest of the queue.
        final message = QueuedMessage._(
          text,
          attachments,
          from: from,
          origin: origin,
        );
        _enqueue(message, last: false);
        notifyListeners();
        return message;
      }
      await send(text, attachments: attachments, from: from);
      return null;
    }
    final message = QueuedMessage._(
      text,
      attachments,
      from: from,
      origin: origin,
      steered: true,
    );
    _enqueue(message, last: false);
    try {
      await _steerWith();
    } catch (_) {
      _queue.remove(message);
      notifyListeners();
      rethrow;
    }
    return message;
  }

  /// Promotes a message already queued to the front and ends the running turn, as [steer] does.
  Future<void> steerQueued(QueuedMessage message) async {
    if (!_queue.contains(message)) return;
    if (_status != ChatStatus.working) {
      await sendQueued(message);
      return;
    }
    _queue
      ..remove(message)
      ..insert(0, message);
    await _steerWith();
  }

  Future<void> _steerWith() async {
    _steering = true;
    notifyListeners();
    try {
      await cancel();
    } catch (_) {
      // The turn goes on, so the message waits in the queue as any other does.
      _steering = false;
      rethrow;
    }
  }

  /// A turn ended: the next queued message goes, unless the turn did not end as it should, when the queue waits for the person,
  /// who may send it, edit it or clear it. A turn ended to steer is the way the steering message goes.
  void _turnEnded(TurnOutcome outcome) {
    final steered = _steering;
    _closeTurn(outcome);
    if (_reportsUsage) unawaited(refreshUsage());
    // A message is on its way: what follows the end of this turn waits until the agent has taken it.
    if (_delivering) {
      _deferredEnd = (outcome: outcome, steered: steered);
      return;
    }
    _queueAfter(outcome, steered);
  }

  void _queueAfter(TurnOutcome outcome, bool steered) {
    if (_queue.isEmpty) return;
    final held = steered && outcome == TurnOutcome.cancelled
        ? null
        : switch (outcome) {
            TurnOutcome.completed => null,
            TurnOutcome.cancelled => 'the turn was stopped',
            TurnOutcome.failed => 'the turn failed',
          };
    // A message written to steer goes with the turn it ended; one whose turn ended otherwise comes back to the person.
    if (held != null) _failSteered(held);
    _queueHeld = _queue.isEmpty ? null : held;
    // After the update that ended the turn has been dispatched: an answer to the next prompt could otherwise arrive inside it.
    if (held == null) _scheduleQueue();
  }

  void _failSteered(String why) {
    final steering = [
      for (final message in _queue)
        if (message._steered && !message._handed.isCompleted) message,
    ];
    for (final message in steering) {
      _queue.remove(message);
      message._handed.completeError(StateError(why));
    }
  }

  void _scheduleQueue() {
    scheduleMicrotask(
      () => unawaited(
        sendQueued().catchError((Object error) {
          _note('A queued message was not sent: ${reasonOf(error)}');
          notifyListeners();
        }),
      ),
    );
  }

  /// Sends [message], else the first queued message, now, and lets the queue go on after it.
  Future<void> sendQueued([QueuedMessage? message]) async {
    if (_queue.isEmpty ||
        _status == ChatStatus.working ||
        _delivering ||
        _opening) {
      return;
    }
    if (message != null && !_queue.contains(message)) return;
    _queueHeld = null;
    if (message == null && queueDelivery == QueueDelivery.merge) {
      _mergeForDelivery();
    }
    final next = message ?? _queue.first;
    _queue.remove(next);
    notifyListeners();
    try {
      await _deliver(next._text, next._attachments, next.from);
      if (!next._handed.isCompleted) next._handed.complete();
    } catch (error) {
      if (next._steered) {
        // It goes back to the person, not to the head of a queue they did not choose.
        next._handed.completeError(error);
        if (_queue.isNotEmpty) {
          _queueHeld = 'it could not be sent: ${reasonOf(error)}';
        }
        notifyListeners();
        return;
      }
      // Not sent, so it stays first in the queue, which waits for the person.
      if (!_queue.contains(next)) _queue.insert(0, next);
      _queueHeld = 'it could not be sent: ${reasonOf(error)}';
      notifyListeners();
      rethrow;
    }
  }

  /// Sends everything that waits as one prompt, whatever [queueDelivery] says: the messages are joined first, as many as one
  /// prompt holds, and the rest go as turns end.
  Future<void> sendMerged() async {
    if (_queue.isEmpty ||
        _status == ChatStatus.working ||
        _delivering ||
        _opening) {
      return;
    }
    _mergeForDelivery();
    await sendQueued(_queue.first);
  }

  /// Joins the messages at the front of the queue that one prompt can hold: those from the same client, as many as keep within
  /// [ChatLimits.attachments].
  void _mergeForDelivery() {
    while (_queue.length > 1) {
      final first = _queue.first, second = _queue[1];
      if (first.from != second.from || _joined(first, second) == null) return;
      _join(first, second);
    }
  }

  /// The attachments of [above] and [below] together, one of each name, or null when that is more than a message holds or one
  /// of them is a steer waiting for its turn to end, which goes alone.
  List<AcpAttachment>? _joined(QueuedMessage above, QueuedMessage below) {
    if (above._goingFirst || below._goingFirst) return null;
    final names = {for (final item in above._attachments) item.name};
    final all = [
      ...above._attachments,
      for (final item in below._attachments)
        if (names.add(item.name)) item,
    ];
    return all.length > limits.attachments ? null : all;
  }

  void _join(QueuedMessage above, QueuedMessage below) {
    above
      .._attachments = _joined(above, below)!
      .._text = [
        above._text,
        below._text,
      ].where((part) => part.trim().isNotEmpty).join('\n\n');
    _queue.remove(below);
    if (!below._handed.isCompleted) below._handed.complete();
  }

  /// Joins [message] to the one above it, with a blank line between their texts and one of each attachment, and returns the one
  /// they became. Throws [StateError] when the attachments are more than a message holds ([ChatLimits.attachments]) or the two
  /// were written by different clients; nothing is changed then. The first message, or one not queued, is left as it is.
  QueuedMessage? mergeQueued(QueuedMessage message) {
    final index = _queue.indexOf(message);
    if (index < 1) return null;
    final above = _queue[index - 1];
    if (above.from != message.from) {
      throw StateError('They were written by different clients');
    }
    if (above._goingFirst || message._goingFirst) {
      throw StateError('One of them is going first, on its own');
    }
    if (_joined(above, message) == null) {
      throw StateError(
        'A message holds at most ${limits.attachments} attachments',
      );
    }
    _join(above, message);
    notifyListeners();
    return above;
  }

  /// Joins every queued message into the one before it, as [mergeQueued] does, as far as one message can hold them; what does
  /// not fit stays queued after it. Returns how many messages were joined into others.
  int mergeAllQueued() {
    var joined = 0;
    var index = 1;
    while (index < _queue.length) {
      final above = _queue[index - 1], message = _queue[index];
      if (above.from != message.from || _joined(above, message) == null) {
        index++;
        continue;
      }
      _join(above, message);
      joined++;
    }
    if (joined > 0) notifyListeners();
    return joined;
  }

  /// Moves [message] [by] places toward the front (negative) or the back of the queue, to the end it can reach.
  void moveQueued(QueuedMessage message, int by) {
    final from = _queue.indexOf(message);
    if (from < 0 || by == 0) return;
    final to = (from + by).clamp(0, _queue.length - 1);
    if (to == from) return;
    _queue
      ..removeAt(from)
      ..insert(to, message);
    notifyListeners();
  }

  /// Takes [message] out of the queue to be written again, and returns whether it was there. What it said and went with it
  /// are for the host to put back where the person writes.
  bool unqueue(QueuedMessage message) {
    if (!_queue.remove(message)) return false;
    if (!message._handed.isCompleted) message._handed.complete();
    if (_queue.isEmpty) _queueHeld = null;
    notifyListeners();
    return true;
  }

  /// Changes the text of a queued message.
  void editQueued(QueuedMessage message, String text) {
    if (!_queue.contains(message) ||
        (text.trim().isEmpty && message.attachments.isEmpty)) {
      return;
    }
    message._text = text;
    notifyListeners();
  }

  /// Takes one queued message out, or every one.
  void removeQueued([QueuedMessage? message]) {
    final gone = message == null ? List.of(_queue) : [message];
    if (message == null) {
      _queue.clear();
    } else {
      _queue.remove(message);
    }
    for (final one in gone) {
      if (!one._handed.isCompleted && !_queue.contains(one)) {
        one._handed.complete();
      }
    }
    if (_queue.isEmpty) _queueHeld = null;
    notifyListeners();
  }

  Future<void> _deliver(
    String text,
    List<AcpAttachment> attachments,
    String? from,
  ) async {
    final connection = _connection;
    final remote = sessionId;
    if (connection == null || connection.isClosed || remote == null) {
      throw StateError('A connected ACP peer is required');
    }
    _delivering = true;
    try {
      await _handOver(connection, remote, text, attachments, from);
    } catch (error) {
      // What was queued behind it would wait for a turn that is not coming.
      if (_queue.isNotEmpty && _queueHeld == null) {
        _queueHeld = 'it could not be sent: ${reasonOf(error)}';
      }
      rethrow;
    } finally {
      _delivering = false;
      final ended = _deferredEnd;
      _deferredEnd = null;
      if (ended != null) _queueAfter(ended.outcome, ended.steered);
    }
  }

  /// Takes back the message and the note of attachments shown for the v1 prompt under way, when nothing has come of it: the agent
  /// said nothing since, or [force]. Past that the agent may have recorded it, and it stays.
  void _dropV1Prompt({bool force = false}) {
    final local = _v1Local, note = _v1Note;
    final fresh = force || _applied == _v1Before;
    _v1Local = _v1Note = null;
    if (!fresh) return;
    if (local != null) _messages.remove(local);
    if (note != null) _messages.remove(note);
  }

  Future<void> _handOver(
    ChatConnection connection,
    String remote,
    String text,
    List<AcpAttachment> attachments,
    String? from,
  ) async {
    await _applyPending();
    onPrompt?.call(text, attachments);
    final names = [for (final item in attachments) item.name];
    final v1 = connection.protocolVersion == 1;
    ChatTurn? mine;
    if (v1) {
      // A v1 answer arrives when the whole turn ends, so the turn starts now.
      _status = ChatStatus.working;
      _beginTurn();
      mine = currentTurn;
      if (mine != null) _v1Awaiting.add(mine);
    }
    // The person's message goes in before the agent can answer it, however fast the answer comes.
    final shownAt = _heldFirst.firstOrNull?.at;
    releaseFirst(notify: false);
    final local = text.trim().isEmpty
        ? null
        : _entry(MessageRole.user, text, sentBy: from, at: shownAt);
    if (local != null) {
      _add(local);
      if (!v1) _unacked.add(local);
    }
    ChatMessage? note;
    if (v1 && names.isNotEmpty) {
      _note('Sent with this: ${names.join(', ')}');
      note = _messages.last;
    }
    _trim();
    if (v1) {
      _v1Local = local;
      _v1Note = note;
      _v1Before = _applied;
    }
    void rollback() {
      if (mine != null) _v1Awaiting.remove(mine);
      // Nothing came of the prompt, so what was shown for it goes, or a retry would show it twice. A v2 message the agent echoed
      // before refusing is one it recorded, and stays.
      if (v1) {
        _dropV1Prompt(force: true);
        _status = ChatStatus.connected;
        _closeTurn(TurnOutcome.failed);
      } else if (local != null && _unacked.remove(local)) {
        _messages.remove(local);
      }
      notifyListeners();
    }

    notifyListeners();
    if (v1) {
      final Future<String?> turn;
      try {
        turn = connection.prompt(remote, text, attachments: attachments);
      } catch (_) {
        rollback();
        rethrow;
      }
      Object? early;
      var settled = false;
      unawaited(
        turn.then(
          (_) => settled = true,
          onError: (Object error) {
            early = error;
            settled = true;
          },
        ),
      );
      // A prompt that is refused at once is refused here; one that is under way has been handed over.
      await Future<void>.delayed(Duration.zero);
      if (early != null) {
        rollback();
        throw early!;
      }
      if (!settled || identical(_connection, connection)) {
        unawaited(_watchTurn(turn, connection, mine));
      }
      return;
    }
    final String? messageId;
    try {
      final answer = connection.prompt(remote, text, attachments: attachments);
      final wait = limits.handOverSeconds;
      messageId = wait == 0
          ? await answer
          : await answer.timeout(
              Duration(seconds: wait),
              onTimeout: () => throw StateError(
                'the agent did not acknowledge the message within $wait seconds',
              ),
            );
    } catch (_) {
      rollback();
      rethrow;
    }
    if (local != null && _unacked.remove(local) && messageId != null) {
      final other = _messages
          .where(
            (m) =>
                !identical(m, local) &&
                m.messageId == messageId &&
                m.role == MessageRole.user,
          )
          .firstOrNull;
      if (other != null) {
        _messages.remove(local);
        other._sentBy ??= local._sentBy;
      } else {
        local._messageId = messageId;
      }
    }
    _nameAttachments(messageId, names);
    _trim();
    notifyListeners();
  }

  /// Follows a v1 turn to its end, which its connection says by an event for the session, or else by the answer to the prompt.
  Future<void> _watchTurn(
    Future<String?> turn,
    ChatConnection connection,
    ChatTurn? mine,
  ) async {
    Object? failure;
    try {
      await turn;
    } catch (error) {
      failure = error;
    }
    // The event that ends a turn is delivered a moment after the answer is given.
    await Future<void>.delayed(Duration.zero);
    if (mine != null) _v1Awaiting.remove(mine);
    // Only the turn this prompt started: a later one is not ended by an answer that comes after it began.
    if (!identical(_connection, connection) ||
        _status != ChatStatus.working ||
        !identical(currentTurn, mine) ||
        _events.isClosed) {
      return;
    }
    _status = ChatStatus.connected;
    if (failure != null) {
      _dropV1Prompt();
      _note('The prompt failed: ${reasonOf(failure)}');
      _turnEnded(TurnOutcome.failed);
    } else {
      _needs(AttentionKind.finished);
      _turnEnded(TurnOutcome.completed);
    }
    notifyListeners();
  }

  /// Ends the running turn, answering every request still open as ACP requires: cancelled, since the turn cannot end until they
  /// are.
  Future<void> cancel({String? from}) async {
    _requireClient(from);
    final connection = _connection;
    final remote = sessionId;
    if (connection == null || remote == null) {
      throw StateError('No active connected ACP session');
    }
    _endWaiting(const AcpCancel());
    notifyListeners();
    await connection.cancel(remote);
  }

  /// Ends the conversation here: a request still open is answered cancelled. The connection is left to its owner.
  void close() {
    _endWaiting(const AcpCancel());
    detach();
    dispose();
  }

  @override
  void dispose() {
    unawaited(_updatesSub?.cancel());
    unawaited(_eventsSub?.cancel());
    unawaited(_usageSub?.cancel());
    _updatesSub = _eventsSub = _usageSub = null;
    _connection = null;
    // What is still asked is withdrawn, so the agent is not left waiting on a conversation that is gone.
    _withdraw();
    unawaited(_events.close());
    super.dispose();
  }
}
