import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart' show FilledButton, TextField;
import 'package:outer_office/main.dart';

/// The Mission's UID: what its route and every link keep through a rename.
const uid = '019a1b2c-3d4e-7f00-8123-456789abcdef';

/// A gateway that knows one Mission of Operation G, called `G-1` and earlier `G-old`, and
/// renames it to any handle but `G-taken`; every request is kept in [requests].
MissionBoard gateway(
  List<http.Request> requests, {
  List<String> actions = const ['mission.read', 'mission.control'],
}) {
  var handle = 'G-1';
  Map<String, Object?> mission() => {
    'operation_id': 'G',
    'mission_id': uid,
    'handle': handle,
    'sessions': const [],
    'brief': null,
    'usage': null,
    'attempts': [
      {
        'attempt': 1,
        'session_id': '019a1b2c-3d4e-7f00-8123-000000000001',
        'handle': 'G-1.1',
        'station': 'hq',
        'status': 'active',
      },
    ],
  };
  return MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      requests.add(request);
      final path = request.url.path;
      if (path == '/v1/auth/login') {
        return http.Response(jsonEncode({'token': 'user-token'}), 201);
      }
      if (path == '/v1/user/operations/G/missions/$uid/rename') {
        final wanted =
            (jsonDecode(request.body) as Map<String, dynamic>)['handle']
                as String;
        if (wanted == 'G-taken') {
          return http.Response(
            jsonEncode({'error': 'handle G-taken is taken by another Mission'}),
            409,
            headers: {'content-type': 'application/json'},
          );
        }
        final previous = handle;
        handle = wanted;
        return http.Response(
          jsonEncode({
            'operation_id': 'G',
            'mission_id': uid,
            'handle': handle,
            'previous': previous,
          }),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      if (path == '/v1/missions') {
        final asked = request.url.queryParameters['mission'];
        final names =
            asked == null ||
            const ['G-old', 'g-old'].contains(asked) ||
            asked == uid ||
            asked.toLowerCase() == handle.toLowerCase();
        return http.Response(
          jsonEncode([if (names) mission()]),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      return http.Response(
        switch (path) {
          '/v1/board' => jsonEncode([
            {
              'operation_id': 'G',
              'mission_id': uid,
              'session_id': 'provider-session-1',
              'state': 'working',
              'sequence': 1,
              'updated_at': 100.0,
              'station_id': 'hq',
            },
          ]),
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
            'stations': [],
          }),
          _ => '[]',
        },
        200,
        headers: {'content-type': 'application/json'},
      );
    }),
  );
}

/// Opens the app, signed in to [board], at the web location [location]; returns what the app
/// reported of its routes.
Future<List<MethodCall>> openAt(
  WidgetTester tester,
  MissionBoard board,
  String location,
) async {
  tester.view.physicalSize = const Size(1400, 1000);
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
  await board.login('operator', 'secret');
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  return reported;
}

String? lastLocation(List<MethodCall> reported) => [
  for (final call in reported)
    if (call.method == 'routeInformationUpdated')
      (call.arguments as Map)['uri'] as String,
].lastOrNull;

void main() {
  testWidgets('a rename shows the new handle on the board and the detail while '
      'the route keeps the UID', (tester) async {
    final requests = <http.Request>[];
    final board = gateway(requests);
    addTearDown(board.dispose);
    final reported = await openAt(
      tester,
      board,
      '/missions?operation_id=G&mission_id=$uid',
    );
    expect(find.text('G › G-1'), findsOneWidget);
    expect(find.text('Mission G-1'), findsOneWidget);
    expect(
      find.textContaining(uid),
      findsNothing,
      reason: 'people see the handle',
    );

    await tester.tap(find.text('Rename'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).last, 'G-taken');
    await tester.tap(find.widgetWithText(FilledButton, 'Rename'));
    await tester.pumpAndSettle();
    expect(
      find.text('handle G-taken is taken by another Mission'),
      findsOneWidget,
      reason: 'a refusal shows in the gateway\'s own words',
    );

    await tester.enterText(find.byType(TextField).last, 'G-build-fix');
    await tester.tap(find.widgetWithText(FilledButton, 'Rename'));
    await tester.pumpAndSettle();
    expect(
      find.byType(FilledButton),
      findsNothing,
      reason: 'the dialog closed',
    );
    expect(find.text('G › G-build-fix'), findsOneWidget);
    expect(find.text('Mission G-build-fix'), findsOneWidget);
    expect(find.text('G › G-1'), findsNothing);
    final renames = requests.where(
      (request) => request.url.path.endsWith('/rename'),
    );
    expect(renames.map((request) => request.url.path).toSet(), {
      '/v1/user/operations/G/missions/$uid/rename',
    });
    expect(
      lastLocation(reported) ?? '/missions?operation_id=G&mission_id=$uid',
      contains('mission_id=$uid'),
      reason: 'the route keeps the UID',
    );
  });

  testWidgets(
    'a route with a handle the Mission had before opens the Mission',
    (tester) async {
      final requests = <http.Request>[];
      final board = gateway(requests);
      addTearDown(board.dispose);
      final reported = await openAt(
        tester,
        board,
        '/missions?operation_id=G&mission_id=G-old',
      );
      expect(find.text('G › G-1'), findsOneWidget);
      expect(lastLocation(reported), contains('mission_id=$uid'));
      expect(
        requests.any(
          (request) =>
              request.url.path == '/v1/missions' &&
              request.url.queryParameters['mission'] == 'G-old',
        ),
        isTrue,
      );
    },
  );

  testWidgets(
    'a route with the current handle, in any case, opens the Mission',
    (tester) async {
      final board = gateway([]);
      addTearDown(board.dispose);
      await openAt(tester, board, '/missions?operation_id=G&mission_id=g-1');
      expect(find.text('G › G-1'), findsOneWidget);
    },
  );

  testWidgets('without mission.control the rename shows what it needs', (
    tester,
  ) async {
    final requests = <http.Request>[];
    final board = gateway(requests, actions: const ['mission.read']);
    addTearDown(board.dispose);
    await openAt(tester, board, '/missions?operation_id=G&mission_id=$uid');
    expect(
      find.byTooltip('Needs mission.control in Operation G'),
      findsOneWidget,
    );
    await tester.tap(find.text('Rename'), warnIfMissed: false);
    await tester.pumpAndSettle();
    expect(find.byType(FilledButton), findsNothing);
    expect(
      requests.where((request) => request.url.path.endsWith('/rename')),
      isEmpty,
    );
  });
}
