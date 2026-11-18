import 'package:coqui_acp_chat/coqui_acp_chat.dart'
    show Compaction, MessageRole, ToolCall;
import 'package:coqui_acp_chat_ui/coqui_acp_chat_ui.dart' show ChatEntryData;

import '../held/held_briefs_page.dart' show revealInvisible;
import 'assistant_controller.dart';
import 'turns.dart';

/// An entry of Outer Office's conversation as the shared chat widgets read it.
///
/// The shared roles are the person's words, the other side's, and a quiet line; every other kind
/// (a proposal, a view, a card, a failure) is a quiet line here and is drawn by a renderer that
/// knows [entry]'s own role. Text a person did not write goes through [revealInvisible], so a
/// character that draws as nothing shows as `<U+XXXX>`.
final class OoEntry implements ChatEntryData {
  OoEntry(this.entry)
    : text = revealInvisible(entry.text),
      at = DateTime.fromMillisecondsSinceEpoch(
        (entry.at * 1000).round(),
        isUtc: true,
      );

  /// The entry as the gateway sent it.
  final AssistantEntry entry;

  @override
  final String text;

  @override
  final DateTime at;

  @override
  MessageRole get role => switch (entry.role) {
    'person' => MessageRole.user,
    'moneypenny' || 'agent' => MessageRole.agent,
    _ => MessageRole.notice,
  };

  @override
  ToolCall? get tool => null;

  @override
  Compaction? get compaction => null;
}

/// The tools one turn used, in one row ("Sources checked (3)").
final class OoSources implements ChatEntryData {
  const OoSources(this.item, this.at);

  final SourcesItem item;

  @override
  final DateTime at;

  @override
  MessageRole get role => MessageRole.notice;

  @override
  String get text => 'Sources checked (${item.tools.length})';

  @override
  ToolCall? get tool => null;

  @override
  Compaction? get compaction => null;
}

/// A line the chat says about itself, such as how many older entries are not shown.
final class OoNote implements ChatEntryData {
  const OoNote(this.text, this.at);

  @override
  final String text;

  @override
  final DateTime at;

  @override
  MessageRole get role => MessageRole.notice;

  @override
  ToolCall? get tool => null;

  @override
  Compaction? get compaction => null;
}

/// [entries] laid out as the chat draws them, oldest first: the tool names of each turn
/// folded into one row ([turnsOf]), and [earlier] older entries noted first.
List<ChatEntryData> ooChatEntries(
  List<AssistantEntry> entries, {
  required bool busy,
  int earlier = 0,
}) {
  final first = entries.isEmpty ? 0.0 : entries.first.at;
  return [
    if (earlier > 0)
      OoNote(
        '$earlier earlier entries are not shown.',
        DateTime.fromMillisecondsSinceEpoch(
          (first * 1000).round(),
          isUtc: true,
        ),
      ),
    for (final item in turnsOf(entries, busy: busy))
      switch (item) {
        EntryItem(:final entry) => OoEntry(entry),
        SourcesItem() => OoSources(
          item,
          DateTime.fromMillisecondsSinceEpoch(
            ((entries.isEmpty ? 0.0 : entries.last.at) * 1000).round(),
            isUtc: true,
          ),
        ),
      },
  ];
}

/// Whether [entry] is a view, which takes the whole width: a table, a chart.
bool isWideEntry(ChatEntryData entry) =>
    entry is OoEntry && entry.entry.role == 'view';
