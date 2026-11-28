import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:markdown/markdown.dart' as md;

import 'entry.dart';
import 'look.dart';
import 'renderers.dart';
import 'selection.dart';
import 'tools.dart';

/// Markdown from an agent, which is untrusted. An image would be fetched as soon as it is drawn, which lets whatever wrote it
/// send data to any address, so only its description is shown. A link is copied rather than opened, so the address is seen first.
///
/// A host adds to it without a fork: [inlineSyntaxes] and [builders] give its own words a meaning (a reference to one of its
/// records, drawn as a link), [onTapLink] says what a link does, and [softLineBreak] keeps the line breaks an agent wrote.
class AgentMarkdown extends StatelessWidget {
  const AgentMarkdown({
    super.key,
    required this.text,
    this.inlineSyntaxes = const [],
    this.builders = const {},
    this.onTapLink,
    this.softLineBreak = false,
  });

  final String text;

  /// Tried before the Markdown parser's own inline syntaxes.
  final List<md.InlineSyntax> inlineSyntaxes;

  /// Draws an element by its tag, besides the package's own code block.
  final Map<String, MarkdownElementBuilder> builders;

  /// What following a link does, with its address and the words it shows; the address is copied, with a note, when null.
  final void Function(String href, String label)? onTapLink;

  /// Whether a line break inside a paragraph stays a line break.
  final bool softLineBreak;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final base = Theme.of(context).textTheme.bodyMedium?.copyWith(fontSize: look.bodySize, height: 1.45);
    final code = base?.copyWith(fontFamily: look.codeFont, fontSize: look.bodySize - 1);
    // The conversation's own selection takes this text in, so it is not a selection of its own.
    // Each block of the Markdown is a line of its own when copied; a list's markers are drawn but not copied.
    return SelectionBlock(child: MarkdownBody(
      data: text,
      bulletBuilder: (parameters) => SelectionContainer.disabled(
        child: Text(
          parameters.style == BulletStyle.orderedList ? '${parameters.index + 1}.' : '•',
          textAlign: parameters.style == BulletStyle.orderedList ? TextAlign.right : TextAlign.center,
          style: base,
        ),
      ),
      builders: {'pre': _CodeBlock(look), ...builders},
      inlineSyntaxes: inlineSyntaxes,
      softLineBreak: softLineBreak,
      styleSheet: _sheetOf(Theme.of(context)).copyWith(
        p: base,
        a: TextStyle(color: look.accent, decoration: TextDecoration.underline, decorationColor: look.accent),
        code: code?.copyWith(backgroundColor: look.code),
        codeblockDecoration: BoxDecoration(color: look.code, borderRadius: BorderRadius.circular(look.radius / 2)),
        blockquoteDecoration: BoxDecoration(
          color: look.notice,
          border: Border(left: BorderSide(color: look.muted, width: 3)),
        ),
      ),
      imageBuilder: (uri, title, alt) => Text(
        alt?.isNotEmpty ?? false ? '[image: $alt]' : '[image]',
        style: TextStyle(color: look.muted),
      ),
      onTapLink: (label, href, title) {
        if (href == null) return;
        if (onTapLink != null) {
          onTapLink!(href, label);
          return;
        }
        Clipboard.setData(ClipboardData(text: href));
        ScaffoldMessenger.maybeOf(context)?.showSnackBar(
          SnackBar(content: Text('Address copied: $href', maxLines: 2, overflow: TextOverflow.ellipsis)),
        );
      },
    ));
  }
}

/// One entry of a conversation, drawn by its kind and the look in use. A host's [ChatRenderers.entry] renderers see it first.
class TranscriptEntry extends StatelessWidget {
  const TranscriptEntry({super.key, required this.entry, this.streaming = false});

  final ChatEntryData entry;
  final bool streaming;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final renderers = ChatScope.renderersOf(context);
    final body = renderers.drawEntry(context, entry) ?? _own(context, look, renderers);
    final toTheRight = entry.role == MessageRole.user && look.userToTheRight;
    return look.column(
      Padding(padding: EdgeInsets.only(bottom: look.gap), child: body),
      toTheRight: toTheRight,
      wide: renderers.wide?.call(entry) ?? false,
    );
  }

  Widget _own(BuildContext context, ChatLook look, ChatRenderers renderers) {
    final tool = entry.tool;
    final compaction = entry.compaction;
    return switch (entry.role) {
      MessageRole.tool => tool == null ? const SizedBox.shrink() : ToolEntry(tool: tool),
      MessageRole.thought => Thought(text: entry.text, initiallyExpanded: streaming),
      MessageRole.compaction => compaction == null ? const SizedBox.shrink() : CompactionView(compaction: compaction),
      MessageRole.agent => MessageBox(
        color: look.agent,
        inBubble: look.agentInBubble,
        copy: entry.text,
        child: AgentMarkdown(text: entry.text),
      ),
      MessageRole.user => MessageBox(
        color: look.user,
        inBubble: look.userPrefix == null,
        copy: entry.text,
        child: Text(
          '${look.userPrefix ?? ''}${entry.text}',
          style: TextStyle(fontSize: look.bodySize, height: 1.45, color: look.userPrefix == null ? null : look.accent),
        ),
      ),
      MessageRole.notice => MessageBox(
        color: look.notice,
        child: Text(entry.text, style: TextStyle(fontSize: look.bodySize - 2, color: look.muted, height: 1.45)),
      ),
    };
  }
}

/// What a message sits in: a bubble, or the page, with a button that copies it.
class MessageBox extends StatelessWidget {
  const MessageBox({super.key, required this.color, required this.child, this.inBubble = true, this.copy});

  final Color color;
  final bool inBubble;
  final Widget child;

  /// What its copy button copies; none without.
  final String? copy;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final body = !inBubble
        ? child
        : Container(
            padding: EdgeInsets.symmetric(horizontal: look.padding + 2, vertical: look.padding - 2),
            decoration: BoxDecoration(
              color: color,
              border: look.bubbleBorder == null ? null : Border.all(color: look.bubbleBorder!),
              borderRadius: BorderRadius.circular(look.radius),
            ),
            child: child,
          );
    if (copy == null) return body;
    return Column(crossAxisAlignment: CrossAxisAlignment.end, mainAxisSize: MainAxisSize.min, children: [
      body,
      CopyButton(text: copy!, tooltip: 'Copy message'),
    ]);
  }
}

/// Copies [text] as it is, and says so.
class CopyButton extends StatelessWidget {
  const CopyButton({super.key, required this.text, this.tooltip = 'Copy'});

  final String text;
  final String tooltip;

  @override
  Widget build(BuildContext context) => SelectionContainer.disabled(
    child: IconButton(
      tooltip: tooltip,
      iconSize: 15,
      visualDensity: VisualDensity.compact,
      color: context.chatLook.muted,
      onPressed: () async {
        await Clipboard.setData(ClipboardData(text: text));
        if (context.mounted) {
          ScaffoldMessenger.maybeOf(context)?.showSnackBar(const SnackBar(content: Text('Copied'), duration: Duration(seconds: 1)));
        }
      },
      icon: const Icon(Icons.copy_rounded),
    ),
  );
}

/// A code block in an agent's Markdown, with a button that copies its code alone.
class _CodeBlock extends MarkdownElementBuilder {
  _CodeBlock(this.look);
  final ChatLook look;

  @override
  bool isBlockElement() => true;

  @override
  Widget? visitElementAfterWithContext(BuildContext context, md.Element element, TextStyle? preferredStyle, TextStyle? parentStyle) {
    var code = element.textContent;
    if (code.endsWith('\n')) code = code.substring(0, code.length - 1);
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.symmetric(vertical: 4),
      decoration: BoxDecoration(color: look.code, borderRadius: BorderRadius.circular(look.radius / 2)),
      child: Stack(children: [
        SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.fromLTRB(10, 10, 40, 10),
          child: Text(code, style: TextStyle(fontFamily: look.codeFont, fontSize: look.bodySize - 1, height: 1.35)),
        ),
        Positioned(top: 0, right: 0, child: CopyButton(text: code, tooltip: 'Copy code')),
      ]),
    );
  }
}

/// An agent's thinking, kept out of the way until asked for, or open while streaming.
class Thought extends StatelessWidget {
  const Thought({super.key, required this.text, this.initiallyExpanded = false});

  final String text;
  final bool initiallyExpanded;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    return Theme(
      data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
      child: ExpansionTile(
        initiallyExpanded: initiallyExpanded,
        dense: true,
        tilePadding: EdgeInsets.zero,
        leading: Icon(Icons.psychology_outlined, size: 18, color: look.muted),
        title: Text('Thinking', style: TextStyle(color: look.muted, fontSize: look.bodySize - 2)),
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: Text(text, style: TextStyle(color: look.muted, fontSize: look.bodySize - 1, height: 1.4)),
          ),
        ],
      ),
    );
  }
}

/// The agent making room in its context. What it kept, its summary, is folded away like a thought.
class CompactionView extends StatelessWidget {
  const CompactionView({super.key, required this.compaction});

  final Compaction compaction;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final said = switch (compaction.status) {
      CompactionStatus.running => 'Compacting the context…',
      CompactionStatus.done => 'Context compacted',
      CompactionStatus.failed => 'Compacting the context failed${compaction.error == null ? '' : ': ${compaction.error}'}',
      CompactionStatus.cancelled => 'Compacting the context was stopped',
    };
    final icon = Icon(Icons.compress, size: 18, color: look.muted);
    final title = Text(said, style: TextStyle(color: look.muted, fontSize: look.bodySize - 2));
    if (compaction.summary.isEmpty) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 8),
        child: Row(children: [icon, const SizedBox(width: 16), Expanded(child: title)]),
      );
    }
    return Theme(
      data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
      child: ExpansionTile(
        dense: true,
        tilePadding: EdgeInsets.zero,
        leading: icon,
        title: title,
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: Text(compaction.summary, style: TextStyle(color: look.muted, fontSize: look.bodySize - 1, height: 1.4)),
          ),
        ],
      ),
    );
  }
}

/// Markdown as the text it shows, a block to a line: a code block is its code, and no mark of the Markdown is left.
String plainMarkdown(String text) {
  final nodes = md.Document(extensionSet: md.ExtensionSet.gitHubFlavored).parseLines(text.split('\n'));
  return [
    for (final node in nodes)
      if (node.textContent.trimRight() case final shown when shown.isNotEmpty) shown,
  ].join('\n');
}

/// A conversation's text in reading order, as copying all of it gives it: each message, notice and tool by what it shows,
/// and a folded thought or summary by its heading.
String conversationText(Iterable<ChatEntryData> entries) => [
  for (final entry in entries)
    switch (entry.role) {
      MessageRole.agent => plainMarkdown(entry.text),
      MessageRole.user || MessageRole.notice => entry.text,
      MessageRole.thought => 'Thinking',
      MessageRole.tool => entry.tool?.title ?? '',
      MessageRole.compaction => 'Context compacted',
    },
].where((text) => text.isNotEmpty).join('\n\n');

/// `MarkdownStyleSheet.fromTheme` for this library's [ThemeData]: flutter_markdown_plus reads the Flutter SDK's own Material
/// theme, which is not the one in the tree, and every field left unset is taken from there.
MarkdownStyleSheet _sheetOf(ThemeData theme) {
  final text = theme.textTheme;
  final body = text.bodyMedium!;
  return MarkdownStyleSheet(
    a: const TextStyle(color: Colors.blue),
    p: body,
    pPadding: EdgeInsets.zero,
    code: body.copyWith(
      backgroundColor: theme.cardTheme.color,
      fontFamily: 'monospace',
      fontSize: body.fontSize! * 0.85,
    ),
    h1: text.headlineSmall,
    h1Padding: EdgeInsets.zero,
    h2: text.titleLarge,
    h2Padding: EdgeInsets.zero,
    h3: text.titleMedium,
    h3Padding: EdgeInsets.zero,
    h4: text.bodyLarge,
    h4Padding: EdgeInsets.zero,
    h5: text.bodyLarge,
    h5Padding: EdgeInsets.zero,
    h6: text.bodyLarge,
    h6Padding: EdgeInsets.zero,
    em: const TextStyle(fontStyle: FontStyle.italic),
    strong: const TextStyle(fontWeight: FontWeight.bold),
    del: const TextStyle(decoration: TextDecoration.lineThrough),
    blockquote: body,
    img: body,
    checkbox: body.copyWith(color: theme.primaryColor),
    blockSpacing: 8.0,
    listIndent: 24.0,
    listBullet: body,
    listBulletPadding: const EdgeInsets.only(right: 4),
    tableHead: const TextStyle(fontWeight: FontWeight.w600),
    tableBody: body,
    tableHeadAlign: TextAlign.center,
    tablePadding: const EdgeInsets.only(bottom: 4.0),
    tableBorder: TableBorder.all(color: theme.dividerColor),
    tableColumnWidth: const FlexColumnWidth(),
    tableCellsPadding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
    tableCellsDecoration: const BoxDecoration(),
    blockquotePadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
    blockquoteDecoration: BoxDecoration(
      color: theme.colorScheme.surfaceContainerHighest,
      borderRadius: BorderRadius.circular(3),
      border: Border(left: BorderSide(color: theme.colorScheme.primary, width: 3)),
    ),
    codeblockPadding: const EdgeInsets.all(8.0),
    codeblockDecoration: BoxDecoration(
      color: theme.cardTheme.color ?? theme.cardColor,
      borderRadius: BorderRadius.circular(2.0),
    ),
    horizontalRuleDecoration: BoxDecoration(
      border: Border(top: BorderSide(width: 5.0, color: theme.dividerColor)),
    ),
  );
}
