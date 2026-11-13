import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:material_ui/material_ui.dart';

import '../shell/app_window.dart';
import 'chat_layout.dart';
import 'chat_popout_stub.dart'
    if (dart.library.js_interop) 'chat_popout_web.dart';

/// The query parameter that makes the app start as Moneypenny's pop-out window, not the app.
const popoutParameter = 'oo_window';

/// Its value for the chat.
const popoutChat = 'chat';

/// The browser's pop-up windows and same-origin message channel, or null where there is no
/// browser. Tests give their own.
PopupPlatform? browserPopups() => platformPopups();

/// A window the page opened with `window.open`.
abstract interface class PopupHandle {
  bool get closed;
  void close();
  void focus();
}

/// A same-origin `BroadcastChannel`: JSON objects in, JSON objects out.
abstract interface class ChatLink {
  Stream<Map<String, Object?>> get messages;
  void post(Map<String, Object?> message);
  void close();
}

/// What the page needs from the browser to open Moneypenny's chat in another window of the
/// same app, and what that window needs to find its opener.
abstract interface class PopupPlatform {
  /// The URL the pop-out starts at: this app with [popoutParameter] set.
  Uri get popoutUrl;

  /// Opens a window [width] by [height] logical pixels at [url]; null when the browser
  /// blocked it.
  PopupHandle? open(Uri url, {required double width, required double height});

  /// The channel named [name]; every page of this origin that opens it hears the others.
  ChatLink link(String name);

  /// This page is going away: closed, reloaded or navigated off.
  Stream<void> get pageHide;

  /// This page took the focus.
  Stream<void> get focused;

  /// This page is a pop-out whose opener has closed, or was never there.
  bool get openerGone;

  /// Closes this window, which only a window a page opened may do.
  void closeSelf();
}

/// The browser refused to open the chat's window, which it does for a pop-up no click asked
/// for and when the person blocked pop-ups for the site.
class ChatWindowBlocked implements Exception {
  const ChatWindowBlocked();

  static const message =
      'Your browser blocked Moneypenny\'s window. Allow pop-ups for this page and '
      'try again.';
}

String _channelName(String? person) => 'oo.chat.${person ?? ''}';

/// Opens Moneypenny's chat as a second instance of this app in a browser window, bound to the
/// same conversation (the pop-out resumes it by its ID) and to the same composer draft (a
/// `BroadcastChannel`), and gives back a window the chat can close.
///
/// [appearance] is told whenever it changes so the pop-out follows the page's theme. Input in
/// the pop-out calls [onActivity], and it may report again after each change of [renewals].
ChatWindowOpener popupChatWindowOpener(
  PopupPlatform platform, {
  Listenable? appearance,
  Map<String, Object?> Function()? appearanceNow,
  VoidCallback? onActivity,
  ValueListenable<int>? renewals,
}) => (context, chat, onClosed) {
  final link = platform.link(_channelName(chat.person));
  final handle = platform.open(
    platform.popoutUrl,
    width: chat.settings.windowWidth,
    height: chat.settings.windowHeight,
  );
  if (handle == null) {
    link.close();
    throw const ChatWindowBlocked();
  }
  return _Popup(
    platform: platform,
    handle: handle,
    link: link,
    chat: chat,
    onClosed: onClosed,
    appearance: appearance,
    appearanceNow: appearanceNow,
    onActivity: onActivity,
    renewals: renewals,
  );
};

class _Popup implements AppWindow {
  _Popup({
    required this.platform,
    required this.handle,
    required this.link,
    required this.chat,
    required this.onClosed,
    required this.appearance,
    required this.appearanceNow,
    required this.onActivity,
    required this.renewals,
  }) {
    _subscriptions
      ..add(link.messages.listen(_heard))
      ..add(platform.pageHide.listen((_) => link.post({'t': 'gone'})))
      // The pop-out's own pagehide says it closed; a window the browser killed may not.
      ..add(
        platform.focused.listen((_) {
          if (handle.closed) _finish();
        }),
      );
    chat.message.addListener(_draft);
    appearance?.addListener(_look);
    renewals?.addListener(_renewed);
  }

  final VoidCallback? onActivity;
  final ValueListenable<int>? renewals;

  void _renewed() => link.post({'t': 'renewed'});

  final PopupPlatform platform;
  final PopupHandle handle;
  final ChatLink link;
  final MoneypennyChat chat;
  final VoidCallback onClosed;
  final Listenable? appearance;
  final Map<String, Object?> Function()? appearanceNow;
  final _subscriptions = <StreamSubscription<Object?>>[];
  String _sent = '';
  String? _looked;
  bool _done = false;

  void _draft() {
    final text = chat.message.text;
    if (text == _sent) return;
    _sent = text;
    link.post({'t': 'draft', 'text': text});
  }

  void _look() {
    final now = appearanceNow?.call();
    if (now == null) return;
    final text = encodeLinkMessage(now);
    if (text == _looked) return;
    _looked = text;
    link.post({'t': 'appearance', ...now});
  }

  void _heard(Map<String, Object?> message) {
    switch (message['t']) {
      case 'ask':
        _sent = chat.message.text;
        _looked = appearanceNow == null
            ? null
            : encodeLinkMessage(appearanceNow!());
        link.post({
          't': 'state',
          'conversation': chat.controller.conversationId,
          'draft': chat.message.text,
          ...?appearanceNow?.call(),
        });
      case 'draft':
        final text = message['text'];
        if (text is String && text != chat.message.text) {
          _sent = text;
          chat.message.value = TextEditingValue(
            text: text,
            selection: TextSelection.collapsed(offset: text.length),
          );
        }
      case 'active':
        onActivity?.call();
      case 'link':
        final target = message['link'];
        if (target is String) chat.onLink?.call(target);
      case 'bye':
        final hide = message['why'] == 'hide';
        _finish();
        if (hide) chat.collapse();
    }
  }

  void _finish() {
    if (_done) return;
    _done = true;
    _release();
    onClosed();
  }

  void _release() {
    chat.message.removeListener(_draft);
    appearance?.removeListener(_look);
    renewals?.removeListener(_renewed);
    for (final subscription in _subscriptions) {
      unawaited(subscription.cancel());
    }
    link.close();
  }

  @override
  void close() {
    if (_done) return;
    _done = true;
    link.post({'t': 'gone'});
    _release();
    handle.close();
  }

  @override
  void activate() {
    if (_done) return;
    if (handle.closed) {
      _finish();
    } else {
      handle.focus();
    }
  }
}

/// What the opener told a pop-out when it asked.
typedef PopoutState = ({
  String? conversation,
  String draft,
  Map<String, Object?> appearance,
});

/// The pop-out's side: asks its opener for the conversation and the draft, keeps the draft
/// level with the opener's, and tells it when the person brings the chat back or closes the
/// window.
class PopoutRun {
  PopoutRun(this.platform, {String? person})
    : link = platform.link(_channelName(person)) {
    _subscriptions
      ..add(link.messages.listen(_heard))
      ..add(platform.pageHide.listen((_) => _bye('closed')));
    if (platform.openerGone) {
      _gone();
    } else {
      link.post({'t': 'ask'});
    }
  }

  final PopupPlatform platform;
  final ChatLink link;
  final _subscriptions = <StreamSubscription<Object?>>[];
  final _state = Completer<PopoutState>();
  MoneypennyChat? _chat;
  String _sent = '';
  bool _ended = false;
  bool _reported = false;

  /// Tells the opener there was input here, so its sign-in is not asked about; once per
  /// renewal, as it says when one happens.
  void activity() {
    if (_reported || _ended) return;
    _reported = true;
    link.post({'t': 'active'});
  }

  /// The opener's conversation and draft; never completes when the opener is gone.
  Future<PopoutState> get state => _state.future;

  /// Whether the opener has closed or reloaded: the chat is back in the page, and this
  /// window has nothing to show.
  final openerClosed = ValueNotifier<bool>(false);

  /// Keeps [chat]'s composer draft level with the opener's, and sends the links she offers
  /// to the page.
  void attach(MoneypennyChat chat, {required String draft}) {
    _chat = chat;
    _sent = draft;
    chat.onLink = (target) => link.post({'t': 'link', 'link': target});
    chat.message
      ..value = TextEditingValue(
        text: draft,
        selection: TextSelection.collapsed(offset: draft.length),
      )
      ..addListener(_draft);
  }

  void _draft() {
    final text = _chat?.message.text ?? '';
    if (text == _sent) return;
    _sent = text;
    link.post({'t': 'draft', 'text': text});
  }

  void _heard(Map<String, Object?> message) {
    switch (message['t']) {
      case 'state':
        if (_state.isCompleted) return;
        _state.complete((
          conversation: message['conversation'] as String?,
          draft: message['draft'] as String? ?? '',
          appearance: {
            'theme': message['theme'],
            'backdrop': message['backdrop'],
          },
        ));
      case 'draft':
        final text = message['text'];
        final chat = _chat;
        if (text is String && chat != null && text != chat.message.text) {
          _sent = text;
          chat.message.value = TextEditingValue(
            text: text,
            selection: TextSelection.collapsed(offset: text.length),
          );
        }
      case 'appearance':
        onAppearance?.call({
          'theme': message['theme'],
          'backdrop': message['backdrop'],
        });
      case 'renewed':
        _reported = false;
      case 'gone':
        _gone();
    }
  }

  /// Told the theme and wallpaper the opener's page now has.
  void Function(Map<String, Object?> appearance)? onAppearance;

  void _gone() {
    if (_ended) return;
    _ended = true;
    openerClosed.value = true;
    platform.closeSelf();
  }

  void _bye(String why) {
    if (_ended) return;
    _ended = true;
    link.post({'t': 'bye', 'why': why});
  }

  /// The person chose Bring back: the opener shows the chat again and this window goes.
  void bringBack() {
    _bye('back');
    platform.closeSelf();
  }

  /// The person chose Hide: the opener hides the chat and this window goes.
  void hide() {
    _bye('hide');
    platform.closeSelf();
  }

  void dispose() {
    _chat?.message.removeListener(_draft);
    for (final subscription in _subscriptions) {
      unawaited(subscription.cancel());
    }
    link.close();
    openerClosed.dispose();
  }
}

/// Encodes [message] for a channel that carries strings.
String encodeLinkMessage(Map<String, Object?> message) => jsonEncode(message);

/// Decodes one frame of a channel; null for anything that is not a JSON object.
Map<String, Object?>? decodeLinkMessage(Object? frame) {
  if (frame is! String) return null;
  try {
    final value = jsonDecode(frame);
    return value is Map ? value.cast<String, Object?>() : null;
  } on FormatException {
    return null;
  }
}

/// The pop-out's address from the page's: the same app, nothing after the `?`.
Uri popoutUriFrom(Uri page) => page.removeFragment().replace(
  queryParameters: {popoutParameter: popoutChat},
);

/// Whether this page was opened as the chat's pop-out.
bool startedAsPopout(Uri page) =>
    page.queryParameters[popoutParameter] == popoutChat;
