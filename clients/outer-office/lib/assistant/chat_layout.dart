import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../routes/oo_route.dart';
import '../settings/oo_config.dart';
import 'pages.dart' show PageActions;
import '../shell/app_window.dart';
import '../shell/oo_shell.dart';
import 'assistant_controller.dart';

/// Where Moneypenny's chat is, as the person chose it.
enum ChatLayout {
  /// Hidden: only the top-bar button, with a dot when she said something meanwhile.
  collapsed,

  /// Beside the page from [OoBreakpoints.wide], a modal sheet over it down to
  /// [OoBreakpoints.compact], full screen below that.
  docked,

  /// In the centre area, the rail kept and the page kept mounted out of sight; the same as
  /// [docked] below [OoBreakpoints.compact].
  expanded,

  /// In her own native window (desktop only).
  popped;

  static ChatLayout? byName(Object? name) {
    for (final layout in values) {
      if (layout.name == name) return layout;
    }
    return null;
  }
}

/// How the chat is drawn for a [ChatLayout] at a window width.
enum ChatPlacement {
  /// Not drawn.
  hidden,

  /// A resizable, non-modal panel beside the page.
  side,

  /// A modal sheet at the end of the window, over the page.
  sheet,

  /// The whole window, one view at a time.
  fullScreen,

  /// The centre area, the page out of sight.
  centre,

  /// In another window; this one says so.
  away;

  /// Whether focus stays inside the chat until it closes.
  bool get modal => this == sheet || this == fullScreen;
}

/// The placement of [layout] in a window [width] logical pixels wide.
ChatPlacement chatPlacement(ChatLayout layout, double width) =>
    switch (layout) {
      ChatLayout.collapsed => ChatPlacement.hidden,
      ChatLayout.popped => ChatPlacement.away,
      _ when width < OoBreakpoints.compact => ChatPlacement.fullScreen,
      ChatLayout.expanded => ChatPlacement.centre,
      ChatLayout.docked when width < OoBreakpoints.wide => ChatPlacement.sheet,
      ChatLayout.docked => ChatPlacement.side,
    };

/// The keyboard shortcuts of the chat, labelled for this platform: Ctrl+Alt elsewhere,
/// Control+Command (⌃⌘) on macOS.
enum ChatShortcut {
  /// Open, focus the composer, or hide.
  toggle('I', '⌃⌘I'),

  /// Expanded or docked.
  expand('Shift+I', '⌃⌘⇧I'),

  /// Her own window or back.
  popOut('O', '⌃⌘O');

  const ChatShortcut(this._key, this._mac);

  final String _key;
  final String _mac;

  /// The label shown in tooltips, such as `Ctrl+Alt+I`.
  String get label =>
      defaultTargetPlatform == TargetPlatform.macOS ? _mac : 'Ctrl+Alt+$_key';
}

/// A tooltip naming its [shortcut], such as `Expand (Ctrl+Alt+Shift+I)`.
String withShortcut(String tooltip, ChatShortcut shortcut) =>
    '$tooltip (${shortcut.label})';

/// Opens Moneypenny's own window for [chat] above [context]; [onClosed] runs when it goes.
typedef ChatWindowOpener = AppWindow Function(
  BuildContext context,
  MoneypennyChat chat,
  VoidCallback onClosed,
);

/// The layout and panel width one person chose on this device, kept under
/// `oo.chat.<personId>` as `{"layout": …, "width": …}`. Not synced across devices.
class ChatPreferences {
  ChatPreferences(this._preferences);

  final Future<SharedPreferences> _preferences;

  static String key(String personId) => 'oo.chat.$personId';

  /// The stored choice, or null when there is none or it does not parse. A missing width is
  /// null; the caller keeps it within its limits.
  Future<({ChatLayout layout, double? width})?> read(String personId) async {
    final text = (await _preferences).getString(key(personId));
    if (text == null) return null;
    try {
      final json = jsonDecode(text);
      if (json is! Map) return null;
      final layout = ChatLayout.byName(json['layout']);
      if (layout == null) return null;
      final width = json['width'];
      return (
        layout: layout,
        width: width is num && width.isFinite ? width.toDouble() : null,
      );
    } on FormatException {
      return null;
    }
  }

  Future<void> write(String personId, ChatLayout layout, double width) async {
    await (await _preferences).setString(
      key(personId),
      jsonEncode({'layout': layout.name, 'width': width}),
    );
  }

  /// Where [personId]'s "Open Moneypenny's links at once" is kept.
  static String followKey(String personId) => 'oo.chat.$personId.follow';

  /// Whether [personId] chose to open her links at once; off unless they did.
  Future<bool> readFollow(String personId) async =>
      (await _preferences).getBool(followKey(personId)) ?? false;

  Future<void> writeFollow(String personId, bool follow) async {
    await (await _preferences).setBool(followKey(personId), follow);
  }
}

/// The person's conversation with Moneypenny and where it is shown: one per sign-in,
/// shared by every layout and window, so they all show the same conversation.
class MoneypennyChat extends ChangeNotifier {
  MoneypennyChat({
    required this.controller,
    this.preferences,
    this.openWindow,
    this.settings = const ChatSettings(),
    this.label,
    this.proposalWait = ControlSettings.defaultConfirming,
    this.pageActions,
  }) : _width = settings.width {
    controller.addListener(_heard);
  }

  /// Sizes and timing, from `OO.cfg`'s `[chat]`.
  final ChatSettings settings;

  /// What this conversation is, shown in the chat, such as the preview's sample.
  final String? label;

  /// How long a proposal may stay applying before its card says the outcome is unknown
  /// (`OO.cfg`'s `[controls] confirming`).
  final Duration proposalWait;

  /// What the page cards ask of Universal Exports; none where there are no pages (the demo).
  final PageActions? pageActions;

  final AssistantController controller;
  final ChatPreferences? preferences;

  /// Opens her own window; without it (the web) she cannot pop out.
  final ChatWindowOpener? openWindow;

  bool get canPopOut => openWindow != null;

  ChatLayout _layout = ChatLayout.collapsed;
  ChatLayout get layout => _layout;

  /// The layout the next open returns to.
  ChatLayout _lastOpen = ChatLayout.docked;
  ChatLayout get lastOpen => _lastOpen;

  double _width;

  /// The side panel's width, between [ChatSettings.minWidth] and [ChatSettings.maxWidth].
  double get width => _width;

  /// She answered or proposed something while the chat was hidden.
  bool get unseen => _unseen;
  bool _unseen = false;
  double? _seenUpTo;

  /// The composer's text and focus, kept across layouts and windows.
  final message = TextEditingController();
  final composer = FocusNode(debugLabel: 'Moneypenny composer');

  /// Has focus whenever anything inside the chat does.
  final region = FocusNode(
    debugLabel: 'Moneypenny',
    canRequestFocus: false,
    skipTraversal: true,
  );

  /// The route of the app page the person is on, sent with each message.
  final route = ValueNotifier<OoRoute?>(null);

  /// Opens an app page one of her views, links or navigation cards points to.
  void Function(String link)? onLink;

  /// Whether a page she offers opens without the person's click: the person's choice, off
  /// by default. Never for one offered after she read outside text, nor over a dialog.
  bool get followAtOnce => _followAtOnce;
  bool _followAtOnce = false;

  /// Sets [followAtOnce], kept for the person once they are known.
  void setFollowAtOnce(bool follow) {
    if (_followAtOnce == follow) return;
    _followAtOnce = follow;
    _followChosen = true;
    final person = _person;
    if (person != null) {
      unawaited(
        preferences?.writeFollow(person, follow).catchError((Object _) {}),
      );
    }
    notifyListeners();
  }

  bool _followChosen = false;

  /// Opens her memory of the person above the given context.
  void Function(BuildContext context)? onMemory;

  /// Keeps the panel's state (scroll, draft) while it moves between places in one window.
  final panelKey = GlobalKey(debugLabel: 'Moneypenny panel');

  String? _person;

  /// The person whose choice is restored and saved, once known.
  String? get person => _person;
  bool _changedBeforeRestore = false;
  bool _focusPending = false;
  FocusNode? _returnFocus;
  AppWindow? _window;
  bool _disposed = false;

  /// Restores [personId]'s layout and width. [narrow] (below [OoBreakpoints.compact])
  /// starts a remembered docked or expanded chat hidden, one view at a time; `popped`
  /// stays popped only where she can pop out.
  Future<void> restore(String personId, {required bool narrow}) async {
    if (_person == personId) return;
    _person = personId;
    final ({ChatLayout layout, double? width})? saved;
    final bool follow;
    try {
      saved = await preferences?.read(personId);
      follow = await preferences?.readFollow(personId) ?? false;
    } on Object {
      return;
    }
    if (_disposed || _person != personId) return;
    if (!_followChosen && follow != _followAtOnce) {
      _followAtOnce = follow;
      notifyListeners();
    }
    if (saved == null || _disposed || _person != personId) return;
    _width = settings.clamp(saved.width ?? settings.width);
    if (!_changedBeforeRestore) {
      var layout = saved.layout;
      if (layout == ChatLayout.popped && !canPopOut) layout = ChatLayout.docked;
      if (layout != ChatLayout.collapsed) _lastOpen = layout;
      if (narrow && layout != ChatLayout.popped) layout = ChatLayout.collapsed;
      _layout = layout;
    }
    notifyListeners();
  }

  void _set(ChatLayout layout) {
    if (layout == ChatLayout.popped && !canPopOut) layout = ChatLayout.docked;
    if (_layout == layout) return;
    _changedBeforeRestore = true;
    _layout = layout;
    if (layout == ChatLayout.collapsed) {
      _seenUpTo = _latest() ?? double.negativeInfinity;
    } else {
      _lastOpen = layout;
      _unseen = false;
    }
    _save();
    notifyListeners();
  }

  void _save() {
    final person = _person;
    if (person == null) return;
    unawaited(
      preferences?.write(person, _layout, _width).catchError((Object _) {}),
    );
  }

  /// Shows the chat in [to], else in the layout it was last open in, and focuses the
  /// composer when [focus].
  void open({ChatLayout? to, bool focus = true}) {
    if (_layout == ChatLayout.collapsed) _remember();
    _set(to ?? _lastOpen);
    if (focus) focusComposer();
  }

  /// Hides the chat; the page keeps focus.
  void collapse() {
    final back = _returnFocus;
    _set(ChatLayout.collapsed);
    _returnFocus = null;
    if (back != null && back.context != null && back.canRequestFocus) {
      back.requestFocus();
    }
  }

  /// Expanded becomes docked, anything else open becomes expanded.
  void toggleExpanded() => switch (_layout) {
    ChatLayout.expanded => _set(ChatLayout.docked),
    ChatLayout.popped => null,
    ChatLayout.collapsed => open(to: ChatLayout.expanded),
    ChatLayout.docked => _set(ChatLayout.expanded),
  };

  /// Back beside the page, from the centre or her own window.
  void dock() => _set(ChatLayout.docked);

  /// Into her own window, or back from it.
  void togglePopOut() {
    if (!canPopOut) return;
    if (_layout == ChatLayout.popped) {
      _set(ChatLayout.docked);
    } else {
      if (_layout == ChatLayout.collapsed) _remember();
      _set(ChatLayout.popped);
    }
  }

  /// Sets the side panel's width; [save] stores it (at the end of a drag).
  void resize(double width, {bool save = true}) {
    final clamped = settings.clamp(width);
    if (clamped == _width) {
      if (save) _save();
      return;
    }
    _width = clamped;
    if (save) _save();
    notifyListeners();
  }

  /// Gives the composer the keyboard as soon as it can take it (it is disabled until the
  /// conversation is connected).
  void focusComposer() {
    _focusPending = true;
    _window?.activate();
    takePendingFocus();
    if (!_focusPending) return;
    WidgetsBinding.instance
      ..addPostFrameCallback((_) => takePendingFocus())
      ..scheduleFrame();
  }

  /// Focuses the composer when a [focusComposer] is waiting and it can take it.
  void takePendingFocus() {
    if (!_focusPending || _disposed) return;
    if (composer.context == null || !composer.canRequestFocus) return;
    _focusPending = false;
    composer.requestFocus();
  }

  /// Gives the keyboard back to what had it before the chat opened, else to nothing.
  void focusPage() {
    final back = _returnFocus;
    if (back != null && back.context != null && back.canRequestFocus) {
      back.requestFocus();
    } else {
      composer.unfocus();
    }
  }

  void _remember() {
    final focused = FocusManager.instance.primaryFocus;
    // A scope would hand focus straight back to its last child, the composer.
    _returnFocus = focused is FocusScopeNode || region.hasFocus
        ? null
        : focused;
  }

  double? _latest() => controller.entries.isEmpty
      ? null
      : controller.entries
            .map((entry) => entry.at)
            .reduce((a, b) => a > b ? a : b);

  /// Marks [unseen] when she answers, shows a view, offers a page or proposes while hidden.
  void _heard() {
    if (_layout != ChatLayout.collapsed || _unseen) return;
    final seen = _seenUpTo;
    if (seen == null) {
      _seenUpTo = _latest();
      return;
    }
    final news = controller.entries.any(
      (entry) =>
          entry.at > seen &&
          const {
            'moneypenny',
            'proposal',
            'view',
            'navigation',
          }.contains(entry.role),
    );
    if (!news) return;
    _unseen = true;
    notifyListeners();
  }

  /// Opens or closes her window to match [layout]; the window's own close docks the chat.
  void syncWindow(BuildContext context) {
    final opener = openWindow;
    if (_layout == ChatLayout.popped && _window == null && opener != null) {
      _window = opener(context, this, _windowClosed);
    } else if (_layout != ChatLayout.popped && _window != null) {
      final window = _window!;
      _window = null;
      window.close();
    }
  }

  void _windowClosed() {
    if (_window == null) return;
    _window = null;
    if (!_disposed && _layout == ChatLayout.popped) _set(ChatLayout.docked);
  }

  /// Applies a chat shortcut in a window [width] logical pixels wide; true when it was one.
  ///
  /// A key that types a character in a text field is never taken: on Windows AltGr arrives
  /// as Ctrl+Alt, so AltGr+I (í on US-International) must still type.
  bool handleKey(KeyEvent event, {required double width}) {
    if (event is! KeyDownEvent) return false;
    final keyboard = HardwareKeyboard.instance;
    final focused = FocusManager.instance.primaryFocus;
    final inChat = region.hasFocus;
    if (event.logicalKey == LogicalKeyboardKey.escape) {
      if (!inChat || keyboard.isShiftPressed) return false;
      final placement = chatPlacement(_layout, width);
      if (_layout == ChatLayout.expanded && placement == ChatPlacement.centre) {
        _set(ChatLayout.docked);
      } else if (placement.modal) {
        collapse();
      } else if (composer.hasFocus) {
        focusPage();
      } else {
        return false;
      }
      return true;
    }
    final mac = defaultTargetPlatform == TargetPlatform.macOS;
    final chord = mac
        ? keyboard.isControlPressed && keyboard.isMetaPressed
        : keyboard.isControlPressed && keyboard.isAltPressed;
    if (!chord) return false;
    final key = event.logicalKey;
    if (key != LogicalKeyboardKey.keyI && key != LogicalKeyboardKey.keyO) {
      return false;
    }
    if (keyTypesCharacter(event, focused)) return false;
    if (key == LogicalKeyboardKey.keyO) {
      if (!canPopOut) return false;
      togglePopOut();
    } else if (keyboard.isShiftPressed) {
      toggleExpanded();
    } else if (_layout == ChatLayout.collapsed) {
      open();
    } else if (inChat) {
      collapse();
    } else {
      _remember();
      focusComposer();
    }
    return true;
  }

  @override
  void dispose() {
    _disposed = true;
    controller.removeListener(_heard);
    final window = _window;
    _window = null;
    window?.close();
    controller.dispose();
    message.dispose();
    composer.dispose();
    region.dispose();
    route.dispose();
    super.dispose();
  }
}

/// Whether [event] would type a character into the focused text field. A browser reports
/// Ctrl+Alt+I with the character `i`; only a character other than the key's own counts.
bool keyTypesCharacter(KeyEvent event, FocusNode? focused) {
  final character = event.character;
  if (character == null || focused?.context == null) return false;
  final editing =
      focused!.context!.widget is EditableText ||
      focused.context!.findAncestorWidgetOfExactType<EditableText>() != null;
  if (!editing) return false;
  return character.toLowerCase() != event.logicalKey.keyLabel.toLowerCase();
}
