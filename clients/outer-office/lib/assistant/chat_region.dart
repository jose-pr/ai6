import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../routes/oo_route.dart';
import '../shell/oo_shell.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_theme.dart';
import 'assistant_panel.dart';
import 'chat_layout.dart';

/// How long [chat] takes to move, unless the platform asks for no animations.
Duration chatTransition(BuildContext context, MoneypennyChat chat) =>
    MediaQuery.disableAnimationsOf(context)
    ? Duration.zero
    : Duration(milliseconds: chat.settings.transitionMs);

/// Moneypenny's chat around the app shell [child]: her keyboard shortcuts, the full-screen
/// chat of narrow windows, her own window, and the layout [personId] chose last time.
///
/// Without a [chat] (the preview, or before sign-in) it is only [child].
class ChatShell extends StatefulWidget {
  const ChatShell({
    required this.chat,
    required this.child,
    this.personId,
    this.route,
    this.onLink,
    this.onMemory,
    super.key,
  });

  final MoneypennyChat? chat;
  final Widget child;
  final String? personId;

  /// The route of the app page the person is on, sent with each message to her.
  final OoRoute? route;

  /// Opens an app page one of her views links to.
  final void Function(String link)? onLink;

  /// Opens her memory of the person above the given context.
  final void Function(BuildContext context)? onMemory;

  @override
  State<ChatShell> createState() => _ChatShellState();
}

class _ChatShellState extends State<ChatShell> {
  ModalRoute<Object?>? _route;

  @override
  void initState() {
    super.initState();
    widget.chat?.addListener(_changed);
    HardwareKeyboard.instance.addHandler(_key);
    _hand(widget.chat);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _route = ModalRoute.of(context);
  }

  @override
  void didUpdateWidget(ChatShell old) {
    super.didUpdateWidget(old);
    if (old.chat != widget.chat) {
      old.chat?.removeListener(_changed);
      widget.chat?.addListener(_changed);
    }
    _hand(widget.chat);
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_key);
    widget.chat?.removeListener(_changed);
    super.dispose();
  }

  /// Gives [chat] this page's context and callbacks, and restores the person's layout.
  void _hand(MoneypennyChat? chat) {
    if (chat == null) return;
    chat
      ..onLink = widget.onLink
      ..onMemory = widget.onMemory;
    final route = widget.route;
    final person = widget.personId;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || widget.chat != chat) return;
      if (chat.route.value != route) chat.route.value = route;
      if (person != null && chat.person != person) {
        final narrow = MediaQuery.sizeOf(context).width < OoBreakpoints.compact;
        chat.restore(person, narrow: narrow);
      }
    });
  }

  void _changed() {
    if (!mounted) return;
    setState(() {});
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) widget.chat?.syncWindow(context);
    });
  }

  bool _key(KeyEvent event) {
    final chat = widget.chat;
    // Something covers the page (a dialog, the Memory page): its keys are its own.
    if (chat == null || !mounted || !(_route?.isCurrent ?? true)) return false;
    return chat.handleKey(event, width: MediaQuery.sizeOf(context).width);
  }

  @override
  Widget build(BuildContext context) {
    final chat = widget.chat;
    if (chat == null) return widget.child;
    final full =
        chatPlacement(chat.layout, MediaQuery.sizeOf(context).width) ==
        ChatPlacement.fullScreen;
    return Stack(
      fit: StackFit.expand,
      children: [
        // One view at a time; the page stays mounted underneath.
        Offstage(
          key: const ValueKey('page'),
          offstage: full,
          child: TickerMode(enabled: !full, child: widget.child),
        ),
        if (full)
          _Entrance(
            chat: chat,
            key: const ValueKey('chat-full'),
            from: const Offset(0, 0.03),
            child: Material(
              color: OoTokens.of(context).bg,
              child: SafeArea(
                child: ChatPanel(
                  chat: chat,
                  placement: ChatPlacement.fullScreen,
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// The page [child] in the shell's centre, with Moneypenny's chat beside it (resizable),
/// over it (a modal sheet), in its place (expanded, the page kept offstage so its scroll
/// and filters survive), or a note that she is in her own window.
class ChatRegion extends StatelessWidget {
  const ChatRegion({
    required this.chat,
    required this.pageLabel,
    required this.child,
    super.key,
  });

  final MoneypennyChat? chat;

  /// The current page's name, for "Back to …" when expanded.
  final String pageLabel;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final chat = this.chat;
    if (chat == null) return child;
    return ListenableBuilder(
      listenable: chat,
      builder: (context, _) {
        final placement = chatPlacement(
          chat.layout,
          MediaQuery.sizeOf(context).width,
        );
        return LayoutBuilder(
          builder: (context, constraints) {
            final settings = chat.settings;
            final widest = settings.clamp(
              constraints.maxWidth - settings.minPageWidth,
            );
            final width = chat.width.clamp(settings.minWidth, widest);
            final side = placement == ChatPlacement.side;
            final centre = placement == ChatPlacement.centre;
            return Stack(
              children: [
                Positioned.fill(
                  key: const ValueKey('page'),
                  right: side ? width + _ResizeHandle.width : 0,
                  child: Offstage(
                    offstage: centre,
                    child: TickerMode(enabled: !centre, child: child),
                  ),
                ),
                if (side)
                  Positioned(
                    key: const ValueKey('chat-side'),
                    top: 0,
                    bottom: 0,
                    right: 0,
                    width: width + _ResizeHandle.width,
                    child: _Entrance(
                      chat: chat,
                      from: const Offset(0.08, 0),
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          _ResizeHandle(chat: chat, widest: widest),
                          Expanded(
                            child: Material(
                              color: OoTokens.of(context).surface,
                              child: ChatPanel(
                                chat: chat,
                                placement: placement,
                              ),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                if (placement == ChatPlacement.sheet) ...[
                  Positioned.fill(
                    key: const ValueKey('chat-barrier'),
                    child: ModalBarrier(
                      color: Colors.black54,
                      dismissible: true,
                      onDismiss: chat.collapse,
                      semanticsLabel: 'Close Moneypenny',
                    ),
                  ),
                  Positioned(
                    key: const ValueKey('chat-sheet'),
                    top: 0,
                    bottom: 0,
                    right: 0,
                    width: width.clamp(0, constraints.maxWidth),
                    child: BlockSemantics(
                      child: _Entrance(
                        chat: chat,
                        from: const Offset(1, 0),
                        child: Material(
                          elevation: 8,
                          color: OoTokens.of(context).surface,
                          child: ChatPanel(chat: chat, placement: placement),
                        ),
                      ),
                    ),
                  ),
                ],
                if (centre)
                  Positioned.fill(
                    key: const ValueKey('chat-centre'),
                    child: _Entrance(
                      chat: chat,
                      from: const Offset(0, 0.02),
                      child: Material(
                        color: OoTokens.of(context).bg,
                        child: ChatPanel(
                          chat: chat,
                          placement: placement,
                          pageLabel: pageLabel,
                        ),
                      ),
                    ),
                  ),
                if (placement == ChatPlacement.away)
                  Positioned(
                    key: const ValueKey('chat-away'),
                    right: 16,
                    bottom: 16,
                    child: _AwayNote(chat: chat),
                  ),
              ],
            );
          },
        );
      },
    );
  }
}

/// The one mounted chat: the conversation, its header buttons for [placement], and the
/// "Moneypenny" landmark around it. [inWindow] is her own window.
class ChatPanel extends StatelessWidget {
  const ChatPanel({
    required this.chat,
    required this.placement,
    this.pageLabel,
    this.inWindow = false,
    super.key,
  });

  final MoneypennyChat chat;
  final ChatPlacement placement;

  /// The page "Back to …" returns to, where [placement] is centre.
  final String? pageLabel;
  final bool inWindow;

  @override
  Widget build(BuildContext context) {
    final panel = Semantics(
      container: true,
      explicitChildNodes: true,
      role: SemanticsRole.complementary,
      label: 'Moneypenny',
      child: Focus(
        focusNode: chat.region,
        child: ValueListenableBuilder(
          valueListenable: chat.route,
          builder: (context, route, _) => AssistantPanel(
            // Keeps the scroll and the panel's state as it moves around the page. The
            // full-screen chat mounts its own: moving a keyed subtree out of it into the
            // page's just-onstage region fails Flutter's semantics geometry assertion.
            key: inWindow || placement == ChatPlacement.fullScreen
                ? null
                : chat.panelKey,
            controller: chat.controller,
            route: route,
            onOpen: (link) => chat.onLink?.call(link),
            onMemory: chat.onMemory == null
                ? null
                : () => chat.onMemory?.call(context),
            onClose: placement == ChatPlacement.fullScreen
                ? chat.collapse
                : null,
            closeTooltip: withShortcut('Close Moneypenny', ChatShortcut.toggle),
            actions: _actions(),
            message: chat.message,
            composerFocus: chat.composer,
            label: chat.label,
            textWidth: placement == ChatPlacement.centre
                ? chat.settings.textWidth
                : null,
          ),
        ),
      ),
    );
    final body = switch (placement) {
      ChatPlacement.centre => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 0),
            child: ActionChip(
              avatar: const Icon(Icons.arrow_back, size: 18),
              label: Text('Back to ${pageLabel ?? 'the page'}'),
              tooltip: withShortcut('Back to the side', ChatShortcut.expand),
              onPressed: chat.dock,
            ),
          ),
          Expanded(child: panel),
        ],
      ),
      _ => panel,
    };
    return _FocusWhenConnected(
      chat: chat,
      child: placement.modal ? FocusScope(child: body) : body,
    );
  }

  List<Widget> _actions() {
    IconButton button(String tooltip, IconData icon, VoidCallback onPressed) =>
        IconButton(
          tooltip: tooltip,
          visualDensity: VisualDensity.compact,
          onPressed: onPressed,
          icon: Icon(icon),
        );
    return [
      if (inWindow)
        button(
          withShortcut('Bring back', ChatShortcut.popOut),
          Icons.input,
          chat.togglePopOut,
        )
      else ...[
        if (placement == ChatPlacement.centre)
          button(
            withShortcut('Back to the side', ChatShortcut.expand),
            Icons.close_fullscreen,
            chat.toggleExpanded,
          )
        else if (placement != ChatPlacement.fullScreen)
          button(
            withShortcut('Expand', ChatShortcut.expand),
            Icons.open_in_full,
            chat.toggleExpanded,
          ),
        if (chat.canPopOut)
          button(
            withShortcut('Pop out', ChatShortcut.popOut),
            Icons.open_in_new,
            chat.togglePopOut,
          ),
      ],
      if (placement != ChatPlacement.fullScreen)
        button(
          withShortcut('Hide Moneypenny', ChatShortcut.toggle),
          Icons.close,
          chat.collapse,
        ),
    ];
  }
}

/// Focuses the composer once the conversation connects, when a shortcut or button asked
/// for it before then (the composer is disabled until it connects).
class _FocusWhenConnected extends StatefulWidget {
  const _FocusWhenConnected({required this.chat, required this.child});

  final MoneypennyChat chat;
  final Widget child;

  @override
  State<_FocusWhenConnected> createState() => _FocusWhenConnectedState();
}

class _FocusWhenConnectedState extends State<_FocusWhenConnected> {
  @override
  void initState() {
    super.initState();
    widget.chat.controller.addListener(_connected);
  }

  @override
  void didUpdateWidget(_FocusWhenConnected old) {
    super.didUpdateWidget(old);
    if (old.chat != widget.chat) {
      old.chat.controller.removeListener(_connected);
      widget.chat.controller.addListener(_connected);
    }
  }

  @override
  void dispose() {
    widget.chat.controller.removeListener(_connected);
    super.dispose();
  }

  void _connected() {
    if (!widget.chat.controller.connected) return;
    WidgetsBinding.instance.addPostFrameCallback(
      (_) => mounted ? widget.chat.takePendingFocus() : null,
    );
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// Where she is in her own window: says so, with Bring back.
class _AwayNote extends StatelessWidget {
  const _AwayNote({required this.chat});

  final MoneypennyChat chat;

  @override
  Widget build(BuildContext context) => Semantics(
    container: true,
    role: SemanticsRole.complementary,
    label: 'Moneypenny',
    child: Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 10, 8, 10),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          spacing: 12,
          children: [
            const ExcludeSemantics(child: MoneypennyAvatar()),
            const Text('Moneypenny is in her own window'),
            TextButton(
              onPressed: chat.togglePopOut,
              child: Tooltip(
                message: withShortcut('Bring back', ChatShortcut.popOut),
                child: const Text('Bring back'),
              ),
            ),
          ],
        ),
      ),
    ),
  );
}

/// The handle between the page and the docked chat: dragged, or moved with the arrow keys
/// [ChatSettings.widthStep] at a time, it sets the chat's width up to [widest].
class _ResizeHandle extends StatefulWidget {
  const _ResizeHandle({required this.chat, required this.widest});

  /// Its own width, in logical pixels.
  static const double width = 8;

  final MoneypennyChat chat;
  final double widest;

  @override
  State<_ResizeHandle> createState() => _ResizeHandleState();
}

class _ResizeHandleState extends State<_ResizeHandle> {
  bool _hovered = false;
  bool _focused = false;

  double get _width =>
      widget.chat.width.clamp(widget.chat.settings.minWidth, widget.widest);

  void _resize(double width, {bool save = true}) => widget.chat.resize(
    width.clamp(widget.chat.settings.minWidth, widget.widest),
    save: save,
  );

  KeyEventResult _key(FocusNode _, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    // The chat is on the right: the handle moving left widens it.
    if (event.logicalKey == LogicalKeyboardKey.arrowLeft) {
      _resize(_width + widget.chat.settings.widthStep);
    } else if (event.logicalKey == LogicalKeyboardKey.arrowRight) {
      _resize(_width - widget.chat.settings.widthStep);
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final active = _hovered || _focused;
    final width = _width.round();
    return Semantics(
      slider: true,
      label: 'Moneypenny width',
      value: '$width pixels',
      increasedValue:
          '${(_width + widget.chat.settings.widthStep).clamp(widget.chat.settings.minWidth, widget.widest).round()} pixels',
      decreasedValue:
          '${(_width - widget.chat.settings.widthStep).clamp(widget.chat.settings.minWidth, widget.widest).round()} pixels',
      onIncrease: () => _resize(_width + widget.chat.settings.widthStep),
      onDecrease: () => _resize(_width - widget.chat.settings.widthStep),
      child: Focus(
        onKeyEvent: _key,
        onFocusChange: (focused) => setState(() => _focused = focused),
        child: MouseRegion(
          cursor: SystemMouseCursors.resizeColumn,
          onEnter: (_) => setState(() => _hovered = true),
          onExit: (_) => setState(() => _hovered = false),
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onHorizontalDragUpdate: (details) =>
                _resize(_width - details.delta.dx, save: false),
            onHorizontalDragEnd: (_) => _resize(_width),
            child: SizedBox(
              width: _ResizeHandle.width,
              child: Center(
                child: AnimatedContainer(
                  duration: chatTransition(context, widget.chat),
                  width: active ? 3 : 1,
                  color: _focused ? tokens.accent : tokens.border,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Slides and fades [child] in from [from] (a fraction of its size) when it appears.
class _Entrance extends StatelessWidget {
  const _Entrance({
    required this.chat,
    required this.from,
    required this.child,
    super.key,
  });

  final MoneypennyChat chat;
  final Offset from;
  final Widget child;

  @override
  Widget build(BuildContext context) => TweenAnimationBuilder<double>(
    tween: Tween(begin: 0, end: 1),
    duration: chatTransition(context, chat),
    curve: Curves.easeOutCubic,
    child: child,
    builder: (context, t, child) => FractionalTranslation(
      translation: from * (1 - t),
      child: Opacity(opacity: t, child: child),
    ),
  );
}

/// The top bar's Moneypenny button: opens the chat where it was last open, shows her own
/// window, or hides the chat; a dot says she answered or proposed something while hidden.
class MoneypennyButton extends StatelessWidget {
  const MoneypennyButton({required this.chat, super.key});

  final MoneypennyChat chat;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: chat,
    builder: (context, _) {
      final (String tooltip, VoidCallback onPressed) = switch (chat.layout) {
        ChatLayout.collapsed => ('Ask Moneypenny', chat.open),
        ChatLayout.popped => ('Go to Moneypenny\'s window', chat.focusComposer),
        _ => ('Hide Moneypenny', chat.collapse),
      };
      return Semantics(
        value: chat.unseen ? 'She has something new' : null,
        child: IconButton(
          tooltip: withShortcut(tooltip, ChatShortcut.toggle),
          isSelected: chat.layout != ChatLayout.collapsed,
          onPressed: onPressed,
          icon: Badge(
            isLabelVisible: chat.unseen,
            smallSize: 9,
            child: const OoIcon(OoIconName.moneypenny),
          ),
        ),
      );
    },
  );
}
