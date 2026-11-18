import 'package:flutter/foundation.dart' show immutable;

/// One thing the composer offers after `/`: a skill of Moneypenny's catalog, or a command her
/// harness announced.
@immutable
class ChatCommand {
  const ChatCommand({
    required this.name,
    this.description = '',
    this.argumentHint,
    this.source = 'skill',
  });

  /// What is typed after the `/`.
  final String name;
  final String description;

  /// What follows the name, such as `[period]`.
  final String? argumentHint;

  /// `skill` or `harness`.
  final String source;

  /// A command from `GET /v1/user/moneypenny/commands`, or null for one without a name.
  static ChatCommand? fromJson(Object? json) {
    if (json is! Map) return null;
    final name = json['name'];
    if (name is! String || name.isEmpty) return null;
    final description = json['description'];
    final hint = json['argument_hint'];
    final source = json['source'];
    return ChatCommand(
      name: name,
      description: description is String ? description : '',
      argumentHint: hint is String && hint.isNotEmpty ? hint : null,
      source: source is String ? source : 'skill',
    );
  }
}

/// Where the composer reads the commands from: the gateway, or a fixed sample in the preview.
abstract interface class ChatCommands {
  /// The commands to offer, those of [conversation]'s harness included when it is named.
  Future<List<ChatCommand>> load({String? conversation});
}

/// The first word of [text] after its `/`, when it opens with one: the name the person typed.
String? typedCommand(String text) {
  final trimmed = text.trimLeft();
  if (!trimmed.startsWith('/')) return null;
  final end = trimmed.indexOf(RegExp(r'\s'));
  final name = trimmed.substring(1, end < 0 ? trimmed.length : end);
  return name.isEmpty ? null : name;
}
