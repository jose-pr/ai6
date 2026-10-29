// ignore_for_file: invalid_use_of_internal_member
// ignore_for_file: implementation_imports

import 'package:flutter/src/widgets/_window.dart';
import 'package:material_ui/material_ui.dart';

/// A native desktop window the app opened, such as a Session's or Moneypenny's.
abstract interface class AppWindow {
  /// Closes the window; closing it twice does nothing.
  void close();

  /// Brings the window to the front and gives it the keyboard.
  void activate();
}

/// Opens a native window of [size] titled [title], drawn by [builder], through the
/// [WindowRegistry] above [context]. [onClosed] runs once when the window goes, whether the
/// person closed it or [AppWindow.close] did.
///
/// Desktop only, with Flutter's experimental windowing enabled: elsewhere there is no
/// registry and this throws.
AppWindow openAppWindow(
  BuildContext context, {
  required Size size,
  required String title,
  required WidgetBuilder builder,
  VoidCallback? onClosed,
}) {
  final registry = WindowRegistry.of(context);
  late final _RegisteredWindow window;
  final controller = WindowController(
    size: size,
    title: title,
    delegate: _WindowDelegate(onDestroyed: () => window._gone()),
  );
  window = _RegisteredWindow(
    registry,
    controller,
    WindowEntry(controller: controller, builder: builder),
    onClosed,
  );
  registry.register(window._entry);
  return window;
}

class _RegisteredWindow implements AppWindow {
  _RegisteredWindow(
    this._registry,
    this._controller,
    this._entry,
    this._onClosed,
  );

  final WindowRegistry _registry;
  final WindowController _controller;
  final WindowEntry _entry;
  final VoidCallback? _onClosed;
  bool _closed = false;

  void _gone() {
    if (_closed) return;
    _closed = true;
    _registry.unregister(_entry);
    _onClosed?.call();
  }

  @override
  void close() {
    if (_closed) return;
    // The registry must drop a window before it is destroyed.
    _gone();
    _controller.destroy();
  }

  @override
  void activate() {
    if (!_closed) _controller.activate();
  }
}

class _WindowDelegate with WindowControllerDelegate {
  _WindowDelegate({required this.onDestroyed});

  final VoidCallback onDestroyed;

  @override
  void onWindowDestroyed() {
    onDestroyed();
    super.onWindowDestroyed();
  }
}
