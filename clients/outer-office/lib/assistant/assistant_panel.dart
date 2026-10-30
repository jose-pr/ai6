import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../routes/oo_route.dart';
import '../shell/oo_shell.dart' show OoPage;
import '../theme/oo_theme.dart';
import 'assistant_controller.dart';
import 'assistant_view.dart';
import 'entry_kind.dart';
import 'navigation_card.dart';

/// The chat with Moneypenny: her answers, the tools she used, her views and the changes she
/// proposes, with Confirm and Cancel, and Stop while she answers. Each message carries
/// [route], the app page the person is on, so "this Mission" means something to her. Its menu
/// opens her memory of the person ([onMemory]). Enter sends; Shift+Enter starts a new line.
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

  @override
  State<AssistantPanel> createState() => _AssistantPanelState();
}

class _AssistantPanelState extends State<AssistantPanel> {
  TextEditingController? _ownMessage;

  TextEditingController get _message =>
      widget.message ?? (_ownMessage ??= TextEditingController());

  @override
  void initState() {
    super.initState();
    widget.controller.open();
  }

  @override
  void dispose() {
    _ownMessage?.dispose();
    super.dispose();
  }

  void _send() {
    final text = _message.text;
    if (text.trim().isEmpty ||
        widget.controller.busy ||
        !widget.controller.connected) {
      return;
    }
    widget.controller.say(text, route: widget.route);
    _message.clear();
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
                          !controller.connected
                              ? 'Connecting…'
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
                  if (widget.onMemory != null || widget.onFollowAtOnce != null)
                    PopupMenuButton<void>(
                      tooltip: 'Moneypenny menu',
                      style: const ButtonStyle(
                        visualDensity: VisualDensity.compact,
                      ),
                      itemBuilder: (context) => [
                        if (widget.onMemory case final onMemory?)
                          PopupMenuItem(
                            onTap: onMemory,
                            child: const ListTile(
                              leading: Icon(Icons.psychology_outlined),
                              title: Text('Her memory of you'),
                            ),
                          ),
                        if (widget.onFollowAtOnce case final onFollow?)
                          CheckedPopupMenuItem(
                            checked: widget.followAtOnce,
                            onTap: () => onFollow(!widget.followAtOnce),
                            child: const Text(
                              "Open Moneypenny's links at once",
                            ),
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
              child: Text(
                controller.problem!,
                style: TextStyle(color: theme.colorScheme.error),
              ),
            ),
          _column(
            Padding(
              padding: const EdgeInsets.all(12),
              child: Row(
                children: [
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
    this.onOpen,
  });

  final AssistantEntry entry;
  final AssistantController controller;
  final void Function(String link)? onOpen;

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
          child: Text(revealInvisible(entry.text)),
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
              child: MarkdownText(entry.text, onOpen: onOpen),
            ),
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
      ),
      'navigation' => NavigationCard(data: entry.data, onOpen: onOpen),
      'view' => AssistantViewCard(
        json: entry.data?['view'],
        exposed: entry.data?['exposed'] == true,
        limits: controller.viewLimits,
        onOpen: onOpen,
        onSubmit: entry.id == null
            ? null
            : (values) => controller.submit(entry.id!, values),
      ),
      'notice' => Row(
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
      _ => Text(
        revealInvisible(entry.text),
        style: TextStyle(color: colors.error),
      ),
    };
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
    this.confirmingAfter = const Duration(seconds: 30),
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
            if (proposal['exposed'] == true)
              OutsideTextStrip(
                sources: [
                  for (final source
                      in (proposal['exposed_by'] as List?) ?? const [])
                    '$source',
                ],
              ),
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
