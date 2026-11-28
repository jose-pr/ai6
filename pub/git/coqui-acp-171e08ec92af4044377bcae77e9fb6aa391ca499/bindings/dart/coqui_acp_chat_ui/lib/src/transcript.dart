import 'dart:async';

import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:flutter/foundation.dart' show defaultTargetPlatform, TargetPlatform;
import 'package:material_ui/material_ui.dart';
import 'package:flutter/rendering.dart' show ScrollDirection, SelectedContent;
import 'package:flutter/services.dart';

import 'content.dart';
import 'entry.dart';
import 'fold.dart';
import 'folded.dart';
import 'look.dart';
import 'renderers.dart';
import 'selection.dart';
import 'working.dart';

/// A conversation: its entries laid out at the look's [ChatDensity], the agent's plan as a checklist at the end while it has one,
/// one selection over all of it, and the end kept in view as entries arrive until the person scrolls away, with a button back.
///
/// It draws nothing of its own that a host cannot replace: [renderers] adds content kinds, a card for a kind of tool, and the
/// lines a burst, a turn and the plan fold into.
class ChatTranscript extends StatefulWidget {
  const ChatTranscript({
    super.key,
    required this.entries,
    this.terminals = const {},
    this.plan = const [],
    this.working = false,
    this.renderers = ChatRenderers.none,
    this.density,
    this.controller,
    this.narrow = false,
    this.turns = const [],
    this.agentName,
    this.starting,
    this.clock = DateTime.now,
  });

  /// In order, oldest first.
  final List<ChatEntryData> entries;

  /// What commands printed, by the terminal id a tool's content names.
  final Map<String, TerminalView> terminals;
  final List<PlanEntry> plan;

  /// Whether a turn is running, which keeps the last from folding.
  final bool working;
  final ChatRenderers renderers;

  /// How much it shows; the look's own when null.
  final ChatDensity? density;
  final ScrollController? controller;

  /// A phone's width: less space at the sides.
  final bool narrow;

  /// When the turns started and ended: a folded turn says how long it ran, and the working row counts from the running one's start.
  final List<TurnSpan> turns;

  /// Who the working row says is starting, as in "Starting Claude…".
  final String? agentName;

  /// Whether the session itself is still starting; null infers it from whether the conversation has seen any agent output yet.
  final bool? starting;

  /// The time the working row counts to.
  final DateTime Function() clock;

  @override
  State<ChatTranscript> createState() => ChatTranscriptState();
}

class ChatTranscriptState extends State<ChatTranscript> {
  /// How far from the end, in logical pixels, still counts as at the end.
  static const double nearEnd = 24;

  final GlobalKey<SelectableTranscriptState> _selection = GlobalKey();
  ScrollController? _own;
  bool _following = true;

  ScrollController get _scroll => widget.controller ?? (_own ??= ScrollController());

  /// The conversation's own selection, which Ctrl+A fills and Ctrl+C copies.
  SelectableTranscriptState? get selection => _selection.currentState;

  /// Whether the end is in view and stays in view as entries arrive.
  bool get following => _following;

  bool _onScroll(ScrollNotification notification) {
    // Only the transcript's own scrolling, not a code box's inside an entry.
    if (notification.depth != 0) return false;
    if (notification is UserScrollNotification) {
      if (notification.direction == ScrollDirection.forward && _following) {
        setState(() => _following = false);
      }
      return false;
    }
    if (notification is ScrollUpdateNotification) {
      final delta = notification.scrollDelta ?? 0;
      if (delta < 0) {
        if (_following) setState(() => _following = false);
        return false;
      }
      if (delta > 0 && notification.metrics.extentAfter <= 2 && !_following) {
        setState(() => _following = true);
      }
      return false;
    }
    if (notification is ScrollEndNotification) {
      final atEnd = notification.metrics.extentAfter <= nearEnd;
      if (!atEnd && _following) {
        setState(() => _following = false);
      }
    }
    return false;
  }

  /// The transcript grew or shrank: while following, its end comes into view.
  bool _onMetrics(ScrollMetricsNotification notification) {
    if (notification.depth == 0 && _following && notification.metrics.extentAfter > 0) _toEnd();
    return false;
  }

  Widget _workingRow() {
    final running = widget.turns.where((turn) => turn.endedAt == null).lastOrNull;
    // The running turn's entries are those after the person's last message.
    final at = widget.entries.lastIndexWhere((entry) => entry.role == MessageRole.user);
    final turn = at < 0 ? widget.entries : widget.entries.sublist(at + 1);
    final since = at < 0 ? null : widget.entries[at].at;
    final initialStart = widget.starting ??
        !widget.entries.any(
          (entry) => entry.role == MessageRole.agent || entry.role == MessageRole.thought || entry.role == MessageRole.tool,
        );
    final look = context.chatLook;
    return look.column(
      Padding(
        padding: EdgeInsets.only(bottom: look.gap, top: 2),
        child: WorkingRow(
          headline: workingHeadline(turn, agent: widget.agentName, starting: initialStart),
          startedAt: running?.startedAt ?? since,
          clock: widget.clock,
        ),
      ),
    );
  }

  void _toEnd() {
    if (_scroll.hasClients) _scroll.jumpTo(_scroll.position.maxScrollExtent);
  }

  /// Follows the end again and brings it into view: what sending a message does.
  void jumpToLatest() {
    setState(() => _following = true);
    _toEnd();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _toEnd();
    });
  }

  @override
  void dispose() {
    _own?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final density = widget.density ?? look.density;
    final items = foldTranscript(
      widget.entries,
      density: density,
      working: widget.working,
      showThinking: look.showThinking,
      turns: widget.turns,
    );
    final planAt = widget.plan.isEmpty ? -1 : items.length;
    final count = items.length + (widget.plan.isEmpty ? 0 : 1) + (widget.working ? 1 : 0);
    return ChatScope(
      renderers: widget.renderers,
      terminals: widget.terminals,
      child: Stack(children: [
        NotificationListener<ScrollMetricsNotification>(
          onNotification: _onMetrics,
          child: NotificationListener<ScrollNotification>(
            onNotification: _onScroll,
            child: SelectableTranscript(
              key: _selection,
              entries: widget.entries,
              child: ListView.builder(
                controller: _scroll,
                padding: EdgeInsets.symmetric(horizontal: widget.narrow ? 12 : 40, vertical: look.padding + 10),
                itemCount: count,
                itemBuilder: (context, index) {
                  // Each entry is a paragraph of its own in what a selection copies.
                  if (index < items.length) return SelectionBlock(lead: '\n\n', child: TranscriptItemView(item: items[index]));
                  if (index != planAt) return SelectionContainer.disabled(child: _workingRow());
                  return SelectionBlock(
                    lead: '\n\n',
                    child: look.column(Padding(padding: EdgeInsets.only(bottom: look.gap), child: PlanChecklist(entries: widget.plan))),
                  );
                },
              ),
            ),
          ),
        ),
        if (!_following)
          Positioned(
            left: 0,
            right: 0,
            bottom: 12,
            child: Center(
              child: IconButton.filledTonal(
                tooltip: 'Jump to latest',
                onPressed: jumpToLatest,
                icon: const Icon(Icons.arrow_downward_rounded),
              ),
            ),
          ),
      ]),
    );
  }
}

/// The conversation as one selection: dragging selects across messages, tool cards, code and diffs, Ctrl+A (⌘A on a Mac)
/// selects all of it and Ctrl+C copies, and the context menu offers Copy and Select all. Entries the list has not built are
/// in it too: after Select all, Copy gives the whole conversation's text.
class SelectableTranscript extends StatefulWidget {
  const SelectableTranscript({super.key, required this.entries, required this.child});

  final List<ChatEntryData> entries;
  final Widget child;

  @override
  State<SelectableTranscript> createState() => SelectableTranscriptState();
}

class SelectableTranscriptState extends State<SelectableTranscript> {
  late final FocusNode _focus = FocusNode(debugLabel: 'transcript', onKeyEvent: _onKey);

  /// Whether the selection is all of it: set by Select all, and ended by any other change.
  bool _all = false;
  bool _selectingAll = false;
  String? _allShown;

  /// What the selection holds now, as the blocks under it wrote it.
  String? _selected;

  bool get _mac => defaultTargetPlatform == TargetPlatform.macOS || defaultTargetPlatform == TargetPlatform.iOS;

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    final keys = HardwareKeyboard.instance;
    if (!(_mac ? keys.isMetaPressed : keys.isControlPressed) || keys.isShiftPressed || keys.isAltPressed) {
      return KeyEventResult.ignored;
    }
    if (event.logicalKey == LogicalKeyboardKey.keyA) {
      _startSelectingAll();
    } else if (event.logicalKey == LogicalKeyboardKey.keyC) {
      if (_all) {
        unawaited(copyAll());
        return KeyEventResult.handled;
      }
      if (_selected != null) {
        unawaited(copySelected());
        return KeyEventResult.handled;
      }
    }
    return KeyEventResult.ignored;
  }

  /// Puts the whole conversation's text on the clipboard.
  Future<void> copyAll() => Clipboard.setData(ClipboardData(text: conversationText(widget.entries)));

  /// Puts the selected text on the clipboard without the blank lines the blocks put at its ends.
  Future<void> copySelected() => Clipboard.setData(ClipboardData(text: _trimmed(_selected ?? '')));

  static String _trimmed(String text) => text.replaceAll(RegExp(r'^\n+|\n+$'), '');

  /// Select all is under way until the frame after it: what the selection reports meanwhile is all of it.
  void _startSelectingAll() {
    _selectingAll = true;
    _all = true;
    WidgetsBinding.instance.addPostFrameCallback((_) => _selectingAll = false);
  }

  void _changed(SelectedContent? content) {
    _selected = content?.plainText;
    if (_selectingAll) {
      _all = content != null;
      _allShown = content?.plainText;
    } else if (_all && content?.plainText != _allShown) {
      _all = false;
    }
  }

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => SelectionArea(
    focusNode: _focus,
    onSelectionChanged: _changed,
    contextMenuBuilder: (context, region) => AdaptiveTextSelectionToolbar.buttonItems(
      anchors: region.contextMenuAnchors,
      buttonItems: [
        for (final item in region.contextMenuButtonItems)
          switch (item.type) {
            ContextMenuButtonType.copy => item.copyWith(onPressed: () {
              if (_all) {
                unawaited(copyAll());
                region.hideToolbar();
              } else if (_selected != null) {
                unawaited(copySelected());
                region.hideToolbar();
              } else {
                item.onPressed?.call();
              }
            }),
            ContextMenuButtonType.selectAll => item.copyWith(onPressed: () {
              _startSelectingAll();
              item.onPressed?.call();
            }),
            _ => item,
          },
      ],
    ),
    child: widget.child,
  );
}
