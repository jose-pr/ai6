import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart'
    show DropdownButton, NavigationBar, Text, TextField, TextInputAction;
import 'package:outer_office/main.dart';

Finder navLabel(String label) =>
    find.descendant(of: find.byType(NavigationBar), matching: find.text(label));

Map<String, Object?> item(
  String id,
  double at,
  String kind,
  String action, {
  String operation = 'G',
  String? mission,
}) => {
  'id': id,
  'at': at,
  'kind': kind,
  'action': action,
  'operation_id': operation,
  'station_id': 'station-alpha',
  'mission_id': mission,
  'session_id': null,
  'actor': null,
  'target': null,
  'detail': <String, String>{},
};

/// A gateway whose Operation G has four events (one only after [newer] is set) and whose
/// Operation L has one; every timeline request is kept in [asked].
class FakeTimeline {
  bool newer = false;
  final asked = <Uri>[];

  http.Response answer(Uri url) {
    asked.add(url);
    final query = url.queryParameters;
    final operation = url.pathSegments[3];
    final Map<String, Object?> page;
    if (operation == 'L') {
      page = {
        'items': [item('event:l1', 50, 'cable', 'sent', operation: 'L')],
        'next_cursor': null,
      };
    } else if (query['mission'] == 'mission-1') {
      page = {
        'items': [
          item('event:e4', 40, 'lifecycle', 'blocked', mission: 'mission-1'),
        ],
        'next_cursor': null,
      };
    } else if (query['cursor'] == 'after-e4') {
      page = {
        'items': [item('event:e3', 30, 'lifecycle', 'working')],
        'next_cursor': null,
      };
    } else {
      page = {
        'items': [
          if (newer) item('event:e6', 60, 'lifecycle', 'done'),
          item('event:e5', 50, 'control', 'resume.applied'),
          if (!newer)
            item('event:e4', 40, 'lifecycle', 'blocked', mission: 'mission-1'),
        ],
        'next_cursor': newer ? 'after-e5' : 'after-e4',
      };
    }
    return http.Response(jsonEncode({'operation_id': operation, ...page}), 200);
  }
}

Future<MissionBoard> signedIn(
  FakeTimeline timeline, {
  List<String> actions = const ['mission.read', 'audit.read'],
}) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      final path = request.url.path;
      if (path == '/v1/auth/login') {
        return http.Response(jsonEncode({'token': 'user-token'}), 201);
      }
      expect(request.headers['authorization'], 'Bearer user-token');
      if (path.endsWith('/timeline')) return timeline.answer(request.url);
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
  testWidgets('older pages load below and a refresh adds new activity above', (
    tester,
  ) async {
    final timeline = FakeTimeline();
    final board = await signedIn(timeline);
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.tap(navLabel('Timeline'));
    await tester.pumpAndSettle();

    expect(find.text('control · resume.applied'), findsOneWidget);
    expect(find.text('lifecycle · blocked'), findsOneWidget);
    expect(timeline.asked.single.path, '/v1/user/operations/G/timeline');

    await tester.tap(find.text('Load older activity'));
    await tester.pumpAndSettle();
    expect(timeline.asked.last.queryParameters['cursor'], 'after-e4');
    expect(find.text('lifecycle · working'), findsOneWidget);
    expect(find.text('Load older activity'), findsNothing);

    timeline.newer = true;
    await tester.tap(find.byTooltip('Refresh timeline'));
    await tester.pumpAndSettle();
    expect(timeline.asked.last.queryParameters.containsKey('cursor'), isFalse);
    // The new item is on top and nothing already loaded is lost or repeated.
    final titles = tester
        .widgetList<Text>(find.textContaining(' · '))
        .map((text) => text.data)
        .where((title) => title!.startsWith(RegExp('(lifecycle|control) ')))
        .toList();
    expect(titles, [
      'lifecycle · done',
      'control · resume.applied',
      'lifecycle · blocked',
      'lifecycle · working',
    ]);
  });

  testWidgets('the timeline filters by Mission and by Operation', (
    tester,
  ) async {
    final timeline = FakeTimeline();
    final board = await signedIn(timeline);
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.tap(navLabel('Timeline'));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), 'mission-1');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(timeline.asked.last.queryParameters['mission'], 'mission-1');
    expect(find.text('lifecycle · blocked'), findsOneWidget);
    expect(find.text('control · resume.applied'), findsNothing);
    expect(find.textContaining('Mission mission-1'), findsOneWidget);

    await tester.tap(find.byType(DropdownButton<String>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('L').last);
    await tester.pumpAndSettle();
    expect(timeline.asked.last.path, '/v1/user/operations/L/timeline');
    expect(find.text('cable · sent'), findsOneWidget);
  });

  testWidgets('the timeline needs audit.read', (tester) async {
    final timeline = FakeTimeline();
    final board = await signedIn(timeline, actions: const ['mission.read']);
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    expect(navLabel('Timeline'), findsNothing);
    expect(timeline.asked, isEmpty);
  });
}
