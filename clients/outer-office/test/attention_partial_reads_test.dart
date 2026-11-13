import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/oo_freshness.dart';

Map<String, Object?> _held(String operation) => {
  'key': 'source_github--org_repo_7',
  'operation': operation,
  'brief': {
    'title': 'Resolve item in $operation',
    'body': 'Do it.',
    'id': 'org/repo#7',
    'source': 'source:github',
    'operation': operation,
  },
  'vetting': {'verdict': 'hold', 'findings': []},
  'held_at': 1_798_000_000.0,
};

Map<String, Object?> _proposal(String operation) => {
  'id': 'proposal-$operation',
  'operation_id': operation,
  'parent_mission_id': 'mission-1',
  'proposer_agent_id': '007',
  'proposer_run_id': 'provider-id-1',
  'brief': {'title': 'Proposal in $operation', 'body': 'Do it.'},
  'created_at': 1_798_000_000.0,
};

/// A board signed in with every action on [answers]' Operations; each Operation answers the
/// per-Operation reads with its status (200 gives one item).
Future<MissionBoard> _board(Map<String, int> answers) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      http.Response json(Object body, [int status = 200]) => http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json'},
      );
      final path = request.url.path;
      if (path == '/v1/auth/login') return json({'token': 'user-token'}, 201);
      if (path == '/v1/user/profile') {
        return json({
          'person_id': 'operator-007',
          'name': 'operator',
          'display_name': null,
          'groups': [],
          'expires_at': 1_900_000_000,
          'operations': [
            for (final operation in answers.keys)
              {
                'operation_id': operation,
                'actions': ['mission.create', 'approval.decide'],
              },
          ],
          'stations': [],
        });
      }
      final match = RegExp(
        r'^/v1/user/operations/([^/]+)/(held-briefs|mission-proposals|green-ink)$',
      ).firstMatch(path);
      if (match != null) {
        final operation = match.group(1)!;
        final status = answers[operation]!;
        if (status != 200) return json({'error': 'no'}, status);
        return json(switch (match.group(2)) {
          'held-briefs' => [_held(operation)],
          'mission-proposals' => [_proposal(operation)],
          _ => const <Object>[],
        });
      }
      if (path == '/v1/stations') {
        return json({'headquarters_station': null, 'stations': []});
      }
      return json(const <Object>[]);
    }),
  );
  await board.login('operator', 'secret');
  addTearDown(board.dispose);
  return board;
}

Future<void> _note(WidgetTester tester, MissionBoard board) =>
    tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: FreshnessNote(
            board: board,
            sources: const [
              BoardSource.heldBriefs,
              BoardSource.greenInk,
              BoardSource.proposals,
            ],
          ),
        ),
      ),
    );

void main() {
  testWidgets(
    'an Operation that cannot be read is named beside the items read',
    (tester) async {
      final board = await _board({'op-a': 200, 'op-b': 404, 'op-c': 500});
      expect(board.heldBriefs.map((held) => held.operationId), ['op-a']);
      expect(board.missionProposals.map((p) => p.operationId), ['op-a']);
      expect(board.freshness(BoardSource.heldBriefs), OoFreshness.live);
      expect(board.unreadOperations(BoardSource.heldBriefs), {
        'op-b': 'not found',
        'op-c': 'refused',
      });
      await _note(tester, board);
      expect(
        find.textContaining(
          'Held briefs: could not be read in op-b (not found), op-c (refused)',
        ),
        findsOneWidget,
      );
      expect(
        find.textContaining('Mission proposals: could not be read in op-b'),
        findsOneWidget,
      );
      expect(
        find.textContaining('Green ink requests: could not be read in op-b'),
        findsOneWidget,
      );
    },
  );

  testWidgets('an Operation that refuses with 403 is left out quietly', (
    tester,
  ) async {
    final board = await _board({'op-a': 200, 'op-b': 403});
    expect(board.heldBriefs.map((held) => held.operationId), ['op-a']);
    expect(board.unreadOperations(BoardSource.heldBriefs), isEmpty);
    expect(board.freshness(BoardSource.heldBriefs), OoFreshness.live);
    await _note(tester, board);
    expect(find.textContaining('could not be read'), findsNothing);
  });

  testWidgets('when every Operation fails the section could not be read', (
    tester,
  ) async {
    final board = await _board({'op-a': 404, 'op-b': 404});
    expect(board.heldBriefs, isEmpty);
    expect(board.freshness(BoardSource.heldBriefs), OoFreshness.unavailable);
    await _note(tester, board);
    expect(
      find.textContaining('Held briefs: could not be read'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Mission proposals: could not be read'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Green ink requests: could not be read'),
      findsOneWidget,
    );
  });

  testWidgets('403 everywhere is no access, not a failure', (tester) async {
    final board = await _board({'op-a': 403, 'op-b': 403});
    expect(board.freshness(BoardSource.heldBriefs), OoFreshness.noAccess);
    await _note(tester, board);
    expect(find.textContaining('could not be read'), findsNothing);
  });
}
