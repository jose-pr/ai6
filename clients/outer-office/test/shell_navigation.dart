import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart'
    show NavigationDrawer, Scaffold, ScaffoldState;
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/theme/oo_backdrop.dart';

final _menu = find.byTooltip('Open navigation menu');

/// The rail entry or, in a narrow window with the menu open, the drawer entry [label].
Finder navLabel(String label) => find.descendant(
  of: _menu.evaluate().isEmpty
      ? find.byType(OoRail)
      : find.byType(NavigationDrawer),
  matching: find.text(label),
);

/// Opens the page [label] from the rail, or from the menu drawer in a narrow window.
Future<void> openPage(WidgetTester tester, String label) async {
  if (_menu.evaluate().isNotEmpty) {
    await tester.tap(_menu);
    await tester.pumpAndSettle();
  }
  await tester.tap(navLabel(label));
  await tester.pumpAndSettle();
}

/// Whether the rail, or the menu drawer in a narrow window, offers [label]; the drawer is
/// closed again.
Future<bool> offersPage(WidgetTester tester, String label) async {
  final narrow = _menu.evaluate().isNotEmpty;
  if (narrow) {
    await tester.tap(_menu);
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
