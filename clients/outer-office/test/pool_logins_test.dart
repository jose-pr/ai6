import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/attention_summary.dart';
import 'package:outer_office/shell/oo_freshness.dart';
import 'package:outer_office/theme/oo_icon.dart';

import 'attention_fake.dart';
import 'pools_test.dart' show account;
import 'routes_test.dart' show locations;
import 'work_views_test.dart' show openAt;

/// Unix seconds the tests' clock stands at.
const _now = 1_800_000_000.0;

Map<String, Object?> _healthy() => {
  ...account('alpha'),
  'login': {
    'stations': ['station-a', 'station-b'],
    'expires_at': _now + 2 * 86400,
    'renewed_at': _now - 3 * 3600,
    'renewal_held_by': 'station-a',
    'renewal_held_until': _now + 240,
    'last_result': 'renewed',
    'last_attempt_at': _now - 3 * 3600,
    'failures': 0,
    'needs_person': null,
    'needs_person_command': null,
    'requests': [
      {
        'action': 'add_station',
        'station': 'station-c',
        'by': 'ana',
        'at': _now - 60,
      },
    ],
  },
};

Map<String, Object?> _needsPerson() => {
  ...account('beta'),
  'login': {
    'stations': <String>[],
    'needs_person': 'no login is kept for this account',
    'needs_person_command': 'ai6 pools add-login team beta --handler claude',
    'failures': 0,
    'requests': <Object?>[],
  },
};

Future<MissionBoard> _board(
  List<http.Request> writes, {
  List<String> actions = const ['mission.read', 'pool.read'],
  int writeStatus = 202,
}) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      final path = request.url.path;
      if (path == '/v1/auth/login') {
        return http.Response(jsonEncode({'token': 'user-token'}), 201);
      }
      if (request.method != 'GET') {
        writes.add(request);
        return http.Response('secret body', writeStatus);
      }
      if (path.endsWith('/pools')) {
        return http.Response(
          jsonEncode({
            'operation_id': 'G',
            'accounts': [_healthy(), _needsPerson()],
          }),
          200,
        );
      }
      return http.Response(switch (path) {
        '/v1/board' || '/v1/missions' || '/v1/user/audit' => '[]',
        '/v1/stations' => jsonEncode({
          'headquarters_station': null,
          'stations': [],
        }),
        '/v1/user/profile' => jsonEncode({
          'person_id': 'operator-007',
          'name': 'operator',
          'display_name': null,
          'groups': [],
          'expires_at': 1_900_000_000,
          'operations': [
            {'operation_id': 'G', 'actions': actions},
          ],
          'roster': [],
          'stations': [],
        }),
        _ => throw StateError('unexpected request to $path'),
      }, 200);
    }),
  );
  await board.login('operator', 'secret');
  return board;
}

const _manage = ['mission.read', 'pool.read', 'pool.manage'];

void main() {
  setUp(() {
    ooClock = () =>
        DateTime.fromMillisecondsSinceEpoch((_now * 1000).round(), isUtc: true);
    addTearDown(() => ooClock = DateTime.now);
  });

  for (final (name, size) in [
    ('desktop', const Size(1280, 800)),
    ('phone', const Size(360, 740)),
  ]) {
    testWidgets('the login state renders without overflow at $name width', (
      tester,
    ) async {
      final board = await _board([]);
      addTearDown(board.dispose);
      await openAt(tester, board, '/pools', size: size);

      expect(
        find.textContaining('Renewal held by station-a until in 4 min'),
        findsOneWidget,
      );
      expect(find.text('Renewed 3 h ago'), findsOneWidget);
      expect(find.text('Ends in 2 d'), findsOneWidget);
      expect(find.text('May be held by station-a, station-b'), findsOneWidget);
      expect(
        find.text(
          'Waiting for a Station holding the login: adding Station station-c',
        ),
        findsOneWidget,
      );
      final command = find.textContaining('Run `ai6 pools add-login');
      if (command.evaluate().isEmpty) {
        await tester.scrollUntilVisible(
          command,
          200,
          scrollable: find.byType(Scrollable).last,
        );
      }
      expect(
        find.textContaining(
          'Run `ai6 pools add-login team beta --handler claude` on a host '
          'where you are signed in.',
        ),
        findsOneWidget,
      );
      expect(
        find.textContaining('Needs a person: no login is kept'),
        findsOneWidget,
      );
      expect(find.byIcon(Icons.warning_amber), findsWidgets);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('the actions show only with pool.manage', (tester) async {
    final board = await _board([]);
    addTearDown(board.dispose);
    await openAt(tester, board, '/pools');
    expect(find.text('Add Station'), findsNothing);
    expect(find.text('Remove login'), findsNothing);
    expect(find.byType(InputChip), findsNothing);

    final manager = await _board([], actions: _manage);
    addTearDown(manager.dispose);
    await openAt(tester, manager, '/pools');
    expect(find.text('Add Station'), findsNWidgets(2));
    expect(find.text('Remove login'), findsNWidgets(2));
    expect(find.byType(InputChip), findsNWidgets(2));
  });

  testWidgets('adding and removing a Station, and removing the login, ask the '
      'gateway', (tester) async {
    final writes = <http.Request>[];
    final board = await _board(writes, actions: _manage);
    addTearDown(board.dispose);
    await openAt(tester, board, '/pools', size: const Size(1280, 1200));

    await tester.tap(find.text('Add Station').first);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).last, 'station-d');
    await tester.tap(find.widgetWithText(FilledButton, 'Add'));
    await tester.pumpAndSettle();
    expect(writes.last.method, 'POST');
    expect(
      writes.last.url.path,
      '/v1/user/operations/G/pools/team/alpha/login/stations',
    );
    expect(jsonDecode(writes.last.body), {'station': 'station-d'});
    expect(
      find.textContaining('Asked a Station holding the login'),
      findsOneWidget,
    );

    await tester.tap(find.byTooltip('Remove Station station-b'));
    await tester.pumpAndSettle();
    expect(writes.last.method, 'DELETE');
    expect(
      writes.last.url.path,
      '/v1/user/operations/G/pools/team/alpha/login/stations/station-b',
    );

    await tester.tap(find.text('Remove login').first);
    await tester.pumpAndSettle();
    expect(find.textContaining('is forgotten'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, 'Remove login'));
    await tester.pumpAndSettle();
    expect(writes.last.method, 'DELETE');
    expect(
      writes.last.url.path,
      '/v1/user/operations/G/pools/team/alpha/login',
    );
    expect(writes.last.url.query, isEmpty);

    await tester.tap(find.text('Remove login').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Also sign out at the provider'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Remove login'));
    await tester.pumpAndSettle();
    expect(
      writes.last.url.path,
      '/v1/user/operations/G/pools/team/alpha/login',
    );
    expect(writes.last.url.queryParameters, {'sign_out': 'true'});
    expect(writes, hasLength(4));
  });

  testWidgets('a refusal shows its status, never its body', (tester) async {
    final writes = <http.Request>[];
    final board = await _board(writes, actions: _manage, writeStatus: 403);
    addTearDown(board.dispose);
    await openAt(tester, board, '/pools', size: const Size(1280, 1200));
    await tester.tap(find.byTooltip('Remove Station station-b'));
    await tester.pumpAndSettle();
    expect(find.textContaining('HTTP 403'), findsOneWidget);
    expect(find.textContaining('secret body'), findsNothing);
  });

  testWidgets('a login_needs_person item shows in the bell with the pools '
      'icon and opens the pool page', (tester) async {
    final board = await _board([], actions: _manage);
    addTearDown(board.dispose);
    board.attention = AttentionSummary.fromJson(
      attentionBody(
        items: [
          attentionEntry(
            id: 'login_needs_person:G:team/beta',
            kind: 'login_needs_person',
            title: 'The login of team/beta needs a person',
            route: {'page': 'pools', 'operation_id': 'G', 'pool': 'team'},
            scope: 'operation:G',
          ),
        ],
        coverage: [attentionRead('logins', 'Logins', 'checked')],
      ),
    );
    final reported = await openAt(tester, board, '/');
    await tester.tap(find.byTooltip('Needs attention: 1'));
    await tester.pumpAndSettle();
    final item = find.descendant(
      of: find.byType(MenuItemButton),
      matching: find.textContaining('team/beta needs a person'),
    );
    expect(item, findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(MenuItemButton),
        matching: find.byWidgetPredicate(
          (widget) => widget is OoIcon && widget.name == OoIconName.pools,
        ),
      ),
      findsOneWidget,
    );
    await tester.tap(item);
    await tester.pumpAndSettle();
    expect(locations(reported).last.$1, '/pools?operation_id=G&pool=team');
  });
}
