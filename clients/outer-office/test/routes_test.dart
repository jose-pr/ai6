import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart' show ValueKey;
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/routes/oo_router.dart';
import 'package:outer_office/shell/oo_shell.dart';

import 'shell_navigation.dart';

/// A demo board whose Missions are G's `g-failed` (failed), G's `g-working` and L's
/// `l-failed` (failed), opened at the web location [location] (the URL after `#`).
Future<List<MethodCall>> openAt(WidgetTester tester, String location) async {
  tester.view.physicalSize = const Size(1400, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  tester.binding.platformDispatcher.defaultRouteNameTestValue = location;
  addTearDown(tester.binding.platformDispatcher.clearDefaultRouteNameTestValue);
  final reported = <MethodCall>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.navigation,
    (call) async {
      reported.add(call);
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.navigation,
      null,
    ),
  );
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    demo: true,
    pollInterval: null,
  );
  addTearDown(board.dispose);
  board.sessions = const [
    SessionSummary(
      operationId: 'G',
      missionId: 'g-failed',
      sessionId: 's-g-failed',
      state: 'failed',
      sequence: 3,
    ),
    SessionSummary(
      operationId: 'G',
      missionId: 'g-working',
      sessionId: 's-g-working',
      state: 'working',
      sequence: 2,
    ),
    SessionSummary(
      operationId: 'L',
      missionId: 'l-failed',
      sessionId: 's-l-failed',
      state: 'failed',
      sequence: 4,
    ),
  ];
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  return reported;
}

/// The location and whether it replaced the history entry, of each route the app reported.
List<(String, bool)> locations(List<MethodCall> reported) => [
  for (final call in reported)
    if (call.method == 'routeInformationUpdated')
      (
        (call.arguments as Map)['uri'] as String,
        (call.arguments as Map)['replace'] as bool,
      ),
];

/// What the engine sends when the browser goes back to [location].
Future<void> browserBackTo(WidgetTester tester, String location) async {
  await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
    SystemChannels.navigation.name,
    SystemChannels.navigation.codec.encodeMethodCall(
      MethodCall('pushRouteInformation', {'location': location, 'state': null}),
    ),
    (_) {},
  );
  await tester.pumpAndSettle();
}

/// Valid routes in both forms and invalid ones with their reason; Universal Exports'
/// `routes.rs` tests read the same file.
Map<String, Object?> _fixture() => (jsonDecode(
  File('test/fixtures/assistant_routes.json').readAsStringSync(),
) as Map).cast<String, Object?>();

void main() {
  test('every valid route of the fixture round-trips between both forms', () {
    final valid = _fixture()['valid']! as List;
    expect(valid.length, greaterThanOrEqualTo(OoPage.values.length));
    final pages = <OoPage>{};
    for (final raw in valid) {
      final json = (raw as Map)['json'];
      final uri = raw['uri'] as String;
      final fromJson = OoRoute.fromJson(json);
      expect(fromJson.toUri(), uri);
      final fromUri = OoRoute.parseUri(uri);
      expect(fromUri, fromJson, reason: uri);
      expect(fromUri.toJson(), json, reason: uri);
      expect(OoRoute.fromLocation(Uri.parse(fromUri.location)), fromUri);
      pages.add(fromUri.page);
    }
    expect(pages, OoPage.values.toSet(), reason: 'every page has a case');
  });

  test('every invalid route of the fixture is refused with its reason', () {
    final invalid = _fixture()['invalid']! as List;
    expect(invalid, isNotEmpty);
    for (final raw in invalid) {
      final reason = (raw as Map)['reason'] as String;
      final String refused;
      try {
        if (raw.containsKey('json')) {
          OoRoute.fromJson(raw['json']);
        } else {
          OoRoute.parseUri(raw['uri'] as String);
        }
        fail('accepted $raw');
      } on FormatException catch (error) {
        refused = error.message;
      }
      expect(refused, reason, reason: '$raw');
    }
  });

  test(
    'a web location is the link form after the hash; the bare one is Overview',
    () {
      expect(
        OoRoute.fromLocation(
          Uri.parse('/missions?state=failed&operation_id=G'),
        ),
        OoRoute.parseUri('app:/missions?operation_id=G&state=failed'),
      );
      expect(
        OoRoute.fromLocation(Uri.parse('/')),
        const OoRoute(OoPage.overview),
      );
      expect(
        () => OoRoute.fromLocation(Uri.parse('/missions/G/mission-7')),
        throwsFormatException,
      );
    },
  );

  test('a filter is set, removed and cleared, keeping the tab', () {
    final route = OoRoute.parseUri(
      'app:/missions?operation_id=G&mission_id=m-1',
    );
    expect(route.without('operation_id').toUri(), 'app:/missions');
    final states = route.withField('state', ['failed', 'done']);
    expect(states.states, ['failed', 'done']);
    expect(states['state'], 'failed,done');
    expect(
      states.toUri(),
      'app:/missions?operation_id=G&state=failed,done&mission_id=m-1',
    );
    expect(states.withField('state', const <String>[]), route);
    expect(() => route.withField('token', 'x'), throwsFormatException);
    final roles = OoRoute.parseUri('app:/agents?tab=roles&operation_id=G');
    expect(roles.cleared.toUri(), 'app:/agents?tab=roles');
    expect(roles.cleared.filtered, isFalse);
    expect(roles.filtered, isTrue);
  });

  testWidgets('a web link opens its filtered page, and browser back returns to '
      'the page before', (tester) async {
    final reported = await openAt(
      tester,
      '/missions?operation_id=G&state=failed',
    );
    expect(find.text('Mission g-failed'), findsOneWidget);
    expect(find.text('Mission g-working'), findsNothing);
    expect(find.text('Mission l-failed'), findsNothing);
    expect(find.byKey(const ValueKey('filter-operation_id')), findsOneWidget);
    expect(find.byKey(const ValueKey('filter-state')), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('filter-sort')), findsNothing);
    expect(find.text('Clear filters'), findsOneWidget);

    // Another page is a new history entry.
    await openPage(tester, 'Sessions');
    expect(locations(reported).last, (
      '/sessions',
      false,
    ), reason: 'a page change pushes');
    await browserBackTo(tester, '/missions?operation_id=G&state=failed');
    expect(find.text('Mission g-failed'), findsOneWidget);
    expect(find.text('Mission l-failed'), findsNothing);
    expect(find.byKey(const ValueKey('filter-state')), findsOneWidget);

    // A filter of the same page replaces the entry.
    await tester.tap(
      find.descendant(
        of: find.byKey(const ValueKey('filter-operation_id')),
        matching: find.byTooltip('Remove this filter'),
      ),
    );
    await tester.pumpAndSettle();
    expect(locations(reported).last, ('/missions?state=failed', true));
    expect(find.text('Mission g-failed'), findsOneWidget);
    expect(find.text('Mission l-failed'), findsOneWidget);
    expect(find.text('Mission g-working'), findsNothing);
    await tester.tap(find.text('Clear filters'));
    await tester.pumpAndSettle();
    expect(locations(reported).last, ('/missions', true));
    expect(find.text('Mission g-working'), findsOneWidget);
  });

  testWidgets('a selected Mission the board does not hold says so', (
    tester,
  ) async {
    await openAt(tester, '/missions?operation_id=G&mission_id=gone');
    expect(
      find.text('Mission gone of Operation G is not on this board.'),
      findsOneWidget,
    );
  });

  testWidgets('a web link that is not a route opens Overview and says so', (
    tester,
  ) async {
    await openAt(tester, '/missions/G/g-failed');
    expect(find.text('Command overview'), findsOneWidget);
    expect(
      find.text('$unknownLinkNotice: unknown page missions/G/g-failed'),
      findsOneWidget,
    );
    await tester.tap(find.text('Dismiss'));
    await tester.pumpAndSettle();
    expect(find.textContaining(unknownLinkNotice), findsNothing);
  });

  testWidgets('Copy link copies the gateway origin and the route after #', (
    tester,
  ) async {
    await openAt(tester, '/sessions?state=working');
    expect(find.text('s-g-working'), findsOneWidget);
    expect(find.text('s-g-failed'), findsNothing);
    String? copied;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied = (call.arguments as Map)['text'] as String;
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await tester.tap(find.byTooltip('Copy link to this page'));
    await tester.pump();
    expect(copied, 'https://oo.example/#/sessions?state=working');
  });
}
