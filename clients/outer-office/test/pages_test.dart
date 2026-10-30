import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/pages.dart';
import 'package:outer_office/shell/oo_freshness.dart';
import 'package:plugin_platform_interface/plugin_platform_interface.dart';
import 'package:url_launcher_platform_interface/link.dart';
import 'package:url_launcher_platform_interface/url_launcher_platform_interface.dart';

/// A launcher that records what it was asked and opens nothing.
class FakeLauncher extends UrlLauncherPlatform with MockPlatformInterfaceMixin {
  final launches = <(String, LaunchOptions)>[];

  @override
  LinkDelegate? get linkDelegate => null;

  @override
  Future<bool> canLaunch(String url) async => true;

  @override
  Future<bool> launchUrl(String url, LaunchOptions options) async {
    launches.add((url, options));
    return true;
  }
}

class FakeActions implements PageActions {
  final opened = <(String, String?)>[];
  final remembered = <(String, bool)>[];
  final deleted = <String>[];

  @override
  Future<PageLink> open(String id, {String? network}) async {
    opened.add((id, network));
    return PageLink(
      Uri.parse('https://pages.example/v/token-${opened.length}/'),
      ooClock().millisecondsSinceEpoch / 1000 + 3600,
    );
  }

  @override
  Future<void> remember(String id, {required bool remember}) async =>
      remembered.add((id, remember));

  @override
  Future<void> delete(String id) async => deleted.add(id);
}

Map<String, Object?> fixture({bool exposed = false, num? expires}) => {
  'id': 'page-1',
  'kind': 'dashboard',
  'title': 'Station load',
  'bytes': 2048,
  'expires_at':
      expires ?? ooClock().millisecondsSinceEpoch / 1000 + 20 * 3600 + 300,
  'exposed': exposed,
  'exposed_by': exposed
      ? ['tool:missions', 'tool:WebFetch docs.example']
      : <String>[],
  'feeds': ['stations', 'missions'],
};

Future<(FakeLauncher, FakeActions)> show(
  WidgetTester tester,
  Map<String, Object?> data,
) async {
  final launcher = FakeLauncher();
  UrlLauncherPlatform.instance = launcher;
  final actions = FakeActions();
  tester.view.physicalSize = const Size(800, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: PageCard(data: data, actions: actions),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return (launcher, actions);
}

void main() {
  testWidgets('a page card shows its kind, title, size, expiry and feeds', (
    tester,
  ) async {
    await show(tester, fixture());
    expect(find.text('Station load'), findsOneWidget);
    expect(find.text('dashboard'), findsOneWidget);
    expect(find.text('2.0 KiB'), findsOneWidget);
    expect(find.text('Expires in 20 h'), findsOneWidget);
    expect(find.text('Reads live: stations, missions'), findsOneWidget);
    expect(find.byKey(const ValueKey('page-exposed')), findsNothing);
    expect(
      find.text('Opens in a new tab or your browser, outside Outer Office.'),
      findsOneWidget,
    );
  });

  testWidgets(
    'a remembered page says so, and an exposed one names what she read',
    (tester) async {
      await show(tester, {...fixture(exposed: true), 'expires_at': null});
      expect(find.text('Remembered'), findsOneWidget);
      expect(find.textContaining('Expires'), findsNothing);
      expect(
        find.text(
          'Made after she read text from outside AI6: tool:missions, '
          'tool:WebFetch docs.example',
        ),
        findsOneWidget,
      );
    },
  );

  testWidgets(
    'Open launches the link made when the card was shown, outside the app',
    (tester) async {
      final (launcher, actions) = await show(tester, fixture());
      expect(actions.opened, [('page-1', null)], reason: 'minted at once');
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(launcher.launches, hasLength(1));
      final (url, options) = launcher.launches.single;
      expect(url, 'https://pages.example/v/token-1/');
      expect(options.mode, PreferredLaunchMode.externalApplication);
      expect(options.webOnlyWindowName, '_blank');
      expect(
        actions.opened,
        hasLength(1),
        reason: 'no second link for a click',
      );
    },
  );

  testWidgets(
    'the menu opens with the network off, remembers, forgets and deletes',
    (tester) async {
      final (launcher, actions) = await show(tester, fixture());
      Future<void> choose(String label) async {
        await tester.tap(find.byTooltip('More'));
        await tester.pumpAndSettle();
        await tester.tap(find.text(label));
        await tester.pumpAndSettle();
      }

      await choose('Open with network off');
      expect(actions.opened.last, ('page-1', 'none'));
      expect(launcher.launches.last.$1, 'https://pages.example/v/token-2/');
      await choose('Remember');
      expect(actions.remembered, [('page-1', true)]);
      expect(find.text('Remembered'), findsOneWidget);
      await choose('Forget');
      expect(actions.remembered.last, ('page-1', false));
      expect(find.text('Remembered'), findsNothing);
      await choose('Delete');
      expect(actions.deleted, ['page-1']);
      expect(find.text('Page deleted.'), findsOneWidget);
    },
  );
}
