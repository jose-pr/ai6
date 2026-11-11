import 'package:material_ui/material_ui.dart';

import '../theme/oo_tokens.dart';
import 'assistant_controller.dart';

/// What the chat draws for one place in a conversation: an entry, or the tools one turn used
/// folded into a single row.
sealed class TurnItem {
  const TurnItem();
}

/// An entry drawn as it is.
class EntryItem extends TurnItem {
  const EntryItem(this.entry);

  final AssistantEntry entry;
}

/// Every tool call of one turn, in one row. [live] while the turn is still being answered.
class SourcesItem extends TurnItem {
  const SourcesItem(this.tools, {required this.live});

  /// The tool names, one per call, in the order they were called.
  final List<String> tools;
  final bool live;
}

/// Lays [entries] out for reading, oldest first: a turn runs from one message of the person to
/// the next, and the tools it called are one row. A finished turn puts that row after what
/// she said, so the answer leads; a turn still being answered keeps it where the first call
/// was, so progress shows where it began. Failures, proposals, questions and views stay
/// where they came: only the tool names fold.
List<TurnItem> turnsOf(List<AssistantEntry> entries, {required bool busy}) {
  final result = <TurnItem>[];
  var start = 0;
  for (var index = 0; index <= entries.length; index++) {
    final atEnd = index == entries.length;
    if (!atEnd && (entries[index].role != 'person' || index == start)) continue;
    _turn(entries.sublist(start, index), live: busy && atEnd, into: result);
    start = index;
  }
  return result;
}

void _turn(
  List<AssistantEntry> turn, {
  required bool live,
  required List<TurnItem> into,
}) {
  final tools = [
    for (final entry in turn)
      if (entry.role == 'tools') ...entry.text.split(', '),
  ];
  if (tools.isEmpty) {
    into.addAll(turn.map(EntryItem.new));
    return;
  }
  final sources = SourcesItem(tools, live: live);
  var placed = false;
  for (final entry in turn) {
    if (entry.role != 'tools') {
      into.add(EntryItem(entry));
    } else if (live && !placed) {
      into.add(sources);
      placed = true;
    }
  }
  if (!live) into.add(sources);
}

/// The name of a tool as the person reads it: `mcp__moneypenny__held_briefs` is "held briefs".
String toolLabel(String tool) => tool.split('__').last.replaceAll('_', ' ');

/// One collapsed row for the tools a turn used. It says how many lookups there were and
/// opens to the names, each with how many times it was called.
class SourcesChecked extends StatelessWidget {
  const SourcesChecked({required this.item, super.key});

  final SourcesItem item;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final counts = <String, int>{};
    for (final tool in item.tools) {
      counts.update(toolLabel(tool), (count) => count + 1, ifAbsent: () => 1);
    }
    final calls = item.tools.length;
    final title = item.live
        ? 'Checking sources… ($calls)'
        : 'Sources checked ($calls)';
    return Theme(
      data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
      child: ExpansionTile(
        key: ValueKey(item.live ? 'sources-live' : 'sources-checked'),
        dense: true,
        tilePadding: const EdgeInsets.symmetric(horizontal: 8),
        childrenPadding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
        leading: Icon(Icons.fact_check_outlined, size: 18, color: tokens.muted),
        title: Semantics(
          label: item.live
              ? 'Checking sources, $calls lookups so far. Expand to list them.'
              : 'Sources checked, $calls lookups. Expand to list them.',
          excludeSemantics: true,
          child: Text(
            title,
            style: text.bodySmall?.copyWith(color: tokens.muted),
          ),
        ),
        expandedAlignment: Alignment.centerLeft,
        children: [
          Wrap(
            spacing: 6,
            runSpacing: 4,
            children: [
              for (final MapEntry(:key, :value) in counts.entries)
                Chip(
                  visualDensity: VisualDensity.compact,
                  avatar: const Icon(Icons.build_outlined, size: 14),
                  label: Text(value == 1 ? key : '$key ×$value'),
                ),
            ],
          ),
        ],
      ),
    );
  }
}
