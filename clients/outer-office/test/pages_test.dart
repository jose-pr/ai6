import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/pages.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/routes/oo_router.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;
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

  /// Whether each ask was for a hand-off.
  final handoffs = <bool>[];
  final remembered = <(String, bool)>[];
  final deleted = <String>[];
  final shared = <(String, List<String>)>[];

  /// What [list] answers.
  List<Map<String, Object?>> pages = const [];

  /// A reason [share] refuses with, when set.
  String? refuse;

  @override
  Future<List<Map<String, Object?>>> list() async => pages;

  final publics = <(String, bool)>[];

  @override
  Future<String?> setPublic(String id, {required bool on}) async {
    publics.add((id, on));
    return on ? 'https://pages.example/s/public-$id/' : null;
  }

  @override
  Uri appLink(String id) =>
      Uri.parse('https://ai6.example/#/pages?page_id=$id');

  @override
  Future<List<String>> share(String id, List<String> subjects) async {
    shared.add((id, subjects));
    if (refuse case final reason?) throw Refused(reason);
    return subjects;
  }

  @override
  Future<PageLink> open(
    String id, {
    String? network,
    bool handoff = true,
  }) async {
    opened.add((id, network));
    handoffs.add(handoff);
    final token = 'token-${opened.length}';
    return PageLink(
      Uri.parse(
        'https://pages.example/v/$token/${handoff ? '?handoff=$token' : ''}',
      ),
      ooClock().millisecondsSinceEpoch / 1000 + 3600,
    );
  }

  @override
  Future<void> remember(String id, {required bool remember}) async =>
      remembered.add((id, remember));

  @override
  Future<void> delete(String id) async => deleted.add(id);
}

Map<String, Object?> fixture({num? expires}) => {
  'id': 'page-1',
  'kind': 'dashboard',
  'title': 'Station load',
  'bytes': 2048,
  'expires_at':
      expires ?? ooClock().millisecondsSinceEpoch / 1000 + 20 * 3600 + 300,
  'feeds': ['stations', 'missions'],
};

Future<(FakeLauncher, FakeActions)> show(
  WidgetTester tester,
  Map<String, Object?> data, {
  bool holdsCookie = false,
}) async {
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
        body: PageCard(
          data: data,
          actions: actions,
          holdsCookie: (_) => holdsCookie,
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return (launcher, actions);
}

void main() {
  sharingTests();
  privacyTests();
  menuTests();
  testWidgets('a page card shows its kind, title, size, expiry and feeds', (
    tester,
  ) async {
    await show(tester, fixture());
    expect(find.text('Station load'), findsOneWidget);
    expect(find.text('dashboard'), findsOneWidget);
    expect(find.text('2.0 KiB'), findsOneWidget);
    expect(find.text('Expires in 20 h'), findsOneWidget);
    expect(find.text('Reads live: stations, missions'), findsOneWidget);
    expect(
      find.text('Opens in a new tab or your browser, outside Outer Office.'),
      findsOneWidget,
    );
  });

  testWidgets('a remembered page says so, and no page names what she read', (
    tester,
  ) async {
    await show(tester, {
      ...fixture(),
      'expires_at': null,
      'exposed': true,
      'exposed_by': ['tool:missions'],
    });
    expect(find.text('Remembered'), findsOneWidget);
    expect(find.textContaining('Expires'), findsNothing);
    expect(find.textContaining('outside AI6'), findsNothing);
    expect(find.textContaining('tool:missions'), findsNothing);
  });

  testWidgets(
    'Open launches the plain address at once in the browser that holds the cookie',
    (tester) async {
      final (launcher, actions) = await show(
        tester,
        fixture(),
        holdsCookie: true,
      );
      expect(actions.opened, [('page-1', null)], reason: 'asked at once');
      expect(actions.handoffs, [false]);
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(launcher.launches, hasLength(1));
      final (url, options) = launcher.launches.single;
      expect(url, 'https://pages.example/v/token-1/');
      expect(options.mode, PreferredLaunchMode.externalApplication);
      expect(options.webOnlyWindowName, '_blank');
      expect(actions.opened, hasLength(1), reason: 'no second ask for a click');
    },
  );

  testWidgets(
    'on the desktop, or a browser with no cookie, Open asks for a hand-off and launches it',
    (tester) async {
      final (launcher, actions) = await show(tester, fixture());
      expect(actions.handoffs, [false], reason: 'none is spent by being shown');
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(actions.handoffs, [false, true]);
      final (url, options) = launcher.launches.single;
      expect(url, 'https://pages.example/v/token-2/?handoff=token-2');
      expect(options.mode, PreferredLaunchMode.externalApplication);
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(actions.handoffs, [false, true, true], reason: 'one for each');
      expect(launcher.launches, hasLength(2));
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
      expect(
        launcher.launches.last.$1,
        'https://pages.example/v/token-2/?handoff=token-2',
      );
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

/// A refusal that reads as the gateway's own words.
class Refused implements Exception {
  const Refused(this.message);

  final String message;

  @override
  String toString() => message;
}

void sharingTests() {
  testWidgets(
    'Share sends one subject a line and the card says whom it reaches',
    (tester) async {
      final (_, actions) = await show(tester, fixture());
      await tester.tap(find.byTooltip('More'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Share…'));
      await tester.pumpAndSettle();
      expect(
        find.text(
          "Each viewer's live data is read with their own permissions.",
        ),
        findsOneWidget,
      );
      await tester.enterText(
        find.byKey(const ValueKey('share-subjects')),
        'group:ops\n\n  operation:G  \n',
      );
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(actions.shared, hasLength(1));
      expect(actions.shared.single.$1, 'page-1');
      expect(actions.shared.single.$2, ['group:ops', 'operation:G']);
      expect(find.text('Shared with group:ops, operation:G'), findsOneWidget);
    },
  );

  testWidgets('a refused share keeps the dialog open with the reason', (
    tester,
  ) async {
    final (_, actions) = await show(tester, {...fixture(), 'kind': 'report'});
    actions.refuse = 'you do not hold mission.read in Operation H';
    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Share…'));
    await tester.pumpAndSettle();
    expect(
      find.textContaining('A report shows what you could read'),
      findsOneWidget,
    );
    await tester.enterText(
      find.byKey(const ValueKey('share-subjects')),
      'operation:H',
    );
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(
      find.text('you do not hold mission.read in Operation H'),
      findsOneWidget,
    );
    expect(find.text('Share this page'), findsOneWidget);
    expect(find.textContaining('Shared with operation'), findsNothing);
  });

  testWidgets('a page shared with you says whose it is and only opens', (
    tester,
  ) async {
    await show(tester, {...fixture(), 'shared_by': 'q-007'});
    expect(find.text('Shared by q-007'), findsOneWidget);
    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();
    expect(find.text('Open with network off'), findsOneWidget);
    for (final absent in ['Remember', 'Forget', 'Share…', 'Delete']) {
      expect(find.text(absent), findsNothing, reason: absent);
    }
  });

  testWidgets('Her pages lists yours, then those shared with you', (
    tester,
  ) async {
    final launcher = FakeLauncher();
    UrlLauncherPlatform.instance = launcher;
    final actions = FakeActions()
      ..pages = [
        {...fixture(), 'shares': <String>[]},
        {
          ...fixture(),
          'id': 'page-2',
          'title': 'Their log',
          'shared_by': 'q-007',
        },
      ];
    tester.view.physicalSize = const Size(800, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(MaterialApp(home: HerPagesPage(actions: actions)));
    await tester.pumpAndSettle();
    final yours = tester.getTopLeft(find.text('Your pages')).dy;
    final mine = tester.getTopLeft(find.text('Station load')).dy;
    final sharedHeading = tester.getTopLeft(find.text('Shared with you')).dy;
    final theirs = tester.getTopLeft(find.text('Their log')).dy;
    expect(
      yours < mine && mine < sharedHeading && sharedHeading < theirs,
      isTrue,
    );
    expect(find.text('Shared by q-007'), findsOneWidget);
    await tester.tap(find.text('Open').last);
    await tester.pumpAndSettle();
    expect(launcher.launches.single.$1, contains('token-'));
  });
}

void privacyTests() {
  testWidgets(
    'a public link is made only after the dialog says what it gives',
    (tester) async {
      final (_, actions) = await show(tester, fixture());
      expect(find.textContaining('Public:'), findsNothing);
      await tester.tap(find.byTooltip('More'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Public link…'));
      await tester.pumpAndSettle();
      expect(find.text('Make this page public?'), findsOneWidget);
      expect(find.textContaining('without signing in'), findsOneWidget);
      expect(find.textContaining('It shows no live data'), findsOneWidget);
      expect(
        actions.publics,
        isEmpty,
        reason: 'nothing before the person says',
      );
      await tester.tap(find.text('Make public'));
      await tester.pumpAndSettle();
      expect(actions.publics.single, ('page-1', true));
      expect(
        find.text('Public: anyone with its public link sees it'),
        findsOneWidget,
      );
      // Opened again, it shows the link and takes it away.
      await tester.tap(find.byTooltip('More'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Public link…'));
      await tester.pumpAndSettle();
      expect(
        find.text('https://pages.example/s/public-page-1/'),
        findsOneWidget,
      );
      await tester.tap(find.text('Turn off'));
      await tester.pumpAndSettle();
      expect(actions.publics.last, ('page-1', false));
      expect(find.textContaining('Public:'), findsNothing);
    },
  );

  testWidgets('the share dialog gives the app link to send', (tester) async {
    await show(tester, fixture());
    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Share…'));
    await tester.pumpAndSettle();
    expect(
      find.textContaining('https://ai6.example/#/pages?page_id=page-1'),
      findsOneWidget,
    );
    expect(find.byTooltip('Copy the link'), findsOneWidget);
  });

  testWidgets(
    'a link to one page shows it first, and a hidden one as missing',
    (tester) async {
      final launcher = FakeLauncher();
      UrlLauncherPlatform.instance = launcher;
      final actions = FakeActions()
        ..pages = [
          {...fixture(), 'id': 'page-0', 'title': 'Older'},
          {...fixture(), 'title': 'Sent to me', 'shared_by': 'q-007'},
        ];
      tester.view.physicalSize = const Size(800, 1600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(
        MaterialApp(
          home: HerPagesPage(actions: actions, focus: 'page-1'),
        ),
      );
      await tester.pumpAndSettle();
      final sent = tester.getTopLeft(find.text('The page you were sent')).dy;
      final card = tester.getTopLeft(find.text('Sent to me')).dy;
      final yours = tester.getTopLeft(find.text('Your pages')).dy;
      expect(sent < card && card < yours, isTrue);
      expect(find.text('Nobody has shared a page with you.'), findsOneWidget);
      await tester.tap(find.text('Open').first);
      await tester.pumpAndSettle();
      expect(launcher.launches, hasLength(1));

      await tester.pumpWidget(
        MaterialApp(
          home: HerPagesPage(actions: actions, focus: 'page-x'),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        find.text(
          'There is no page page-x you can open: it is gone, or it is not yours and '
          'nobody shared it with you.',
        ),
        findsOneWidget,
      );
    },
  );

  test('a page link is a route of its own, and the app writes it back', () {
    final route = OoRoute.fromLocation(Uri.parse('/pages?page_id=page-1'));
    expect(route.page, OoPage.pages);
    expect(route['page_id'], 'page-1');
    expect(route.location, '/pages?page_id=page-1');
  });

  testWidgets('the router opens Her pages at the page a link names', (
    tester,
  ) async {
    final delegate = OoRouterDelegate(
      shell: (context, router) => const Text('the shell'),
      pages: (context, pageId) => Text('her pages at $pageId'),
    );
    addTearDown(delegate.dispose);
    await tester.pumpWidget(
      MaterialApp.router(
        routerDelegate: delegate,
        routeInformationParser: OoRouteParser(onUnknown: (_) {}),
        routeInformationProvider: PlatformRouteInformationProvider(
          initialRouteInformation: RouteInformation(
            uri: Uri.parse('/pages?page_id=page-1'),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('her pages at page-1'), findsOneWidget);
    expect(delegate.page.page, OoPage.overview, reason: 'the page behind it');
    // Before sign-in the builder gives nothing, and the page behind it shows.
    final signedOut = OoRouterDelegate(
      shell: (context, router) => const Text('sign in'),
      pages: (context, pageId) => null,
    );
    addTearDown(signedOut.dispose);
    await tester.pumpWidget(
      MaterialApp.router(
        routerDelegate: signedOut,
        routeInformationParser: OoRouteParser(onUnknown: (_) {}),
        routeInformationProvider: PlatformRouteInformationProvider(
          initialRouteInformation: RouteInformation(
            uri: Uri.parse('/pages?page_id=page-1'),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('sign in'), findsOneWidget);
    expect(
      signedOut.route.page,
      OoPage.pages,
      reason: 'kept for after sign-in',
    );
  });
}

/// A chat socket that stays open and says nothing.
class QuietChannel implements AssistantChannel {
  final _incoming = StreamController<String>();

  @override
  Stream<String> get incoming => _incoming.stream;

  @override
  void send(String frame) {}

  @override
  Future<void> close() => _incoming.close();
}

void menuTests() {
  testWidgets("her chat's menu opens Her pages", (tester) async {
    final launcher = FakeLauncher();
    UrlLauncherPlatform.instance = launcher;
    final actions = FakeActions()..pages = [fixture()];
    final controller = AssistantController(
      token: 'user-token',
      connect: QuietChannel.new,
    );
    addTearDown(controller.dispose);
    tester.view.physicalSize = const Size(800, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AssistantPanel(controller: controller, pageActions: actions),
        ),
      ),
    );
    await tester.pump();
    await tester.tap(find.byTooltip('Moneypenny menu'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Her pages'));
    await tester.pumpAndSettle();
    expect(find.text('Your pages'), findsOneWidget);
    expect(find.text('Station load'), findsOneWidget);
  });
}
