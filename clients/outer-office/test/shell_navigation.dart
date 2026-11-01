import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart'
    show
        NavigationBar,
        NavigationDrawer,
        Scaffold,
        ScaffoldState,
        Scrollable,
        Tooltip;
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/theme/oo_backdrop.dart';

final _menu = find.byTooltip('Open navigation menu');
final _bar = find.byType(NavigationBar);

/// The narrow window's way to the drawer: the menu button, or a phone's **More**.
Finder get _opener => _bar.evaluate().isEmpty
    ? _menu
    : find.descendant(of: _bar, matching: find.text('More'));

bool get _inDrawer => _menu.evaluate().isNotEmpty || _bar.evaluate().isNotEmpty;

/// A phone's bottom bar entry [label], when it has one.
Finder _inBar(String label) =>
    find.descendant(of: _bar, matching: find.text(label));

/// The rail entry or, in a narrow window with the menu open, the drawer entry [label]. The icon
/// rail names its entries in tooltips.
Finder navLabel(String label) {
  if (_inDrawer) {
    return find.descendant(
      of: find.byType(NavigationDrawer),
      matching: find.text(label),
    );
  }
  final labelled = find.descendant(
    of: find.byType(OoRail),
    matching: find.text(label),
  );
  return labelled.evaluate().isNotEmpty
      ? labelled
      : find.descendant(
          of: find.byType(OoRail),
          matching: find.byWidgetPredicate(
            (widget) => widget is Tooltip && widget.message == label,
          ),
        );
}

/// Opens the page [label] from the rail, or from the menu drawer in a narrow window.
Future<void> openPage(WidgetTester tester, String label) async {
  if (_inBar(label).evaluate().isNotEmpty) {
    await tester.tap(_inBar(label));
    await tester.pumpAndSettle();
    return;
  }
  if (_inDrawer) {
    await tester.tap(_opener);
    await tester.pumpAndSettle();
  }
  // A drawer builds its rows as they come into view.
  if (_inDrawer && navLabel(label).evaluate().isEmpty) {
    await tester.scrollUntilVisible(
      navLabel(label),
      200,
      scrollable: find.descendant(
        of: find.byType(NavigationDrawer),
        matching: find.byType(Scrollable),
      ),
    );
  }
  await tester.ensureVisible(navLabel(label));
  await tester.pumpAndSettle();
  await tester.tap(navLabel(label));
  await tester.pumpAndSettle();
}

/// Whether the rail, or the menu drawer in a narrow window, offers [label]; the drawer is
/// closed again.
Future<bool> offersPage(WidgetTester tester, String label) async {
  if (_inBar(label).evaluate().isNotEmpty) return true;
  // With every page in the bar there is no More.
  final narrow = _inDrawer && _opener.evaluate().isNotEmpty;
  if (narrow) {
    await tester.tap(_opener);
    await tester.pumpAndSettle();
  }
  final found = navLabel(label).evaluate().isNotEmpty;
  if (narrow) {
    tester.firstState<ScaffoldState>(find.byType(Scaffold)).closeDrawer();
    await tester.pumpAndSettle();
  }
  return found;
}

/// Asserts that no wallpaper is painted behind or inside [target], which must be shown.
void expectNoBackdrop(Finder target) {
  expect(target, findsWidgets);
  expect(
    find.ancestor(of: target, matching: find.byType(OoBackdrop)),
    findsNothing,
  );
  expect(
    find.descendant(of: target, matching: find.byType(OoBackdrop)),
    findsNothing,
  );
}
