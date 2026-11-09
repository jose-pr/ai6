import 'package:material_ui/material_ui.dart';

import '../theme/oo_theme.dart';

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

/// The word being typed after a `/` that opens the message, or null when the text is not
/// that: no `/` first, or the cursor is past the first word (the arguments).
String? commandQuery(String text, int cursor) {
  if (!text.startsWith('/')) return null;
  final end = text.indexOf(RegExp(r'\s'));
  final wordEnd = end < 0 ? text.length : end;
  if (cursor < 1 || cursor > wordEnd) return null;
  return text.substring(1, wordEnd);
}

/// The first word of [text] after its `/`, when it opens with one: the name the person typed.
String? typedCommand(String text) {
  final trimmed = text.trimLeft();
  if (!trimmed.startsWith('/')) return null;
  final end = trimmed.indexOf(RegExp(r'\s'));
  final name = trimmed.substring(1, end < 0 ? trimmed.length : end);
  return name.isEmpty ? null : name;
}

/// [commands] that match [query]: names that start with it first, then names and
/// descriptions that contain it, without regard to case; all of them for an empty query.
List<ChatCommand> matchingCommands(List<ChatCommand> commands, String query) {
  final wanted = query.toLowerCase();
  if (wanted.isEmpty) return commands;
  final starts = <ChatCommand>[];
  final contains = <ChatCommand>[];
  for (final command in commands) {
    final name = command.name.toLowerCase();
    if (name.startsWith(wanted)) {
      starts.add(command);
    } else if (name.contains(wanted) ||
        command.description.toLowerCase().contains(wanted)) {
      contains.add(command);
    }
  }
  return [...starts, ...contains];
}

/// [text] with its first word replaced by `/name ` and the cursor after it.
({String text, int cursor}) insertCommand(String text, String name) {
  final end = text.indexOf(RegExp(r'\s'));
  final rest = end < 0 ? '' : text.substring(end).trimLeft();
  final head = '/$name ';
  return (text: '$head$rest', cursor: head.length);
}

/// The menu of commands over the composer: each row is the name, its argument hint and what it
/// does; [highlighted] is the row Enter picks.
class CommandMenu extends StatelessWidget {
  const CommandMenu({
    required this.commands,
    required this.highlighted,
    required this.onPick,
    this.maxHeight = 240,
    super.key,
  });

  final List<ChatCommand> commands;
  final int highlighted;
  final ValueChanged<ChatCommand> onPick;
  final double maxHeight;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    return Semantics(
      container: true,
      label: 'Commands',
      child: Material(
        color: tokens.raised,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(8),
          side: BorderSide(color: tokens.border),
        ),
        clipBehavior: Clip.antiAlias,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: maxHeight),
          child: ListView.builder(
            shrinkWrap: true,
            padding: const EdgeInsets.symmetric(vertical: 4),
            itemCount: commands.length,
            itemBuilder: (context, index) {
              final command = commands[index];
              final selected = index == highlighted;
              return InkWell(
                key: ValueKey('command-${command.name}'),
                onTap: () => onPick(command),
                child: Container(
                  color: selected ? tokens.selectedFill : null,
                  padding: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 6,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text.rich(
                        TextSpan(
                          children: [
                            TextSpan(
                              text: '/${command.name}',
                              style: text.bodyMedium?.copyWith(
                                color: tokens.text,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                            if (command.argumentHint != null)
                              TextSpan(
                                text: ' ${command.argumentHint}',
                                style: text.bodyMedium?.copyWith(
                                  color: selected ? tokens.text : tokens.muted,
                                ),
                              ),
                          ],
                        ),
                      ),
                      if (command.description.isNotEmpty)
                        Text(
                          command.description,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: text.bodySmall?.copyWith(
                            color: selected ? tokens.text : tokens.muted,
                          ),
                        ),
                    ],
                  ),
                ),
              );
            },
          ),
        ),
      ),
    );
  }
}
