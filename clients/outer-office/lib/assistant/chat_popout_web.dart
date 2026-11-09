import 'dart:async';
import 'dart:js_interop';

import 'package:web/web.dart' as web;

import 'chat_popout.dart';

/// The browser's `window.open` and `BroadcastChannel`.
PopupPlatform? platformPopups() => const _Browser();

class _Browser implements PopupPlatform {
  const _Browser();

  @override
  Uri get popoutUrl => popoutUriFrom(Uri.base);

  @override
  PopupHandle? open(Uri url, {required double width, required double height}) {
    final window = web.window.open(
      url.toString(),
      'oo-moneypenny',
      'popup,width=${width.round()},height=${height.round()}',
    );
    return window == null ? null : _Window(window);
  }

  @override
  ChatLink link(String name) => _Channel(web.BroadcastChannel(name));

  @override
  Stream<void> get pageHide => _events(web.window, 'pagehide');

  @override
  Stream<void> get focused => _events(web.window, 'focus');

  @override
  bool get openerGone {
    final opener = web.window.opener;
    return opener == null || (opener as web.Window).closed;
  }

  @override
  void closeSelf() => web.window.close();
}

Stream<void> _events(web.EventTarget target, String type) {
  late final StreamController<void> events;
  JSFunction? listener;
  events = StreamController<void>.broadcast(
    onListen: () {
      listener = ((web.Event _) => events.add(null)).toJS;
      target.addEventListener(type, listener);
    },
    onCancel: () {
      target.removeEventListener(type, listener);
    },
  );
  return events.stream;
}

class _Window implements PopupHandle {
  _Window(this._window);

  final web.Window _window;

  @override
  bool get closed => _window.closed;

  @override
  void close() => _window.close();

  @override
  void focus() => _window.focus();
}

class _Channel implements ChatLink {
  _Channel(this._channel) {
    _channel.onmessage = ((web.MessageEvent event) {
      final message = decodeLinkMessage(event.data?.dartify());
      if (message != null) _messages.add(message);
    }).toJS;
  }

  final web.BroadcastChannel _channel;
  final _messages = StreamController<Map<String, Object?>>.broadcast();

  @override
  Stream<Map<String, Object?>> get messages => _messages.stream;

  @override
  void post(Map<String, Object?> message) =>
      _channel.postMessage(encodeLinkMessage(message).toJS);

  @override
  void close() {
    _channel.close();
    unawaited(_messages.close());
  }
}
