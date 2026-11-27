/// What a provider refused: a `DOMException`'s `NotAllowedError`, `NotFoundError` or `NetworkError`.
final class AcpPageFailure implements Exception {
  const AcpPageFailure(this.name, this.message);
  final String name;
  final String message;
  @override
  String toString() => '$name: $message';
}

/// An agent this origin was granted.
abstract interface class AcpPageAgent {
  /// Opaque and stable, and different for each origin.
  String get id;

  /// What the person calls it, or what it advertised. Trust it as far as any other text.
  String get label;

  /// The peer id it was added with, if any.
  String? get peer;

  /// Whether the messages are protected by what a proof of that key established.
  bool get verified;
}

/// A connection of one complete JSON-RPC message per string, which `AcpClient` takes as it is.
abstract interface class AcpPageConnection {
  /// The agent, as it stands now that the provider has connected.
  AcpPageAgent get agent;
  Stream<String> get messages;
  Future<void> send(String message);
  Future<void> close();

  /// Why it ended: `page`, `agent`, `navigation`, `revoked` or `failed`.
  Future<String> get closed;
}

/// The provider registry, `document.acp`.
abstract interface class AcpPage {
  /// What this origin was granted. Never prompts.
  Future<List<AcpPageAgent>> agents();

  /// Asks the person, in the provider's own window. Call it from a user action, or it is refused.
  Future<List<AcpPageAgent>> request({
    String? purpose,
    List<String> peers,
    List<String> mcpOrigins,
    bool permissions,
  });

  /// Opens a connection, never prompting. It belongs to this document: navigating closes it.
  Future<AcpPageConnection> connect(AcpPageAgent agent);

  /// Gives up this origin's grant of an agent, closing its connections.
  Future<void> revoke(AcpPageAgent agent);
}
