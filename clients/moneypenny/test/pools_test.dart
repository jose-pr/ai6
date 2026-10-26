import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart'
    show DropdownButton, LinearProgressIndicator, NavigationBar;
import 'package:moneypenny/main.dart';

Finder navLabel(String label) =>
    find.descendant(of: find.byType(NavigationBar), matching: find.text(label));

Map<String, Object?> account(
  String name, {
  List<String> missions = const [],
  int others = 0,
  int? max,
  int? used,
  double? limitedUntil,
  int tokens = 0,
}) => {
  'pool': 'team',
  'account': name,
  'handler': 'claude',
  'kind': 'claude-oauth-token',
  'priority': 0,
  'max_concurrent': max,
  'labels': <String, String>{},
  'active': [
    for (final mission in missions)
      {'station_id': 'station-alpha', 'mission_id': mission, 'since': 1.0},
  ],
  'other_sessions': others,
  'windows': [
    if (used != null)
      {
        'name': 'five_hour',
        'used_permille': used,
        'resets_at': 1_900_000_000.0,
        'status': null,
      },
  ],
  'limited_until': limitedUntil,
  'last_used': null,
  'tokens_today': tokens,
};

Future<MissionBoard> signedIn(
  List<Uri> asked, {
  List<String> actions = const ['mission.read', 'pool.read'],
}) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://moneypenny.example'),
    pollInterval: null,
    client: MockClient((request) async {
      final path = request.url.path;
      if (path == '/v1/auth/login') {
        return http.Response(jsonEncode({'token': 'user-token'}), 201);
      }
      expect(request.headers['authorization'], 'Bearer user-token');
      if (path.endsWith('/pools')) {
        asked.add(request.url);
        final operation = request.url.pathSegments[3];
        return http.Response(
          jsonEncode({
            'operation_id': operation,
            'accounts': operation == 'G'
                ? [
                    account(
                      'alpha',
                      missions: ['mission-1'],
                      others: 1,
                      max: 2,
                      used: 425,
                      tokens: 1200,
                    ),
                    account('beta', used: 1000, limitedUntil: 1_900_000_000),
                  ]
                : [],
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
            {'operation_id': 'L', 'actions': actions},
          ],
          'stations': [],
        }),
        _ => throw StateError('unexpected request to $path'),
      }, 200);
    }),
  );
  await board.login('operator', 'secret');
  return board;
}

void main() {
  testWidgets('account pools show use, limits and tokens per Operation', (
    tester,
  ) async {
    final asked = <Uri>[];
    final board = await signedIn(asked);
    addTearDown(board.dispose);
    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    await tester.tap(navLabel('Account pools'));
    await tester.pumpAndSettle();

    expect(asked.single.path, '/v1/user/operations/G/pools');
    expect(find.text('team/alpha'), findsOneWidget);
    expect(
      find.text('2/2 Sessions · five_hour 42.5% · 1200 tokens today'),
      findsOneWidget,
    );
    expect(find.text('Used by mission-1'), findsOneWidget);
    expect(
      find.text(
        '0 Sessions · five_hour 100.0% · 0 tokens today · '
        'Limited until 2030-03-17T17:46Z',
      ),
      findsOneWidget,
    );
    expect(find.byType(LinearProgressIndicator), findsNWidgets(2));

    await tester.tap(find.byType(DropdownButton<String>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('L').last);
    await tester.pumpAndSettle();
    expect(asked.last.path, '/v1/user/operations/L/pools');
    expect(find.text('No pool account serves this Operation.'), findsOneWidget);
  });

  testWidgets('account pools need pool.read', (tester) async {
    final asked = <Uri>[];
    final board = await signedIn(asked, actions: const ['mission.read']);
    addTearDown(board.dispose);
    await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
    expect(navLabel('Account pools'), findsNothing);
    expect(asked, isEmpty);
  });
}
