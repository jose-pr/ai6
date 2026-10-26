import 'dart:convert';
import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart'
    show DropdownButton, NavigationBar, Tab, TextField;
import 'package:moneypenny/main.dart';

Finder navLabel(String label) =>
    find.descendant(of: find.byType(NavigationBar), matching: find.text(label));

const _agents = [
  {
    'agent': '007',
    'posted': true,
    'operation': {
      'done': 12,
      'failed': 1,
      'blown': 0,
      'abandoned': 0,
      'stalled': 2,
      'lost': 1,
      'quarantined': 0,
      'budget_stops': 0,
      'green_ink_rejected': 0,
      'last': {},
    },
    'every_operation': {'quarantined': 1},
    'by_clearance': {},
    'last_mission_at': 1_900_000_000.0,
    'model_history': [
      {'model': 'opus', 'from': 1_800_000_000.0, 'to': 1_850_000_000.0},
      {'model': 'sonnet', 'from': 1_850_000_000.0, 'to': null},
    ],
    'retirement': {
      'retire_on': '2030-01-31',
      'on_retire': 'desk',
      'status': 'retiring',
    },
    'clearance': 'field',
    'configured_clearance': 'desk',
    'grant': {
      'agent': '007',
      'operation': 'G',
      'clearance': 'field',
      'signer': 'alice',
      'at': 1_850_000_000.0,
      'approval': 'green-ink-1',
    },
    'promotion': {
      'id': 'green-ink-2',
      'scope': {'kind': 'clearance', 'target': '007:00@G'},
    },
  },
  {
    'agent': '008',
    'posted': true,
    'operation': {'done': 0},
    'model_history': [],
    'clearance': 'desk',
    'configured_clearance': 'desk',
    'grant_unreadable': true,
  },
];

/// A board whose person holds `actions` on Operation G, answering the Agents routes and
/// recording every request in `requests`.
MissionBoard signedIn(List<http.Request> requests, List<String> actions) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://moneypenny.example'),
    pollInterval: null,
    client: MockClient((request) async {
      requests.add(request);
      final path = request.url.path;
      if (path == '/v1/user/operations/G/agents') {
        return http.Response(
          jsonEncode(_agents),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      if (path.endsWith('/clearance')) {
        return http.Response(
          jsonEncode({
            'agent': '007',
            'operation': 'G',
            'clearance': 'desk',
            'signer': 'operator-007',
            'at': 1.0,
          }),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      return http.Response('[]', 200);
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
    operations: [UserOperationAccess(operationId: 'G', actions: actions)],
  );
  return board;
}

Future<void> openAgents(WidgetTester tester, MissionBoard board) async {
  tester.view.physicalSize = const Size(800, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(MoneypennyApp(board: board, desktopWindows: false));
  await tester.tap(navLabel('Administration'));
  await tester.pumpAndSettle();
  await tester.tap(find.widgetWithText(Tab, 'Agents'));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'an Agent reader sees records, clearance, retirement and promotion',
    (tester) async {
      final requests = <http.Request>[];
      final board = signedIn(requests, ['agent.read']);
      await openAgents(tester, board);

      expect(
        requests.map((request) => request.url.path),
        contains('/v1/user/operations/G/agents'),
      );
      expect(
        find.text('12 done · 1 failed · 2 stalled · 1 lost'),
        findsOneWidget,
      );
      expect(
        find.text('Clearance grant unreadable: Stations work it at desk'),
        findsOneWidget,
      );
      expect(find.text('1 quarantined in every Operation'), findsOneWidget);
      expect(
        find.textContaining('Model sonnet since 2028-08-'),
        findsOneWidget,
      );
      expect(find.text('field (configured desk)'), findsOneWidget);
      expect(find.text('Desk duty from 2030-01-31'), findsOneWidget);
      expect(find.text('Raised to field by alice'), findsOneWidget);
      expect(find.text('Promotion to 00 awaits green ink'), findsOneWidget);
      expect(find.text('0 done'), findsOneWidget);
      expect(
        find.text('Lower clearance'),
        findsNothing,
        reason: 'agent.configure is needed',
      );
    },
  );

  testWidgets(
    'a configurer lowers a clearance and restores the configured one',
    (tester) async {
      final requests = <http.Request>[];
      final board = signedIn(requests, ['agent.read', 'agent.configure']);
      await openAgents(tester, board);

      expect(
        find.text('Lower clearance'),
        findsOneWidget,
        reason: '008 is at desk already',
      );
      await tester.tap(find.text('Lower clearance'));
      await tester.pumpAndSettle();
      expect(find.byType(DropdownButton<String>), findsWidgets);
      await tester.enterText(find.byType(TextField), 'under review');
      await tester.tap(find.text('Lower'));
      await tester.pumpAndSettle();
      final lowered = requests.lastWhere((request) => request.method == 'PUT');
      expect(lowered.url.path, '/v1/user/operations/G/agents/007/clearance');
      expect(jsonDecode(lowered.body), {
        'clearance': 'desk',
        'reason': 'under review',
      });

      await tester.tap(find.text('Restore configured clearance'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Restore'));
      await tester.pumpAndSettle();
      final restored = requests.lastWhere(
        (request) => request.method == 'DELETE',
      );
      expect(restored.url.path, '/v1/user/operations/G/agents/007/clearance');
    },
  );
}
