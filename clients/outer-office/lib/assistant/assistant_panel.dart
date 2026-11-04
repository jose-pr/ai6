import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../routes/oo_route.dart';
import '../settings/oo_config.dart' show ControlSettings;
import '../shell/oo_markdown.dart';
import '../shell/oo_shell.dart' show OoPage;
import '../theme/oo_theme.dart';
import 'assistant_controller.dart';
import 'assistant_view.dart';
import 'entry_kind.dart';
import 'images.dart';
import 'export.dart' as exporting;
import 'pages.dart';
import 'transcripts.dart';
import 'navigation_card.dart';

/// The chat with Moneypenny: her answers, the tools she used, her views and the changes she
/// proposes, with Confirm and Cancel, and Stop while she answers. Each message carries
/// [route], the app page the person is on, so "this Mission" means something to her. Its menu
/// opens her memory of the person ([onMemory]), with [pageActions] her pages, and exports the
/// conversation. Enter sends; Shift+Enter starts a new line.
class AssistantPanel extends StatefulWidget {
  const AssistantPanel({
    required this.controller,
    this.route,
    this.onOpen,
    this.onMemory,
    this.followAtOnce = false,
    this.onFollowAtOnce,
    this.onClose,
    this.closeTooltip = 'Close Moneypenny',
    this.actions = const [],
    this.message,
    this.composerFocus,
    this.textWidth,
    this.label,
    this.proposalWait = ControlSettings.defaultConfirming,
    this.pageActions,
    this.transcripts,
    this.saveText = exporting.saveText,
    this.imageInput,
    this.imageLoader,
    super.key,
  });

  final AssistantController controller;
  final OoRoute? route;

  /// Opens an app page a view, a link or a navigation card points to
  /// (`app:/missions?operation_id=G`).
  final void Function(String link)? onOpen;

  /// Opens the Memory page; without it the menu does not offer it.
  final VoidCallback? onMemory;

  /// Whether pages she offers open without a click, and where the menu changes that;
  /// without [onFollowAtOnce] the menu does not offer it.
  final bool followAtOnce;
  final ValueChanged<bool>? onFollowAtOnce;

  /// Closes the chat where it fills the screen (a phone); without it there is no button.
  final VoidCallback? onClose;
  final String closeTooltip;

  /// Header buttons after New conversation, such as Expand and Pop out.
  final List<Widget> actions;

  /// The composer's text and focus when they outlive the panel; else the panel's own.
  final TextEditingController? message;
  final FocusNode? composerFocus;

  /// Where set, messages and the composer keep to a centred column this wide, and views
  /// (tables, charts) take the full width.
  final double? textWidth;

  /// A line under the header saying what this conversation is, such as a sample.
  final String? label;

  /// How long a proposal may stay applying before its card says the outcome is unknown
  /// (`ai6.cfg`'s `[oo.controls] confirming`).
  final Duration proposalWait;

  /// What the cards of pages she published ask of Universal Exports; without it a page card
  /// shows her title only.
  final PageActions? pageActions;

  /// Reads the conversation's log and contexts; without it the menu does not offer them.
  final TranscriptReads? transcripts;

  /// Where an export goes: a download in a browser, the Downloads folder on a desktop.
  final Future<String?> Function(String name, String text) saveText;

  /// Where the person pastes and picks images to send; without it, or one that offers
  /// neither, the composer takes none.
  final ImageInput? imageInput;

  /// Reads the images of the conversation, the person's and hers; without it they show as
  /// their names.
  final ImageLoader? imageLoader;

  @override
  State<AssistantPanel> createState() => _AssistantPanelState();
}

class _AssistantPanelState extends State<AssistantPanel> {
  TextEditingController? _ownMessage;

  TextEditingController get _message =>
      widget.message ?? (_ownMessage ??= TextEditingController());

  /// Images added to the message being written.
  List<OutgoingImage> _images = const [];
  StreamSubscription<OutgoingImage>? _pasting;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_showReveal);
    widget.controller.open();
    _pasting = widget.imageInput?.pasted.listen(_add);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_showReveal);
    unawaited(_pasting?.cancel());
    _ownMessage?.dispose();
    super.dispose();
  }

  void _add(OutgoingImage image) {
    if (mounted) setState(() => _images = [..._images, image]);
  }

  Future<void> _pick() async {
    for (final image in await widget.imageInput?.pick() ?? const []) {
      _add(image);
    }
  }

  /// Saves the conversation the app holds as Markdown and says where it went.
  Future<void> _export() async {
    final controller = widget.controller;
    final messenger = ScaffoldMessenger.maybeOf(context);
    final text = exporting.conversationMarkdown(
      conversationId: controller.conversationId,
      entries: controller.entries,
      exportedAt: DateTime.now(),
    );
    try {
      final where = await widget.saveText(
        exporting.exportFileName(controller.conversationId),
        text,
      );
      messenger?.showSnackBar(
        SnackBar(
          content: Text(
            where == null ? 'Conversation exported.' : 'Saved to $where',
          ),
        ),
      );
    } on Object catch (error) {
      messenger?.showSnackBar(
        SnackBar(content: Text('The conversation was not exported: $error')),
      );
    }
  }

  /// Shows a password the person just confirmed resetting, once, outside the conversation.
  void _showReveal() {
    if (!widget.controller.hasReveal) return;
    final secret = widget.controller.takeReveal();
    if (secret == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (context) => AlertDialog(
          title: const Text('New password'),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                'Shown once, only to you. Copy it now; it is not kept anywhere.',
              ),
              const SizedBox(height: 12),
              SelectableText(secret),
            ],
          ),
          actions: [
            TextButton(
              onPressed: () => Clipboard.setData(ClipboardData(text: secret)),
              child: const Text('Copy'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Done'),
            ),
          ],
        ),
      );
    });
  }

  void _send() {
    final text = _message.text;
    if (text.trim().isEmpty ||
        widget.controller.busy ||
        !widget.controller.connected) {
      return;
    }
    widget.controller.say(text, route: widget.route, images: _images);
    _message.clear();
    setState(() => _images = const []);
  }

  /// Enter sends; with Shift it is left to the field, which starts a new line.
  KeyEventResult _composerKey(FocusNode _, KeyEvent event) {
    if (event is! KeyDownEvent ||
        (event.logicalKey != LogicalKeyboardKey.enter &&
            event.logicalKey != LogicalKeyboardKey.numpadEnter) ||
        HardwareKeyboard.instance.isShiftPressed) {
      return KeyEventResult.ignored;
    }
    _send();
    return KeyEventResult.handled;
  }

  /// [child] kept to [AssistantPanel.textWidth], centred, where one is set.
  Widget _column(Widget child) => switch (widget.textWidth) {
    null => child,
    final width => Center(
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: width),
        child: child,
      ),
    ),
  };

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.controller,
    builder: (context, _) {
      final controller = widget.controller;
      final theme = Theme.of(context);
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          LayoutBuilder(
            // A narrow panel gives the portrait's room to the title and the buttons.
            builder: (context, constraints) => Padding(
              padding: const EdgeInsets.fromLTRB(12, 8, 4, 8),
              child: Row(
                children: [
                  if (widget.onClose != null)
                    IconButton(
                      tooltip: widget.closeTooltip,
                      onPressed: widget.onClose,
                      icon: const Icon(Icons.arrow_back),
                    )
                  else if (constraints.maxWidth >= 360)
                    // The title names her; the portrait beside it is decoration.
                    const ExcludeSemantics(child: MoneypennyAvatar(size: 40)),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'Moneypenny',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.titleMedium?.copyWith(
                            fontFamily: ooDisplayFamily,
                            fontWeight: FontWeight.w700,
                            letterSpacing: 1.4,
                            color: OoTokens.of(context).accent,
                          ),
                        ),
                        Text(
                          controller.reconnecting
                              ? 'Reconnecting…'
                              : controller.connecting
                              ? 'Connecting…'
                              : !controller.connected
                              ? 'Not connected'
                              : controller.stopping
                              ? 'Stopping…'
                              : controller.busy
                              ? 'Answering…'
                              : 'Ask about AI6, or ask her to propose a change.',
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.bodySmall,
                        ),
                      ],
                    ),
                  ),
                  IconButton(
                    tooltip: 'New conversation',
                    visualDensity: VisualDensity.compact,
                    onPressed: controller.connected
                        ? controller.startOver
                        : null,
                    icon: const Icon(Icons.add_comment_outlined),
                  ),
                  PopupMenuButton<void>(
                    tooltip: 'Moneypenny menu',
                    style: const ButtonStyle(
                      visualDensity: VisualDensity.compact,
                    ),
                    itemBuilder: (menuContext) => [
                      PopupMenuItem(
                        enabled: controller.entries.isNotEmpty,
                        onTap: () => unawaited(_export()),
                        child: const ListTile(
                          leading: Icon(Icons.download_outlined),
                          title: Text('Export conversation'),
                        ),
                      ),
                      if ((widget.transcripts, controller.conversationId) case (
                        final reads?,
                        final conversation?,
                      ))
                        PopupMenuItem(
                          onTap: () => unawaited(
                            Navigator.of(context).push(
                              MaterialPageRoute<void>(
                                builder: (_) => ConversationLogPage(
                                  reads: reads,
                                  conversation: conversation,
                                ),
                              ),
                            ),
                          ),
                          child: const ListTile(
                            leading: Icon(Icons.receipt_long_outlined),
                            title: Text('Conversation log'),
                          ),
                        ),
                      if (widget.transcripts case final reads?)
                        PopupMenuItem(
                          onTap: () => unawaited(
                            Navigator.of(context).push(
                              MaterialPageRoute<void>(
                                builder: (_) => ConversationsPage(reads: reads),
                              ),
                            ),
                          ),
                          child: const ListTile(
                            leading: Icon(Icons.forum_outlined),
                            title: Text('Logged conversations'),
                          ),
                        ),
                      if (widget.onMemory case final onMemory?)
                        PopupMenuItem(
                          onTap: onMemory,
                          child: const ListTile(
                            leading: Icon(Icons.psychology_outlined),
                            title: Text('Her memory of you'),
                          ),
                        ),
                      if (widget.pageActions case final actions?)
                        PopupMenuItem(
                          onTap: () => unawaited(
                            Navigator.of(context).push(
                              MaterialPageRoute<void>(
                                builder: (_) => HerPagesPage(actions: actions),
                              ),
                            ),
                          ),
                          child: const ListTile(
                            leading: Icon(Icons.web_outlined),
                            title: Text('Her pages'),
                          ),
                        ),
                      if (widget.onFollowAtOnce case final onFollow?)
                        CheckedPopupMenuItem(
                          checked: widget.followAtOnce,
                          onTap: () => onFollow(!widget.followAtOnce),
                          child: const Text("Open Moneypenny's links at once"),
                        ),
                    ],
                  ),
                  ...widget.actions,
                ],
              ),
            ),
          ),
          const Divider(height: 1),
          if (widget.label case final label?)
            Material(
              color: OoTokens.of(context).selectedFill,
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: 12,
                  vertical: 6,
                ),
                child: Row(
                  spacing: 8,
                  children: [
                    Icon(
                      Icons.science_outlined,
                      size: 16,
                      color: OoTokens.of(context).accent,
                    ),
                    Expanded(
                      child: Text(label, style: theme.textTheme.labelMedium),
                    ),
                  ],
                ),
              ),
            ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.all(12),
              reverse: true,
              children: [
                // Reversed: the oldest last.
                for (final entry in controller.entries.reversed)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 10),
                    child: switch (_EntryView(
                      entry: entry,
                      controller: controller,
                      onOpen: widget.onOpen,
                      proposalWait: widget.proposalWait,
                      pageActions: widget.pageActions,
                      imageLoader: widget.imageLoader,
                    )) {
                      // Views are tables and charts: they take the whole width.
                      final view when entry.role == 'view' => view,
                      final other => _column(other),
                    },
                  ),
                if (controller.earlier > 0)
                  Text(
                    '${controller.earlier} earlier entries are not shown.',
                    textAlign: TextAlign.center,
                    style: theme.textTheme.bodySmall,
                  ),
              ],
            ),
          ),
          if (controller.connected &&
              !controller.busy &&
              controller.entries.isEmpty)
            _column(
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 4, 12, 0),
                child: Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final prompt in moneypennyPrompts)
                      ActionChip(
                        label: Text(prompt),
                        onPressed: () =>
                            widget.controller.say(prompt, route: widget.route),
                      ),
                  ],
                ),
              ),
            ),
          if (controller.busy) const LinearProgressIndicator(),
          if (controller.problem != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
              child: Row(
                children: [
                  Expanded(
                    child: Semantics(
                      liveRegion: true,
                      child: Text(
                        controller.problem!,
                        style: TextStyle(color: theme.colorScheme.error),
                      ),
                    ),
                  ),
                  if (!controller.connected && !controller.connecting)
                    TextButton(
                      onPressed: controller.retry,
                      child: const Text('Try again'),
                    ),
                  if (controller.reconnecting)
                    TextButton(
                      onPressed: controller.stopRetrying,
                      child: const Text('Stop trying'),
                    ),
                ],
              ),
            ),
          if (_images.isNotEmpty)
            _column(
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
                child: Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final (index, image) in _images.indexed)
                      ImageThumbnail(
                        key: ValueKey('composer-image-$index'),
                        image: MemoryImage(image.bytes),
                        label: 'Image ${index + 1}',
                        size: 64,
                        onRemove: () => setState(
                          () => _images = [..._images]..removeAt(index),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          _column(
            Padding(
              padding: const EdgeInsets.all(12),
              child: Row(
                children: [
                  if (widget.imageInput?.canPick ?? false)
                    IconButton(
                      tooltip: 'Add an image',
                      onPressed: controller.connected ? _pick : null,
                      icon: const Icon(Icons.add_photo_alternate_outlined),
                    ),
                  Expanded(
                    child: Focus(
                      canRequestFocus: false,
                      skipTraversal: true,
                      onKeyEvent: _composerKey,
                      child: TextField(
                        controller: _message,
                        focusNode: widget.composerFocus,
                        minLines: 1,
                        maxLines: 4,
                        enabled: controller.connected,
                        decoration: const InputDecoration(
                          hintText: 'Ask Moneypenny…',
                        ),
                        onSubmitted: (_) => _send(),
                      ),
                    ),
                  ),
                  const SizedBox(width: 8),
                  if (controller.busy)
                    IconButton.filled(
                      tooltip: controller.stopping ? 'Stopping…' : 'Stop',
                      onPressed: controller.connected && !controller.stopping
                          ? controller.interrupt
                          : null,
                      icon: const Icon(Icons.stop),
                    )
                  else
                    IconButton.filled(
                      tooltip: 'Send',
                      onPressed: controller.connected ? _send : null,
                      icon: const Icon(Icons.send),
                    ),
                ],
              ),
            ),
          ),
        ],
      );
    },
  );
}

class _EntryView extends StatelessWidget {
  const _EntryView({
    required this.entry,
    required this.controller,
    required this.proposalWait,
    this.pageActions,
    this.onOpen,
    this.imageLoader,
  });

  final PageActions? pageActions;
  final Duration proposalWait;
  final AssistantEntry entry;
  final AssistantController controller;
  final void Function(String link)? onOpen;
  final ImageLoader? imageLoader;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final tokens = OoTokens.of(context);
    return switch (entry.role) {
      'person' => Align(
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
              if (_imagesOf(entry) case final images when images.isNotEmpty)
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final (index, sha256) in images.indexed)
                      switch (imageLoader) {
                        final loader? => StoredImage(
                          sha256: sha256,
                          loader: loader,
                          label: 'Your image ${index + 1}',
                        ),
                        null => Chip(
                          avatar: const Icon(Icons.image_outlined, size: 16),
                          label: Text('Image ${index + 1}'),
                        ),
                      },
                  ],
                ),
              Text(revealInvisible(entry.text)),
            ],
          ),
        ),
      ),
      'moneypenny' => Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: 8,
        children: [
          const MoneypennyAvatar(),
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
                entry.text,
                onOpen: onOpen,
                webLinks: false,
                selectable: false,
              ),
            ),
          ),
          // Her words as she wrote them, Markdown included.
          IconButton(
            tooltip: 'Copy',
            iconSize: 16,
            visualDensity: VisualDensity.compact,
            onPressed: () =>
                unawaited(Clipboard.setData(ClipboardData(text: entry.text))),
            icon: const Icon(Icons.copy_outlined),
          ),
        ],
      ),
      'tools' => Wrap(
        spacing: 6,
        children: [
          for (final tool in entry.text.split(', '))
            Chip(
              avatar: const Icon(Icons.build_outlined, size: 16),
              label: Text(_toolName(tool)),
            ),
        ],
      ),
      'proposal' => ProposalCard(
        proposal: entry.data ?? const {},
        onConfirm: controller.confirm,
        onCancel: controller.cancel,
        onOpen: onOpen,
        confirmingAfter: proposalWait,
      ),
      'navigation' => NavigationCard(data: entry.data, onOpen: onOpen),
      'page' when pageActions != null => PageCard(
        data: entry.data,
        actions: pageActions!,
      ),
      'page' => Text(revealInvisible(entry.text)),
      'view' => AssistantViewCard(
        json: entry.data?['view'],
        limits: controller.viewLimits,
        onOpen: onOpen,
        onSubmit: entry.id == null
            ? null
            : (values) => controller.submit(entry.id!, values),
      ),
      'notice' => Column(
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
                  revealInvisible(entry.text),
                  style: TextStyle(
                    color: colors.outline,
                    fontStyle: FontStyle.italic,
                  ),
                ),
              ),
            ],
          ),
          if (entry.data?['diff'] case final String diff when diff.isNotEmpty)
            _UnifiedDiff(diff),
        ],
      ),
      _ => Text(
        revealInvisible(entry.text),
        style: TextStyle(color: colors.error),
      ),
    };
  }
}

/// A unified diff as a notice carries it: what she kept in her memory, line by line. It is taller
/// than a chat line, so it scrolls within a bound of its own.
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

/// The quick questions offered under her greeting on Overview and in an empty chat.
const moneypennyPrompts = [
  'What needs my attention?',
  'Summarise the board',
  'Which Stations are dark?',
];

/// Moneypenny's icon beside her messages, labelled "Moneypenny" for screen readers.
class MoneypennyAvatar extends StatelessWidget {
  const MoneypennyAvatar({this.size = 32, super.key});

  /// The asset declared in `pubspec.yaml`.
  static const asset = 'assets/brand/moneypenny-128.png';

  /// Side in logical pixels; the 128 px asset stays sharp up to 4x at the default 32.
  final double size;

  @override
  Widget build(BuildContext context) => ClipRRect(
    borderRadius: BorderRadius.circular(size / 4),
    child: Image.asset(
      asset,
      width: size,
      height: size,
      filterQuality: FilterQuality.medium,
      semanticLabel: EntryKind.answer.label,
    ),
  );
}

/// [value] with every string in it, keys included, passed through [revealInvisible], so a
/// character that renders as nothing shows as `<U+XXXX>` in what the person confirms.
Object? revealStrings(Object? value) => switch (value) {
  String() => revealInvisible(value),
  List() => [for (final item in value) revealStrings(item)],
  Map() => {
    for (final entry in value.entries)
      revealInvisible('${entry.key}'): revealStrings(entry.value),
  },
  _ => value,
};

/// `mcp__moneypenny__board` as `board`.
String _toolName(String tool) => tool.split('__').last;

/// How a proposal stands, in the words the person reads: an icon and a word, never colour
/// alone. A proposal left applying past [ProposalCard.confirmingAfter] reads as an outcome
/// unknown.
({IconData icon, String word}) proposalStatus(
  String status, {
  bool overdue = false,
}) => switch (status) {
  'confirming' when overdue => (
    icon: Icons.help_outline,
    word: 'Outcome unknown, check the page',
  ),
  'confirming' => (icon: Icons.sync, word: 'Applying'),
  'done' => (icon: Icons.check_circle_outline, word: 'Applied'),
  'failed' => (icon: Icons.error_outline, word: 'Refused'),
  'cancelled' => (icon: Icons.cancel_outlined, word: 'Cancelled'),
  'expired' => (
    icon: Icons.timer_off_outlined,
    word: 'Expired, nothing changed',
  ),
  _ => (icon: Icons.hourglass_empty, word: 'Waiting for you'),
};

/// `missions (G), mission (G/m-12)`: what she read this turn, each tool with the identifiers
/// it was given.
String proposalReads(Object? read) {
  if (read is! List) return '';
  return [
    for (final call in read)
      if (call is Map && call['tool'] is String)
        switch (call['ids']) {
          final Map ids when ids.isNotEmpty =>
            '${call['tool']} (${ids.values.join('/')})',
          _ => '${call['tool']}',
        },
  ].join(', ');
}

/// The page a proposal changes as the person reads it: `Roster › agent 009`; the fields of
/// its route in order after the page.
String proposalAffects(OoRoute route) {
  final values = [for (final key in route.keys) route[key]!];
  return route.page == OoPage.roster
      ? '${route.page.label} › ${values.join(' ')}'
      : [route.page.label, ...values].join(' › ');
}

/// A change Moneypenny proposed: what it is and what it needs, what she had read, the exact
/// request, the page it affects (a link that only opens the page), how it stands and, while
/// it waits, Confirm and Cancel. Confirm answers once: after the first press it is disabled,
/// and a proposal still applying after [confirmingAfter] says its outcome is unknown rather
/// than offering another.
class ProposalCard extends StatefulWidget {
  const ProposalCard({
    required this.proposal,
    required this.onConfirm,
    required this.onCancel,
    this.onOpen,
    this.confirmingAfter = ControlSettings.defaultConfirming,
    super.key,
  });

  final Map<String, Object?> proposal;
  final void Function(String id) onConfirm;
  final void Function(String id) onCancel;

  /// Opens the page the proposal affects, as a link (`app:/<page>?...`).
  final void Function(String link)? onOpen;

  /// How long a proposal may stay applying before the card says the outcome is unknown.
  final Duration confirmingAfter;

  @override
  State<ProposalCard> createState() => _ProposalCardState();
}

class _ProposalCardState extends State<ProposalCard> {
  bool _pressed = false;
  bool _overdue = false;
  Timer? _timer;

  String get _status => widget.proposal['status'] as String? ?? 'pending';

  @override
  void initState() {
    super.initState();
    _watch();
  }

  @override
  void didUpdateWidget(ProposalCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.proposal['status'] != widget.proposal['status']) _watch();
  }

  /// Starts the clock when the proposal is applying and stops it when it is not.
  void _watch() {
    _timer?.cancel();
    if (_status == 'confirming') {
      _timer = Timer(widget.confirmingAfter, () {
        if (mounted) setState(() => _overdue = true);
      });
    } else {
      _overdue = false;
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final proposal = widget.proposal;
    final id = proposal['id'] as String? ?? '';
    final status = _status;
    final request =
        (proposal['request'] as Map?)?.cast<String, Object?>() ?? {};
    final needs = (proposal['needs'] as Map?)?.cast<String, Object?>() ?? {};
    final body = request['body'];
    final result = proposal['result'] as String?;
    final reads = proposalReads(proposal['read']);
    final shown = proposalStatus(status, overdue: _overdue);
    OoRoute? opens;
    if (proposal['opens'] case final String link) {
      try {
        opens = OoRoute.parseUri(link);
      } on FormatException {
        opens = null;
      }
    }
    final open = widget.onOpen;
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          spacing: 6,
          children: [
            Row(
              spacing: 8,
              children: [
                const EntryKindIcon(EntryKind.proposal),
                Expanded(
                  child: Text(
                    'Proposal · needs ${needs['action']} on ${needs['on']}',
                    style: theme.textTheme.labelLarge,
                  ),
                ),
              ],
            ),
            Row(
              children: [
                Expanded(
                  child: Text(
                    revealInvisible(proposal['summary'] as String? ?? ''),
                    style: theme.textTheme.titleSmall,
                  ),
                ),
                const SizedBox(width: 8),
                Chip(
                  key: const ValueKey('proposal-status'),
                  avatar: Icon(shown.icon, size: 16),
                  label: Text(shown.word),
                  visualDensity: VisualDensity.compact,
                ),
              ],
            ),
            if (reads.isNotEmpty)
              Text(
                revealInvisible('Based on: $reads'),
                style: theme.textTheme.bodySmall,
              ),
            SelectableText(
              revealInvisible('${request['method']} ${request['path']}') +
                  (body == null
                      ? ''
                      : '\n${const JsonEncoder.withIndent('  ').convert(revealStrings(body))}'),
              style: const TextStyle(fontFamily: 'monospace'),
            ),
            if (status == 'pending')
              Text(
                'Nothing changes until you confirm.',
                style: theme.textTheme.bodySmall,
              ),
            if (opens != null && open != null)
              Align(
                alignment: AlignmentDirectional.centerStart,
                child: ActionChip(
                  key: const ValueKey('proposal-opens'),
                  avatar: const Icon(Icons.open_in_new, size: 16),
                  label: Text(
                    status == 'done'
                        ? 'Open'
                        : 'Affects: ${revealInvisible(proposalAffects(opens))}',
                  ),
                  tooltip:
                      'Opens ${proposalAffects(opens)}; it changes nothing',
                  onPressed: () => open(opens!.toUri()),
                  visualDensity: VisualDensity.compact,
                ),
              ),
            if (result != null) Text(revealInvisible(result)),
            if (status == 'pending')
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                spacing: 8,
                children: [
                  TextButton(
                    onPressed: () => widget.onCancel(id),
                    child: const Text('Cancel'),
                  ),
                  FilledButton(
                    onPressed: _pressed
                        ? null
                        : () {
                            setState(() => _pressed = true);
                            widget.onConfirm(id);
                          },
                    child: const Text('Confirm'),
                  ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

/// The digests of the images a person's message carried, as its entry's `data.images` lists
/// them.
List<String> _imagesOf(AssistantEntry entry) => [
  for (final image in entry.data?['images'] as List? ?? const [])
    if (image case {'sha256': final String sha256}) sha256,
];
