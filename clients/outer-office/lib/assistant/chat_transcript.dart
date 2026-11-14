import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../shell/oo_markdown.dart';
import '../theme/oo_tokens.dart';
import 'assistant_controller.dart';
import 'turns.dart';

/// [child] kept to [width], centred, where a width is set.
Widget chatColumn(double? width, Widget child) => switch (width) {
  null => child,
  final width => Center(
    child: ConstrainedBox(
      constraints: BoxConstraints(maxWidth: width),
      child: child,
    ),
  ),
};

/// A conversation as the chat draws it, newest at the bottom: [entries] grouped into turns,
/// each tool-using turn's calls folded into one row, and every other entry drawn by
/// [entryBuilder]. Messages keep to a centred column [textWidth] wide where one is set; an
/// entry for which [wide] answers true (a table, a chart) takes the full width.
class ChatTranscript extends StatelessWidget {
  const ChatTranscript({
    required this.entries,
    required this.busy,
    required this.entryBuilder,
    this.textWidth,
    this.wide,
    this.earlier = 0,
    super.key,
  });

  final List<AssistantEntry> entries;

  /// Whether a turn is being answered, so its tools row reads as still checking.
  final bool busy;

  final Widget Function(BuildContext context, AssistantEntry entry)
  entryBuilder;
  final double? textWidth;
  final bool Function(AssistantEntry entry)? wide;

  /// How many older entries the conversation holds that are not in [entries].
  final int earlier;

  @override
  Widget build(BuildContext context) => ListView(
    padding: const EdgeInsets.all(12),
    reverse: true,
    children: [
      // Reversed: the oldest last.
      for (final item in turnsOf(entries, busy: busy).reversed)
        Padding(
          padding: const EdgeInsets.only(bottom: 10),
          child: switch (item) {
            SourcesItem() => chatColumn(textWidth, SourcesChecked(item: item)),
            EntryItem(:final entry) =>
              (wide?.call(entry) ?? false)
                  ? entryBuilder(context, entry)
                  : chatColumn(textWidth, entryBuilder(context, entry)),
          },
        ),
      if (earlier > 0)
        Text(
          '$earlier earlier entries are not shown.',
          textAlign: TextAlign.center,
          style: Theme.of(context).textTheme.bodySmall,
        ),
    ],
  );
}

/// A message of the person's own, or, with [sender], of another person talking in the same
/// conversation. [attachments] sit above the words.
class ChatPersonBubble extends StatelessWidget {
  const ChatPersonBubble({
    required this.text,
    this.attachments,
    this.sender,
    super.key,
  });

  final String text;
  final Widget? attachments;

  /// Who wrote it, when that is not the person reading.
  final String? sender;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final small = Theme.of(context).textTheme.labelSmall
        ?.copyWith(color: tokens.muted);
    return Align(
      alignment: Alignment.centerRight,
      child: Container(
        margin: const EdgeInsets.only(left: 40),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        decoration: BoxDecoration(
          color: tokens.selectedFill,
          border: Border.all(color: tokens.accent.withValues(alpha: 0.5)),
          borderRadius: const BorderRadius.only(
            topLeft: Radius.circular(14),
            topRight: Radius.circular(4),
            bottomLeft: Radius.circular(14),
            bottomRight: Radius.circular(14),
          ),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.end,
          mainAxisSize: MainAxisSize.min,
          spacing: 8,
          children: [
            if (sender case final name?) Text(name, style: small),
            ?attachments,
            Text(revealInvisible(text)),
          ],
        ),
      ),
    );
  }
}

/// What the other side of the conversation said: [avatar], then its words as Markdown in a
/// bubble, then a button that copies them as written.
class ChatAgentBubble extends StatelessWidget {
  const ChatAgentBubble({
    required this.avatar,
    required this.text,
    this.onOpen,
    super.key,
  });

  final Widget avatar;
  final String text;
  final void Function(String link)? onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      spacing: 8,
      children: [
        avatar,
        Expanded(
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            decoration: BoxDecoration(
              color: tokens.raised,
              border: Border.all(color: tokens.border),
              borderRadius: const BorderRadius.only(
                topLeft: Radius.circular(4),
                topRight: Radius.circular(14),
                bottomLeft: Radius.circular(14),
                bottomRight: Radius.circular(14),
              ),
            ),
            child: OoMarkdown(
              text,
              onOpen: onOpen,
              webLinks: false,
              selectable: false,
            ),
          ),
        ),
        IconButton(
          tooltip: 'Copy',
          iconSize: 16,
          visualDensity: VisualDensity.compact,
          onPressed: () =>
              unawaited(Clipboard.setData(ClipboardData(text: text))),
          icon: const Icon(Icons.copy_outlined),
        ),
      ],
    );
  }
}

/// The avatar of a speaker that has no portrait: its [label] (an Agent's codename) in the
/// avatar's rounded square.
class ChatAvatar extends StatelessWidget {
  const ChatAvatar({required this.label, this.size = 32, super.key});

  final String label;
  final double size;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Semantics(
      label: label,
      excludeSemantics: true,
      child: Container(
        width: size,
        height: size,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: tokens.selectedFill,
          border: Border.all(color: tokens.accent.withValues(alpha: 0.5)),
          borderRadius: BorderRadius.circular(size / 4),
        ),
        child: Padding(
          padding: const EdgeInsets.all(3),
          child: FittedBox(
            child: Text(
              label,
              maxLines: 1,
              style: Theme.of(context).textTheme.labelMedium
                  ?.copyWith(color: tokens.accent, fontWeight: FontWeight.w700),
            ),
          ),
        ),
      ),
    );
  }
}

/// A quiet line between messages: something that happened, not something said. A unified
/// [diff] under it is shown line by line.
class ChatNotice extends StatelessWidget {
  const ChatNotice({required this.text, this.diff, super.key});

  final String text;
  final String? diff;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 6,
      children: [
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          spacing: 6,
          children: [
            Icon(Icons.stop_circle_outlined, size: 16, color: colors.outline),
            Flexible(
              child: Text(
                revealInvisible(text),
                style: TextStyle(
                  color: colors.outline,
                  fontStyle: FontStyle.italic,
                ),
              ),
            ),
          ],
        ),
        if (diff case final diff? when diff.isNotEmpty) _UnifiedDiff(diff),
      ],
    );
  }
}

/// A failure, in the error colour.
class ChatErrorLine extends StatelessWidget {
  const ChatErrorLine(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => Text(
    revealInvisible(text),
    style: TextStyle(color: Theme.of(context).colorScheme.error),
  );
}

/// A unified diff as a notice carries it, line by line. It is taller than a chat line, so it
/// scrolls within a bound of its own.
class _UnifiedDiff extends StatelessWidget {
  const _UnifiedDiff(this.diff);

  final String diff;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return ConstrainedBox(
      constraints: const BoxConstraints(maxHeight: 240),
      child: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (final line in const LineSplitter().convert(diff))
              Container(
                color: switch (line) {
                  _ when line.startsWith('+++') || line.startsWith('---') =>
                    null,
                  _ when line.startsWith('+') => colors.tertiaryContainer,
                  _ when line.startsWith('-') => colors.errorContainer,
                  _ => null,
                },
                child: Text(
                  revealInvisible(line),
                  style: const TextStyle(fontFamily: 'monospace'),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// A request to be answered in the conversation: what is asked, [options] to answer with
/// until one is answered, then who answered. [onAnswer] is null while answering is not
/// possible (the connection is gone).
class ChatPermissionCard extends StatelessWidget {
  const ChatPermissionCard({
    required this.request,
    required this.title,
    required this.options,
    required this.onAnswer,
    this.detail,
    this.answeredBy,
    super.key,
  });

  /// The request's ID, which keys its buttons and its answered line.
  final String request;
  final String title;
  final String? detail;
  final List<({String id, String label})> options;
  final void Function(String option)? onAnswer;

  /// Who answered, and with what, once someone did; the options are then off.
  final String? answeredBy;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final answer = answeredBy == null ? onAnswer : null;
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          spacing: 6,
          children: [
            Text(title, style: text.titleSmall),
            if (detail case final detail? when detail.isNotEmpty)
              SelectableText(
                revealInvisible(detail),
                style: text.bodySmall?.copyWith(color: tokens.muted),
              ),
            Wrap(
              spacing: 8,
              runSpacing: 4,
              children: [
                for (final option in options)
                  OutlinedButton(
                    key: ValueKey('permission-$request-${option.id}'),
                    onPressed: answer == null ? null : () => answer(option.id),
                    child: Text(option.label),
                  ),
              ],
            ),
            if (answeredBy case final answered?)
              Text(
                answered,
                key: ValueKey('permission-$request-answered'),
                style: text.bodySmall?.copyWith(color: tokens.muted),
              ),
          ],
        ),
      ),
    );
  }
}

/// The line under a conversation that says what is wrong, with [actions] to put it right.
class ChatProblemLine extends StatelessWidget {
  const ChatProblemLine({
    required this.text,
    this.actions = const [],
    super.key,
  });

  final String text;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
    child: Row(
      children: [
        Expanded(
          child: Semantics(
            liveRegion: true,
            child: Text(
              text,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          ),
        ),
        ...actions,
      ],
    ),
  );
}

/// The box a message is written in, with Send, or Stop while a turn is under way. Without
/// [onStop] the button stays Send.
class ChatComposerBar extends StatelessWidget {
  const ChatComposerBar({
    required this.message,
    required this.focusNode,
    required this.enabled,
    required this.busy,
    required this.hint,
    required this.onSend,
    this.onStop,
    this.stopping = false,
    this.stopTooltip = 'Stop',
    this.leading,
    this.textWidth,
    super.key,
  });

  final TextEditingController message;
  final FocusNode focusNode;
  final bool enabled;
  final bool busy;
  final String hint;
  final VoidCallback onSend;
  final VoidCallback? onStop;
  final bool stopping;
  final String stopTooltip;

  /// A button before the box, such as adding an image.
  final Widget? leading;
  final double? textWidth;

  @override
  Widget build(BuildContext context) => KeyedSubtree(
    key: const ValueKey('composer'),
    child: chatColumn(
      textWidth,
      Padding(
        padding: const EdgeInsets.all(12),
        child: Row(
          children: [
            ?leading,
            Expanded(
              child: TextField(
                controller: message,
                focusNode: focusNode,
                minLines: 1,
                maxLines: 4,
                enabled: enabled,
                decoration: InputDecoration(hintText: hint),
                onSubmitted: (_) => onSend(),
              ),
            ),
            const SizedBox(width: 8),
            if (busy && onStop != null)
              IconButton.filled(
                tooltip: stopping ? 'Stopping…' : stopTooltip,
                onPressed: enabled && !stopping ? onStop : null,
                icon: const Icon(Icons.stop),
              )
            else
              IconButton.filled(
                tooltip: 'Send',
                onPressed: enabled ? onSend : null,
                icon: const Icon(Icons.send),
              ),
          ],
        ),
      ),
    ),
  );
}
