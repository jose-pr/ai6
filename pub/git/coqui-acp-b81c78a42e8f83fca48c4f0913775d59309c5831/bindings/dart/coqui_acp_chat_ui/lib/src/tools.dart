import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:flutter/material.dart';

import 'diff.dart';
import 'look.dart';
import 'renderers.dart';

IconData toolIcon(String? kind) => switch (kind) {
  'read' => Icons.description_outlined,
  'edit' => Icons.edit_outlined,
  'delete' => Icons.delete_outline,
  'move' => Icons.drive_file_move_outline,
  'search' => Icons.search,
  'execute' => Icons.terminal,
  'fetch' => Icons.public,
  'think' => Icons.psychology_outlined,
  _ => Icons.build_outlined,
};

/// A tool call as the conversation shows it: a host's [ChatRenderers.tool] renderer for its kind, else [ToolCard].
class ToolEntry extends StatelessWidget {
  const ToolEntry({super.key, required this.tool});

  final ToolCall tool;

  @override
  Widget build(BuildContext context) {
    final renderers = ChatScope.renderersOf(context);
    final own = tool.kind == null ? null : renderers.tool[tool.kind];
    return own?.call(context, tool) ?? ToolCard(tool: tool);
  }
}

/// A tool the agent is using: what, how it went, and what it changed. A failure opens by itself, since that is what a person came
/// to see, and the rest stay closed to keep a long turn readable.
class ToolCard extends StatelessWidget {
  const ToolCard({super.key, required this.tool});

  final ToolCall tool;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final (statusIcon, statusColor, statusLabel) = switch (tool.status) {
      ToolStatus.pending => (Icons.schedule, look.muted, 'waiting'),
      ToolStatus.running => (Icons.autorenew, look.accent, 'running'),
      ToolStatus.done => (Icons.check_circle_outline, look.accent, 'done'),
      ToolStatus.failed => (Icons.error_outline, look.danger, 'failed'),
      ToolStatus.cancelled => (Icons.block, look.muted, 'cancelled'),
      ToolStatus.stopped => (Icons.stop_circle_outlined, look.muted, 'stopped'),
    };
    final hasDetail = tool.blocks.isNotEmpty || tool.locations.isNotEmpty;
    final header = Row(children: [
      Icon(toolIcon(tool.kind), size: 18, color: look.muted),
      const SizedBox(width: 8),
      Expanded(child: Text(tool.title, maxLines: 2, overflow: TextOverflow.ellipsis, style: TextStyle(fontSize: look.bodySize - 1))),
      const SizedBox(width: 8),
      if (tool.status == ToolStatus.stopped) ...[
        Text('stopped', style: TextStyle(fontSize: look.bodySize - 2, color: look.muted)),
        const SizedBox(width: 4),
      ],
      Tooltip(message: statusLabel, child: Icon(statusIcon, size: 16, color: statusColor, semanticLabel: statusLabel)),
    ]);
    return Container(
      decoration: BoxDecoration(
        border: Border.all(color: look.muted.withValues(alpha: 0.3)),
        borderRadius: BorderRadius.circular(look.radius / 1.5),
      ),
      clipBehavior: Clip.antiAlias,
      child: !hasDetail
          ? Padding(padding: EdgeInsets.all(look.padding), child: header)
          : Theme(
              data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
              child: ExpansionTile(
                // Rebuilt as the status changes, so a tool that fails while being watched opens.
                key: ValueKey('${tool.id}-${tool.status == ToolStatus.failed}'),
                initiallyExpanded: tool.status == ToolStatus.failed,
                dense: true,
                tilePadding: EdgeInsets.symmetric(horizontal: look.padding),
                childrenPadding: EdgeInsets.fromLTRB(look.padding, 0, look.padding, look.padding),
                expandedCrossAxisAlignment: CrossAxisAlignment.stretch,
                title: header,
                children: [
                  for (final place in tool.locations)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 4),
                      child: Text(place, style: TextStyle(fontFamily: look.codeFont, fontSize: 12, color: look.muted)),
                    ),
                  for (final block in tool.blocks) ToolBlockView(tool: tool, block: block),
                ],
              ),
            ),
    );
  }
}

/// One piece of a tool call's content: a host's [ChatRenderers.block] renderer first, then text, a diff, a patch or a terminal.
class ToolBlockView extends StatelessWidget {
  const ToolBlockView({super.key, required this.tool, required this.block});

  final ToolCall tool;
  final ToolBlock block;

  @override
  Widget build(BuildContext context) {
    final scope = ChatScope.maybeOf(context);
    final own = scope?.renderers.drawBlock(context, tool, block);
    if (own != null) return own;
    return switch (block) {
      ToolText(:final text) => CodeBox(text: text),
      ToolDiff(:final path, :final oldText, :final newText) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(path, style: TextStyle(fontFamily: context.chatLook.codeFont, fontSize: 12, color: context.chatLook.muted)),
          const SizedBox(height: 4),
          DiffView(lines: withContext(diffLines(oldText, newText))),
        ],
      ),
      ToolPatch(:final text) => DiffView(lines: patchLines(text)),
      ToolTerminal(:final terminalId) => TerminalBlock(view: scope?.terminals[terminalId]),
    };
  }
}

/// Lines of a change, an added one on the look's `added` and a removed one on its `removed`.
class DiffView extends StatelessWidget {
  const DiffView({super.key, required this.lines});

  /// Null marks lines left out.
  final List<DiffLine?> lines;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final style = TextStyle(fontFamily: look.codeFont, fontSize: 12, height: 1.35);
    return Container(
      constraints: const BoxConstraints(maxHeight: 420),
      decoration: BoxDecoration(color: look.code, borderRadius: BorderRadius.circular(look.radius / 2)),
      child: SingleChildScrollView(
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (final line in lines)
                if (line == null)
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                    child: Text('⋯', style: style.copyWith(color: look.muted)),
                  )
                else
                  Container(
                    color: switch (line.kind) {
                      DiffKind.added => look.added,
                      DiffKind.removed => look.removed,
                      DiffKind.same => null,
                    },
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                    child: Text(
                      '${switch (line.kind) { DiffKind.added => '+', DiffKind.removed => '-', DiffKind.same => ' ' }} ${line.text}',
                      style: style,
                      softWrap: false,
                    ),
                  ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Text in the look's code font, in a box that scrolls past a height.
class CodeBox extends StatelessWidget {
  const CodeBox({super.key, required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    return Container(
      constraints: const BoxConstraints(maxHeight: 320),
      width: double.infinity,
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(color: look.code, borderRadius: BorderRadius.circular(look.radius / 2)),
      child: SingleChildScrollView(
        child: Text(text, style: TextStyle(fontFamily: look.codeFont, fontSize: 12, height: 1.35)),
      ),
    );
  }
}

/// A command and what it printed, with its exit code when it failed.
class TerminalBlock extends StatelessWidget {
  const TerminalBlock({super.key, required this.view});

  final TerminalView? view;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final view = this.view;
    if (view == null) return const SizedBox.shrink();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (view.command != null)
          Text('\$ ${view.command}', style: TextStyle(fontFamily: look.codeFont, fontSize: 12, color: look.accent)),
        if (view.output.isNotEmpty) Padding(padding: const EdgeInsets.only(top: 4), child: CodeBox(text: view.output)),
        if (view.exitCode != null && view.exitCode != 0)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text('exit ${view.exitCode}', style: TextStyle(color: look.danger, fontSize: 12)),
          ),
      ],
    );
  }
}
