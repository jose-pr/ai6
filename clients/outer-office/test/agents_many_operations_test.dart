import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart' show Tab;
import 'package:outer_office/main.dart';

import 'shell_navigation.dart';

Map<String, Object?> _agent(String name) => {
  'agent': name,
  'posted': true,
  'operation': {'done': 1},
  'routes': [
    {'handler': 'claude', 'model': 'sonnet'},
  ],
  'model_history': [],
};

const _scorecard = {
  'window_days': 30,
  'small_sample': 5,
  'unattributed': 0,
  'agents': [],
  'models': [],
};

/// A board whose person reads Agents and scorecards in every Operation of [answers]; each
/// Operation answers the Agents read with its status, and 200 gives one Agent named for it.
MissionBoard _board(Map<String, int> answers) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      http.Response json(Object body, [int status = 200]) => http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json'},
      );
      final match = RegExp(r'^/v1/user/operations/([^/]+)/agents(/scorecard)?$')
          .firstMatch(request.url.path);
      if (match == null) return json(const <Object>[]);
      final operation = match.group(1)!;
      final status = answers[operation]!;
      if (status != 200) return json({'error': 'no'}, status);
      return match.group(2) != null
          ? json(_scorecard)
          : json([_agent('agent-$operation')]);
    }),
  );
  board.accessToken = 'operator-token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'operator-007',
    name: 'operator-007',
    displayName: null,
    groups: const [],
    expiresAt: 1e12,
    operations: [
      for (final operation in answers.keys)
        UserOperationAccess(operationId: operation, actions: ['agent.read']),
    ],
    roster: const ['roster.read'],
  );
  return board;
}

Future<void> _open(WidgetTester tester, MissionBoard board, Size size) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await openPage(tester, 'Agents');
  await tester.tap(find.widgetWithText(Tab, 'Agents'));
  await tester.pumpAndSettle();
}

void main() {
  final many = {for (var i = 1; i <= 8; i++) 'op-$i': 200};

  for (final size in const [Size(1400, 900), Size(420, 800)]) {
    testWidgets('with many Operations the table keeps the page at $size', (
      tester,
    ) async {
      await _open(tester, _board(many), size);
      expect(find.text('Scorecards'), findsOneWidget);
      if (size.width > 1000) {
        for (final operation in many.keys) {
          expect(find.text('agent-$operation'), findsOneWidget);
        }
      }
      final screen = Offset.zero & size;
      expect(
        screen.contains(tester.getCenter(find.text('agent-op-1'))),
        isTrue,
      );
    });
  }

  testWidgets('opened, the scorecards stay within a bounded height', (
    tester,
  ) async {
    await _open(tester, _board(many), const Size(1400, 900));
    await tester.tap(find.text('Scorecards'));
    await tester.pumpAndSettle();
    expect(find.text('Scorecard · op-1'), findsOneWidget);
    expect(find.text('agent-op-1'), findsOneWidget);
    expect(
      tester.getBottomLeft(find.text('Scorecard · op-1')).dy,
      lessThan(900),
    );
  });

  testWidgets('an Operation that cannot be read does not hide the others', (
    tester,
  ) async {
    await _open(
      tester,
      _board({'op-a': 200, 'op-b': 404, 'op-c': 403, 'op-d': 200}),
      const Size(1400, 900),
    );
    expect(find.text('agent-op-a'), findsOneWidget);
    expect(find.text('agent-op-d'), findsOneWidget);
    expect(find.textContaining('Operation op-b:'), findsOneWidget);
    expect(find.textContaining('HTTP 404'), findsOneWidget);
    expect(find.textContaining('op-c'), findsNothing);
  });
}
