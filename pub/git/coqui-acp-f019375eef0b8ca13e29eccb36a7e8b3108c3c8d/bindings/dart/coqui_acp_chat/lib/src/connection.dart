import 'dart:async';

import 'package:coqui_acp/coqui_acp.dart';

import 'usage.dart';

/// A report that reached the connection: for [sessionId] when the agent named one, whose spend it then holds.
typedef UsageNotice = ({String? sessionId, UsageReport report});

/// What a chat needs of the channel to its agent, and nothing of how it is made: updates come in, requests go out. The verified
/// peer, the transport and any restart are the host's. Extend it for a channel of your own; [AcpChatConnection] is one over an
/// [AcpClient].
abstract class ChatConnection {
  /// Every `session/update` the agent sends over this connection, for any of its sessions.
  Stream<AcpSessionUpdate> get updates;

  /// What the connection notes beside the updates: the end of a v1 turn, output lost while nobody listened, a request answered
  /// cancelled because it could not be put to a person.
  Stream<AcpEvent> get events;

  /// Whether nothing more can be sent or received.
  bool get isClosed;

  /// The ACP version in use, once initialized.
  int? get protocolVersion;

  /// Sends a prompt. Under v2 it answers with the id of the message the agent inserted; under v1 it answers when the whole turn has
  /// ended. A prompt that was not acknowledged is never retried.
  Future<String?> prompt(
    String sessionId,
    String text, {
    List<AcpAttachment> attachments = const [],
  });

  /// Asks the agent to end the running turn.
  Future<void> cancel(String sessionId);

  /// `session/set_mode`.
  Future<void> setMode(String sessionId, String modeId);

  /// `session/set_config_option`; answers the options the agent then has, when it sends them.
  Future<Map<String, dynamic>> setConfigOption(
    String sessionId,
    String configId,
    String value,
  );

  /// Whether the agent reports the usage of its account. Nothing is shown of usage for one that does not.
  bool get reportsUsage => false;

  /// The reports the agent sends when one changes.
  Stream<UsageNotice> get usageNotices => const Stream.empty();

  /// Asks the agent for its report, with [sessionId]'s spend; null when it has none to give.
  Future<UsageReport?> fetchUsage({String? sessionId}) async => null;

  /// Ends the connection.
  Future<void> close();
}

/// Where a connection puts the requests an agent makes of the person, with the connection they came by: an agent chooses its own
/// session ids, and two agents can choose the same one. [ChatStore] is one.
abstract interface class ChatRequests {
  Future<AcpDecision> askPermission(
    AcpPermissionRequest request,
    ChatConnection from,
  );
  Future<AcpElicitationAnswer> askQuestion(
    AcpElicitation request,
    ChatConnection from,
  );
}

/// A [ChatConnection] over an [AcpClient].
final class AcpChatConnection extends ChatConnection {
  /// Wraps [client], whose `decider` and `elicitor` are the host's own to wire.
  AcpChatConnection(this.client) : _usage = null;

  /// Builds the client with a decider and an elicitor that put each request to [requests] under this connection, which is what
  /// the client needs and cannot name before it exists.
  AcpChatConnection.routed(
    ChatRequests requests,
    AcpClient Function(AcpDecider decider, AcpElicitor elicitor) build, {
    UsageChannel? usage,
  }) : _usage = usage {
    client = build(
      (request) => requests.askPermission(request, this),
      (request) => requests.askQuestion(request, this),
    );
  }

  late final AcpClient client;
  final UsageChannel? _usage;

  @override
  Stream<AcpSessionUpdate> get updates => client.updates;
  @override
  Stream<AcpEvent> get events => client.events;
  @override
  bool get isClosed => client.isClosed;
  @override
  int? get protocolVersion => client.protocolVersion;

  @override
  Future<String?> prompt(
    String sessionId,
    String text, {
    List<AcpAttachment> attachments = const [],
  }) => client.prompt(sessionId, text, attachments: attachments);

  @override
  Future<void> cancel(String sessionId) => client.cancel(sessionId);
  @override
  Future<void> setMode(String sessionId, String modeId) =>
      client.setMode(sessionId, modeId);
  @override
  Future<Map<String, dynamic>> setConfigOption(
    String sessionId,
    String configId,
    String value,
  ) => client.setConfigOption(sessionId, configId, value);

  @override
  bool get reportsUsage => _usage != null;
  @override
  Stream<UsageNotice> get usageNotices =>
      _usage?.notices ?? const Stream.empty();
  @override
  Future<UsageReport?> fetchUsage({String? sessionId}) async =>
      _usage?.fetch(sessionId: sessionId);

  @override
  Future<void> close() => client.close();
}

/// How a host reads the usage extension beside an [AcpClient], which ignores what it does not know. The wire of it, which has to
/// see the messages before the client does, stays with the host.
abstract interface class UsageChannel {
  Stream<UsageNotice> get notices;
  Future<UsageReport?> fetch({String? sessionId});
}
