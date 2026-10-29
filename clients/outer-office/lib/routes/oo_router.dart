import 'package:flutter/foundation.dart' show SynchronousFuture;
import 'package:material_ui/material_ui.dart';

import '../shell/oo_shell.dart' show OoPage;
import 'oo_route.dart';

/// What the app says when a web URL is not a route.
const unknownLinkNotice = 'Outer Office does not know this link';

/// Reads a web location (the URL after `#`) as an [OoRoute], and writes one back. A location
/// that is not a route opens Overview and tells [onUnknown] why.
class OoRouteParser extends RouteInformationParser<OoRoute> {
  OoRouteParser({required this.onUnknown});

  final void Function(String reason) onUnknown;

  @override
  Future<OoRoute> parseRouteInformation(RouteInformation routeInformation) {
    try {
      return SynchronousFuture(OoRoute.fromLocation(routeInformation.uri));
    } on FormatException catch (error) {
      onUnknown(error.message);
      return SynchronousFuture(const OoRoute(OoPage.overview));
    }
  }

  @override
  RouteInformation restoreRouteInformation(OoRoute configuration) =>
      RouteInformation(uri: Uri.parse(configuration.location));
}

/// Holds the current [OoRoute] and builds the app for it: [shell] for every page, and
/// [memory] above the page the person came from while the route is `memory`.
///
/// A page change is a new history entry; a change of filters on the same page replaces the
/// current one, so browser back leaves the page instead of undoing each filter.
class OoRouterDelegate extends RouterDelegate<OoRoute>
    with ChangeNotifier, PopNavigatorRouterDelegateMixin<OoRoute> {
  OoRouterDelegate({required this.shell, this.memory});

  /// The app for a route that is not `memory`.
  final Widget Function(BuildContext context, OoRouterDelegate router) shell;

  /// Her memory page; `null` (or no builder) shows only the page behind it.
  final Widget? Function(BuildContext context)? memory;

  @override
  final navigatorKey = GlobalKey<NavigatorState>();

  OoRoute _route = const OoRoute(OoPage.overview);
  OoRoute _behind = const OoRoute(OoPage.overview);
  String? _notice;

  /// The current route.
  OoRoute get route => _route;

  /// The page shown under her memory page, or the current route.
  OoRoute get page => _route.page == OoPage.memory ? _behind : _route;

  /// [unknownLinkNotice] with the reason, while it has not been dismissed.
  String? get notice => _notice;

  @override
  OoRoute get currentConfiguration => _route;

  @override
  Future<void> setNewRoutePath(OoRoute configuration) {
    _set(configuration);
    return SynchronousFuture(null);
  }

  /// Notes that a link opened Overview because it is not a route.
  void unknown(String reason) {
    _notice = '$unknownLinkNotice: $reason';
    notifyListeners();
  }

  void dismissNotice() {
    if (_notice == null) return;
    _notice = null;
    notifyListeners();
  }

  /// Opens [route], from a [context] under the app's [Router].
  void go(BuildContext context, OoRoute route) {
    if (route == _route) return;
    final report = route.page == _route.page ? Router.neglect : Router.navigate;
    report(context, () {
      _notice = null;
      _set(route);
    });
  }

  void _set(OoRoute route) {
    if (route.page != OoPage.memory) _behind = route;
    if (route == _route) return;
    _route = route;
    notifyListeners();
  }

  @override
  Widget build(BuildContext context) {
    final memoryPage = _route.page == OoPage.memory
        ? memory?.call(context)
        : null;
    return Navigator(
      key: navigatorKey,
      pages: [
        MaterialPage<void>(
          key: const ValueKey('shell'),
          child: shell(context, this),
        ),
        if (memoryPage != null)
          MaterialPage<void>(key: const ValueKey('memory'), child: memoryPage),
      ],
      onDidRemovePage: (page) {
        if (page.key == const ValueKey('memory')) _set(_behind);
      },
    );
  }
}
