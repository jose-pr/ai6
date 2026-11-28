import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:material_ui/material_ui.dart';

import 'content.dart';
import 'entry.dart';
import 'fold.dart';
import 'look.dart';
import 'renderers.dart';
import 'tools.dart';

/// A [TranscriptItem] drawn: an entry, a burst folded into a line, or a turn's work folded into one.
class TranscriptItemView extends StatelessWidget {
  const TranscriptItemView({super.key, required this.item});

  final TranscriptItem item;

  @override
  Widget build(BuildContext context) => switch (item) {
    EntryItem(:final entry, :final streaming) => TranscriptEntry(entry: entry, streaming: streaming),
    BurstItem(:final tools) => _Placed(child: ToolBurst(tools: tools)),
    WorkItem(:final items, :final duration, :final turn, :final running) => _Placed(
      child: TurnWork(items: items, duration: duration, turn: turn, running: running),
    ),
  };
}

class _Placed extends StatelessWidget {
  const _Placed({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    return look.column(Padding(padding: EdgeInsets.only(bottom: look.gap), child: child));
  }
}

/// A line that opens into [children]: [label], a chevron, and nothing between the line and what it opens.
class FoldLine extends StatefulWidget {
  const FoldLine({super.key, required this.label, required this.children, this.initiallyOpen = false});

  final Widget label;
  final Widget children;
  final bool initiallyOpen;

  @override
  State<FoldLine> createState() => _FoldLineState();
}

class _FoldLineState extends State<FoldLine> {
  late bool _open = widget.initiallyOpen;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      InkWell(
        borderRadius: BorderRadius.circular(look.radius / 2),
        onTap: () => setState(() => _open = !_open),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 4),
          child: Row(children: [
            Flexible(child: widget.label),
            Icon(_open ? Icons.expand_less : Icons.expand_more, size: 18, color: look.muted),
          ]),
        ),
      ),
      if (_open) Padding(padding: const EdgeInsets.only(top: 6), child: widget.children),
    ]);
  }
}

/// "Edited calc.js, ran a command +1 −1": the words of [summary] and what it added and removed, in the look's colours.
class SummaryText extends StatelessWidget {
  const SummaryText({super.key, required this.summary, this.prefix});

  final BurstSummary summary;

  /// What goes before it, such as "Worked for 1m 29s".
  final String? prefix;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final style = TextStyle(color: look.muted, fontSize: look.bodySize - 1);
    final mono = TextStyle(fontFamily: look.codeFont, fontSize: look.bodySize - 2);
    return Text.rich(
      TextSpan(style: style, children: [
        if (prefix != null) TextSpan(text: prefix, style: const TextStyle(fontWeight: FontWeight.w600)),
        if (prefix != null && summary.text.isNotEmpty) const TextSpan(text: ' · '),
        TextSpan(text: summary.text),
        if (summary.added > 0) TextSpan(text: '  +${summary.added}', style: mono.copyWith(color: look.success)),
        if (summary.removed > 0) TextSpan(text: '  −${summary.removed}', style: mono.copyWith(color: look.danger)),
      ]),
      maxLines: 2,
      overflow: TextOverflow.ellipsis,
    );
  }
}

/// Finished tool calls as one line, which opens into their cards. A host's [ChatRenderers.burst] draws the line itself.
class ToolBurst extends StatelessWidget {
  const ToolBurst({super.key, required this.tools});

  final List<ToolCall> tools;

  @override
  Widget build(BuildContext context) {
    final summary = summarizeBurst(tools);
    final cards = Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      for (final tool in tools) Padding(padding: const EdgeInsets.only(bottom: 8), child: ToolEntry(tool: tool)),
    ]);
    final own = ChatScope.renderersOf(context).burst;
    if (own != null) return own(context, summary, tools, cards);
    return FoldLine(
      key: ValueKey('burst ${tools.first.id}'),
      label: SummaryText(summary: summary),
      children: cards,
    );
  }
}

/// The work of a finished turn as "Worked for 1m 29s", which opens into what the agent did; while the turn [running], "Working"
/// and what it has done so far. A host's [ChatRenderers.work] draws the line of a finished turn itself.
class TurnWork extends StatelessWidget {
  const TurnWork({super.key, required this.items, required this.duration, this.turn, this.running = false});

  final List<TranscriptItem> items;
  final Duration duration;

  /// The turn's id, which keys the line so that it stays open or closed when the turn ends.
  final int? turn;
  final bool running;

  @override
  Widget build(BuildContext context) {
    final tools = WorkItem(items, duration).tools;
    final summary = summarizeBurst(tools);
    final inside = Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      for (final item in items) TranscriptItemView(item: item),
    ]);
    final own = running ? null : ChatScope.renderersOf(context).work;
    if (own != null) return own(context, duration, summary, inside);
    return FoldLine(
      key: ValueKey(turn != null ? 'work turn $turn' : 'work ${tools.firstOrNull?.id ?? items.length}'),
      label: SummaryText(summary: summary, prefix: running ? 'Working' : 'Worked for ${spanLabel(duration)}'),
      children: inside,
    );
  }
}

/// What the agent says it is going to do, as a checklist in the conversation: a step done, the one being worked on, and those to
/// come. A host's [ChatRenderers.plan] draws it instead.
class PlanChecklist extends StatelessWidget {
  const PlanChecklist({super.key, required this.entries});

  final List<PlanEntry> entries;

  @override
  Widget build(BuildContext context) {
    final own = ChatScope.renderersOf(context).plan;
    if (own != null) return own(context, entries);
    final look = context.chatLook;
    final done = entries.where((entry) => entry.status == PlanStatus.done).length;
    return Container(
      key: const Key('plan-checklist'),
      width: double.infinity,
      padding: EdgeInsets.all(look.padding),
      decoration: BoxDecoration(
        border: Border.all(color: look.muted.withValues(alpha: 0.3)),
        borderRadius: BorderRadius.circular(look.radius / 1.5),
      ),
      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text('Plan: $done of ${entries.length}', style: TextStyle(fontSize: look.bodySize - 2, color: look.muted)),
        const SizedBox(height: 6),
        for (final entry in entries)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 3),
            child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Icon(
                switch (entry.status) {
                  PlanStatus.done => Icons.check_circle_outline,
                  PlanStatus.running => Icons.timelapse,
                  PlanStatus.pending => Icons.radio_button_unchecked,
                  PlanStatus.cancelled => Icons.block,
                },
                size: 18,
                color: entry.status == PlanStatus.pending || entry.status == PlanStatus.cancelled ? look.muted : look.accent,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  entry.content,
                  style: TextStyle(
                    fontSize: look.bodySize - 1,
                    color: entry.status == PlanStatus.done || entry.status == PlanStatus.cancelled ? look.muted : null,
                    fontWeight: entry.status == PlanStatus.running ? FontWeight.w600 : null,
                    decoration: entry.status == PlanStatus.cancelled ? TextDecoration.lineThrough : null,
                  ),
                ),
              ),
            ]),
          ),
      ]),
    );
  }
}

/// The plan folded to one line, for a host that keeps it above the composer rather than in the conversation.
class PlanPanel extends StatelessWidget {
  const PlanPanel({super.key, required this.entries});

  final List<PlanEntry> entries;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final done = entries.where((entry) => entry.status == PlanStatus.done).length;
    return Theme(
      data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
      child: ExpansionTile(
        dense: true,
        leading: Icon(Icons.checklist, size: 18, color: look.muted),
        title: Text('Plan: $done of ${entries.length}', style: TextStyle(fontSize: look.bodySize - 2, color: look.muted)),
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 220),
            child: ListView(
              shrinkWrap: true,
              children: [
                for (final entry in entries)
                  ListTile(
                    dense: true,
                    visualDensity: VisualDensity.compact,
                    leading: Icon(
                      switch (entry.status) {
                        PlanStatus.done => Icons.check_circle_outline,
                        PlanStatus.running => Icons.play_circle_outline,
                        PlanStatus.pending => Icons.radio_button_unchecked,
                        PlanStatus.cancelled => Icons.block,
                      },
                      size: 16,
                      color: entry.status == PlanStatus.pending ? look.muted : look.accent,
                    ),
                    title: Text(entry.content, style: TextStyle(fontSize: look.bodySize - 2)),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
