import 'dart:convert';

import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/operation_names.dart';

import 'work_views_test.dart' show demoBoard, openAt;

const _sizes = [('desktop', Size(1800, 1200)), ('phone', Size(390, 844))];

MissionBoard _agentsBoard() {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      if (request.url.path.endsWith('/agents')) {
        return http.Response(
          jsonEncode([
            {
              'agent': '007',
              'posted': true,
              'operation': {'done': 3},
              'every_operation': <String, Object?>{},
              'by_clearance': <String, Object?>{},
              'clearance': request.url.path.contains('/G/') ? 'field' : 'desk',
              'routes': [
                {
                  'route': {'handler': 'claude', 'model': 'sonnet'},
                },
              ],
            },
          ]),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      return http.Response('[]', 200);
    }),
  );
  board.accessToken = 'token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'p-1',
    name: 'p-1',
    displayName: null,
    groups: const [],
    expiresAt: 1e12,
    operations: const [
      UserOperationAccess(
        operationId: 'G',
        name: 'Guatiao',
        actions: ['agent.read'],
      ),
      UserOperationAccess(
        operationId: 'L',
        name: 'LineupMux',
        actions: ['agent.read'],
      ),
    ],
    roster: const [],
  );
  addTearDown(board.dispose);
  return board;
}

/// The preview board with its Operations named, as a signed-in session names them.
MissionBoard _demoNamed() {
  final board = demoBoard();
  final session = board.userSession!;
  board.userSession = UserSessionSummary(
    personId: session.personId,
    name: session.name,
    displayName: session.displayName,
    groups: session.groups,
    expiresAt: session.expiresAt,
    operations: [
      for (final operation in session.operations)
        UserOperationAccess(
          operationId: operation.operationId,
          name: operation.operationId == 'G' ? 'Guatiao' : null,
          actions: operation.actions,
        ),
    ],
    roster: session.roster,
    globalActions: session.globalActions,
  );
  return board;
}

void main() {
  setUp(() => OperationNames.use(const {'G': 'Guatiao', 'L': 'LineupMux'}));
  tearDown(() => OperationNames.use(const {}));

  for (final (name, size) in _sizes) {
    testWidgets('Agents names each Operation, not its id, at $name width', (
      tester,
    ) async {
      await openAt(tester, _agentsBoard(), '/agents', size: size);
      expect(find.text('Guatiao'), findsWidgets);
      expect(find.text('LineupMux'), findsWidgets);
      expect(find.text('G'), findsNothing);
      expect(find.text('L'), findsNothing);
    });

    testWidgets('Audit names the Operation of a record at $name width', (
      tester,
    ) async {
      await openAt(tester, _demoNamed(), '/audit', size: size);
      expect(find.textContaining('Operation Guatiao'), findsWidgets);
      expect(find.text('Operation G'), findsNothing);
    });

    testWidgets('an Audit scope chip names its Operation at $name width', (
      tester,
    ) async {
      await openAt(tester, _demoNamed(), '/audit?operation_id=G', size: size);
      expect(find.text('Operation: Guatiao'), findsOneWidget);
    });
  }
}
