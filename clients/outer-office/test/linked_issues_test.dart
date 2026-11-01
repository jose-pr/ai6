import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/issues/linked_issues.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/oo_freshness.dart';

import 'work_views_test.dart' show openAt;

const _issues = {
  'issues': [
    {
      'issue': 'gh:o/r#1',
      'tracker': 'gh',
      'item': 'o/r#1',
      'url': 'https://github.com/o/r/issues/1',
      'relation': 'source',
      'state': 'active',
      'linked_by': 'm',
      'title': 'Flaky\u202e test',
      'issue_state': 'open',
      'labels': ['bug'],
      'phase': 'waiting',
      'open_question': {'asked_at': 1.0, 'expires_at': 2.0},
    },
    {
      'issue': 'gh:o/r#2',
      'tracker': 'gh',
      'item': 'o/r#2',
      'url': null,
      'relation': 'related',
      'state': 'proposed',
      'linked_by': 'agent:007',
      'title': null,
      'issue_state': null,
      'labels': <String>[],
      'phase': null,
      'open_question': null,
    },
  ],
  'entries': [
    {
      'id': 'ox-1',
      'kind': 'status',
      'issue': 'gh:o/r#1',
      'actor': 'm',
      'state': 'posted',
      'url': null,
      'reason': null,
      'dropped': <String>[],
      'at': 1.0,
    },
    {
      'id': 'ox-2',
      'kind': 'comment',
      'issue': 'gh:o/r#1',
      'actor': 'agent:007',
      'state': 'pending',
      'url': null,
      'reason': null,
      'dropped': <String>[],
      'at': 2.0,
    },
    {
      'id': 'ox-3',
      'kind': 'update',
      'issue': 'gh:o/r#1',
      'actor': 'agent:007',
      'state': 'held',
      'url': null,
      'reason': null,
      'dropped': <String>[],
      'at': 3.0,
    },
    {
      'id': 'ox-4',
      'kind': 'create',
      'issue': 'gh',
      'actor': 'agent:007',
      'state': 'refused',
      'url': null,
      'reason': 'this tracker cannot create issues',
      'dropped': ['assignees', 'sprints'],
      'at': 4.0,
    },
  ],
  'replies': {'delivered': 1, 'pending': 0, 'ignored': 2},
};

const _proposals = [
  {
    'id': 'ip-1',
    'operation': 'G',
    'mission_id': 'm1',
    'proposer': 'agent:007',
    'specs': [
      {
        'tracker': 'gh',
        'title': 'From Jira',
        'body': 'Mirrored from https://x.example/AI6-1',
        'labels': ['bug'],
        'mirror_of': 'jira:AI6-1',
      },
    ],
    'findings': ['duplicate'],
    'dropped': ['assignees', 'sprints'],
    'exposure': ['issue:jira'],
    'state': 'pending',
    'mark_ready': false,
    'created': <String>[],
    'failed': <String>[],
  },
];

/// A signed-in board holding [actions] on Operation G; requests go to [seen].
Future<MissionBoard> boardWith(
  List<String> actions,
  List<http.Request> seen,
) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      seen.add(request);
      final path = request.url.path;
      http.Response json(Object body, [int status = 200]) => http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json'},
      );
      return switch (path) {
        '/v1/auth/login' => json({'token': 'user-token'}, 201),
        '/v1/board' || '/v1/missions' => json(const <Object>[]),
        '/v1/stations' => json({'headquarters_station': null, 'stations': []}),
        '/v1/user/profile' => json({
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
        '/v1/user/operations/G/missions/m1/issues'
            when request.method == 'GET' =>
          json(_issues),
        '/v1/user/operations/G/issue-proposals' when request.method == 'GET' =>
          json(_proposals),
        _ when path.startsWith('/v1/user/operations/G/') => json({
          'state': 'queued',
        }),
        _ => json(const <Object>[]),
      };
    }),
  );
  await board.login('operator', 'secret');
  addTearDown(board.dispose);
  return board;
}

Future<void> show(WidgetTester tester, double width, Widget child) async {
  tester.view.physicalSize = Size(width, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(body: SingleChildScrollView(child: child)),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  for (final width in [390.0, 1280.0]) {
    testWidgets('the section shows links, the question and each post state at '
        '$width px', (tester) async {
      final seen = <http.Request>[];
      final board = await boardWith(['mission.read'], seen);
      await show(
        tester,
        width,
        LinkedIssuesSection(board: board, operationId: 'G', missionId: 'm1'),
      );
      expect(tester.takeException(), isNull, reason: 'no overflow at $width');
      expect(find.text('LINKED ISSUES'), findsOneWidget);
      expect(find.text('source'), findsOneWidget);
      expect(find.text('gh:o/r#1'), findsOneWidget);
      expect(find.text('gh:o/r#2'), findsOneWidget);
      expect(find.text('proposed by an Agent'), findsOneWidget);
      expect(
        find.text('Flaky<U+202E> test'),
        findsOneWidget,
        reason: 'what renders as nothing is spelled out',
      );
      expect(find.textContaining('Waiting for an answer'), findsOneWidget);
      expect(find.text('posted'), findsOneWidget);
      expect(find.text('queued'), findsOneWidget);
      expect(find.text('waiting for green ink'), findsOneWidget);
      expect(find.text('refused'), findsOneWidget);
      expect(find.text('this tracker cannot create issues'), findsOneWidget);
      expect(find.text('Not carried over: assignees, sprints'), findsOneWidget);
      expect(find.textContaining('1 replies'), findsOneWidget);
    });

    testWidgets(
      'an issue proposal card shows its text and buttons at $width px',
      (tester) async {
        final seen = <http.Request>[];
        final board = await boardWith(['mission.read', 'issue.create'], seen);
        await show(
          tester,
          width,
          IssueProposalsPanel(board: board, operationId: 'G'),
        );
        expect(tester.takeException(), isNull, reason: 'no overflow at $width');
        expect(find.text('ISSUE PROPOSALS'), findsOneWidget);
        expect(find.text('From Jira'), findsOneWidget);
        expect(
          find.text('Mirrored from https://x.example/AI6-1'),
          findsOneWidget,
        );
        expect(find.text('In gh · copy of jira:AI6-1'), findsOneWidget);
        expect(
          find.text('Not carried over: assignees, sprints'),
          findsOneWidget,
        );
        expect(find.text('Vetting found: duplicate'), findsOneWidget);
        expect(
          find.text('Its Session read outside text: issue:jira'),
          findsOneWidget,
        );
        expect(find.text('Approve and create'), findsOneWidget);
      },
    );
  }

  testWidgets(
    'without an action its button is disabled and says what it needs',
    (tester) async {
      final seen = <http.Request>[];
      final board = await boardWith(['mission.read'], seen);
      await show(
        tester,
        1280,
        LinkedIssuesSection(board: board, operationId: 'G', missionId: 'm1'),
      );
      expect(
        tester
            .widget<TextButton>(
              find.widgetWithText(TextButton, 'Comment').first,
            )
            .onPressed,
        isNull,
      );
      expect(
        find.byWidgetPredicate(
          (widget) =>
              widget is Tooltip &&
              widget.message == 'Comment needs issue.comment',
        ),
        findsOneWidget,
      );
    },
  );

  testWidgets('a person with the action comments, accepts and unlinks', (
    tester,
  ) async {
    final seen = <http.Request>[];
    final board = await boardWith([
      'mission.read',
      'issue.link',
      'issue.comment',
      'issue.update',
    ], seen);
    await show(
      tester,
      1280,
      LinkedIssuesSection(board: board, operationId: 'G', missionId: 'm1'),
    );
    await tester.tap(find.widgetWithText(TextButton, 'Comment').first);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Use the release branch.');
    await tester.tap(find.text('Send'));
    await tester.pumpAndSettle();
    final comment = seen.lastWhere((r) => r.url.path.endsWith('/comments'));
    expect(comment.method, 'POST');
    expect(
      comment.url.path,
      '/v1/user/operations/G/missions/m1/issues/gh%3Ao%2Fr%231/comments',
    );
    expect(jsonDecode(comment.body), {'body': 'Use the release branch.'});

    await tester.tap(find.widgetWithText(TextButton, 'Accept'));
    await tester.pumpAndSettle();
    expect(
      seen.any((r) => r.method == 'POST' && r.url.path.endsWith('/accept')),
      isTrue,
    );
    await tester.tap(find.widgetWithText(TextButton, 'Unlink').first);
    await tester.pumpAndSettle();
    expect(seen.any((r) => r.method == 'DELETE'), isTrue);
  });

  testWidgets('mark ready needs mission.create and is sent with the approval', (
    tester,
  ) async {
    final seen = <http.Request>[];
    final board = await boardWith([
      'mission.read',
      'issue.create',
      'mission.create',
    ], seen);
    await show(
      tester,
      1280,
      IssueProposalsPanel(board: board, operationId: 'G'),
    );
    await tester.tap(find.byType(Checkbox));
    await tester.pump();
    await tester.tap(find.text('Approve and create'));
    await tester.pumpAndSettle();
    final approved = seen.lastWhere((r) => r.url.path.endsWith('/approve'));
    expect(jsonDecode(approved.body), {'mark_ready': true});

    final without = <http.Request>[];
    final limited = await boardWith(['mission.read', 'issue.create'], without);
    await show(
      tester,
      1280,
      IssueProposalsPanel(
        key: const ValueKey('limited'),
        board: limited,
        operationId: 'G',
      ),
    );
    expect(
      tester.widget<CheckboxListTile>(find.byType(CheckboxListTile)).onChanged,
      isNull,
    );
  });

  /// A board with one Mission waiting on its issue and a Station reporting weakened settings.
  Future<MissionBoard> waitingBoard() async {
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      pollInterval: null,
      client: MockClient((request) async {
        http.Response json(Object body, [int status = 200]) => http.Response(
          jsonEncode(body),
          status,
          headers: {'content-type': 'application/json'},
        );
        return switch (request.url.path) {
          '/v1/auth/login' => json({'token': 'user-token'}, 201),
          '/v1/board' => json([
            {
              'operation_id': 'G',
              'mission_id': 'm1',
              'session_id': 'session-1',
              'state': 'blocked',
              'reason': 'issue',
              'sequence': 1,
              'updated_at': 100.0,
            },
          ]),
          '/v1/stations' => json({
            'headquarters_station': null,
            'stations': [
              {
                'station_id': 'hq',
                'healthy': true,
                'headquarters': true,
                'seen': 1.0,
                'roles': <String>[],
                'running': 0,
                'capacity': 1,
                'operations': ['G'],
                'issues_weakened': [
                  'Operation "G" issues is weakened: issues.create direct is '
                      'looser than the default propose',
                ],
              },
            ],
          }),
          '/v1/user/profile' => json({
            'person_id': 'operator-007',
            'name': 'operator',
            'display_name': null,
            'groups': [],
            'expires_at': 1_900_000_000,
            'operations': [
              {
                'operation_id': 'G',
                'actions': ['mission.read'],
              },
            ],
            'stations': [],
          }),
          '/v1/user/operations/G/missions/m1/issues' => json(_issues),
          _ => json(const <Object>[]),
        };
      }),
    );
    await board.login('operator', 'secret');
    addTearDown(board.dispose);
    return board;
  }

  for (final width in [390.0, 1400.0]) {
    testWidgets(
      'the chip of a Mission waiting on its issue says so at $width px',
      (tester) async {
        await openAt(
          tester,
          await waitingBoard(),
          '/missions?operation_id=G',
          size: Size(width, 1200),
        );
        expect(tester.takeException(), isNull);
        expect(find.textContaining('Waiting on the issue'), findsWidgets);
      },
    );
  }

  testWidgets('a Mission detail opens with its linked issues', (tester) async {
    await openAt(
      tester,
      await waitingBoard(),
      '/missions?operation_id=G&mission_id=m1',
      size: const Size(1400, 1600),
    );
    expect(find.text('LINKED ISSUES'), findsOneWidget);
    expect(find.text('gh:o/r#1'), findsOneWidget);
  });

  testWidgets('an Operation lists its issue settings looser than the default', (
    tester,
  ) async {
    await openAt(tester, await waitingBoard(), '/operations?operation_id=G');
    expect(find.text('ISSUE SETTINGS LOOSER THAN THE DEFAULT'), findsOneWidget);
    expect(
      find.text('issues.create direct is looser than the default propose'),
      findsOneWidget,
    );
  });

  test('a brief from a Source names the issue it came from', () {
    final brief = MissionBriefSummary(
      operationId: 'G',
      missionId: 'm1',
      prompt: 'x',
      agentId: null,
      roleId: null,
      handlerProvider: 'h',
      terminalProvider: 't',
      workingDirectory: '.',
      model: null,
      effort: null,
      environmentKeys: const [],
      origin: const BriefOrigin(
        origin: 'source',
        receivedBy: 'hq',
        principal: 'github',
        reference: 'o/r#12',
      ),
    );
    expect(brief.sourceIssue, 'github:o/r#12');
  });

  test('a blocked reason of issue reads in words', () {
    expect(reasonWords('issue'), 'Waiting on the issue');
    expect(reasonWords('green-ink'), 'green ink');
  });
}
