import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/reports/reports_model.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/operation_names.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'work_views_test.dart' show openAt;

Map<String, Object?> report(
  String id,
  String title, {
  String scope = 'operation:G',
  String state = 'open',
  String priority = 'normal',
  Map<String, Object?>? filer,
  List<Map<String, String>> links = const [],
  Map<String, Object?>? assessment,
  Map<String, Object?>? resolution,
  String kind = 'finding',
}) => {
  'id': id,
  'scope': scope,
  'sight': 'content',
  'kind': kind,
  'severity': 'warning',
  'state': state,
  'priority': priority,
  'untrusted': filer?['type'] != 'person',
  'title': title,
  'text': 'Body of $title',
  'filer':
      filer ?? {'type': 'agent', 'agent': '007', 'run': 'r1', 'mission': 'm9'},
  'source': filer?['type'] == 'source' ? 'github:o/r#1' : 'agent:007',
  'reliability': 'F',
  'links': links,
  'created': 1_700_000_000.0,
  'updated': 1_700_000_100.0,
  'assessment': ?assessment,
  'resolution': ?resolution,
};

/// The Reports the gateway holds, by scope, and what was asked of it.
class Gateway {
  Gateway(
    this.byScope, {
    this.actions = const ['mission.read', 'mission.control'],
  });

  final Map<String, List<Map<String, Object?>>> byScope;
  final List<String> actions;
  final seen = <http.Request>[];

  /// What a PUT assessment answers: a status and a body.
  (int, Object) assessed = (200, const {});

  Future<MissionBoard> board() async {
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      pollInterval: null,
      client: MockClient((request) async {
        seen.add(request);
        http.Response json(Object body, [int status = 200]) => http.Response(
          jsonEncode(body),
          status,
          headers: {'content-type': 'application/json'},
        );
        final path = request.url.path;
        if (path == '/v1/reports' && request.method == 'GET') {
          final scope = request.url.queryParameters['scope'];
          final mission = request.url.queryParameters['mission'];
          final all = [for (final list in byScope.values) ...list];
          final shown = scope != null
              ? byScope[scope] ?? const []
              : [
                  for (final one in all)
                    if ((one['links'] as List).any(
                      (link) => (link as Map)['id'] == mission,
                    ))
                      one,
                ];
          return json({
            'reports': shown,
            'priorities': ['critical', 'high', 'normal', 'low'],
            'classifications': ['unclassified', 'restricted'],
          });
        }
        if (path.startsWith('/v1/reports/sources/')) {
          return json({
            'source': Uri.decodeComponent(
              path.substring('/v1/reports/sources/'.length),
            ),
            'reliability': 'B',
            'rated': true,
            'counted': 6,
            'confirmed': 5,
            'rule': {
              'min_reports': 5,
              'counts': ['mission_done'],
              'confirms': ['mission_done'],
              'bands': [90, 75, 50, 25],
            },
            'reports': [
              report(
                'r-old',
                'An older finding',
                state: 'closed',
                resolution: {'by': 'm', 'at': 1.0, 'outcome': 'mission_done'},
              ),
            ],
          });
        }
        if (path.startsWith('/v1/reports/')) {
          final id = path.split('/')[3];
          if (request.method == 'PUT') {
            return json(assessed.$2, assessed.$1);
          }
          if (request.method == 'POST') {
            return json(report(id, 'Reopened', state: 'open'));
          }
          for (final list in byScope.values) {
            for (final one in list) {
              if (one['id'] == id) return json(one);
            }
          }
          return json({'error': 'No such report.'}, 404);
        }
        return switch (path) {
          '/v1/auth/login' => json({'token': 'user-token'}, 201),
          '/v1/board' => json([
            {
              'operation_id': 'G',
              'mission_id': 'm1',
              'run_id': 'run-1',
              'state': 'working',
              'sequence': 1,
              'updated_at': 100.0,
            },
          ]),
          '/v1/stations' => json({
            'headquarters_station': null,
            'stations': <Object>[],
          }),
          '/v1/user/profile' => json({
            'person_id': 'operator-007',
            'name': 'operator',
            'display_name': null,
            'groups': [],
            'expires_at': 1_900_000_000,
            'operations': [
              {'operation_id': 'G', 'name': 'Guatiao', 'actions': actions},
              {'operation_id': 'L', 'name': 'LineupMux', 'actions': actions},
            ],
            'stations': [],
          }),
          _ => json(const <Object>[]),
        };
      }),
    );
    await board.login('operator', 'secret');
    addTearDown(board.dispose);
    return board;
  }

  /// The PUT assessments sent, decoded.
  List<Map<String, dynamic>> get puts => [
    for (final request in seen)
      if (request.method == 'PUT')
        jsonDecode(request.body) as Map<String, dynamic>,
  ];
}

Gateway twoOperations() => Gateway({
  'operation:G': [
    report('r1', 'The release notes name 0.3', priority: 'high'),
    report(
      'r2',
      'Rotate the deploy key',
      state: 'assessed',
      filer: {'type': 'person', 'person': 'ana'},
      assessment: {
        'verdict': 'mission',
        'by': 'person:ana',
        'at': 1_700_000_050.0,
        'comment': 'Worth a Mission',
        'judgement': {'likelihood': 'likely', 'confidence': 'moderate'},
      },
    ),
  ],
  'operation:L': [
    report(
      'r3',
      'Issue gh#12 changed',
      scope: 'operation:L',
      priority: 'low',
      kind: 'source-item',
      filer: {'type': 'source', 'source': 'github', 'item': 'o/r#12'},
    ),
  ],
});

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    OperationNames.use(const {'G': 'Guatiao', 'L': 'LineupMux'});
  });

  testWidgets('the page lists the Reports of every Operation, by Operation and '
      'state', (tester) async {
    final gateway = twoOperations();
    await openAt(tester, await gateway.board(), '/reports');
    expect(find.text('The release notes name 0.3'), findsOneWidget);
    expect(find.text('Rotate the deploy key'), findsOneWidget);
    expect(find.text('Issue gh#12 changed'), findsOneWidget);
    expect(find.text('Guatiao'), findsWidgets);
    expect(find.text('LineupMux'), findsWidgets);
    expect(find.text('open'), findsNWidgets(2));
    expect(find.text('assessed'), findsOneWidget);
    expect(find.text('Reports'), findsWidgets, reason: 'the rail has the page');
    final scopes = [
      for (final request in gateway.seen)
        if (request.url.path == '/v1/reports')
          request.url.queryParameters['scope'],
    ];
    expect(scopes, unorderedEquals(['operation:G', 'operation:L']));
  });

  testWidgets('the state and Operation filters narrow the list', (
    tester,
  ) async {
    await openAt(tester, await twoOperations().board(), '/reports?state=open');
    expect(find.text('Rotate the deploy key'), findsNothing);
    expect(find.text('The release notes name 0.3'), findsOneWidget);
    expect(find.text('Issue gh#12 changed'), findsOneWidget);
  });

  testWidgets('the Operation filter keeps only that Operation', (tester) async {
    await openAt(
      tester,
      await twoOperations().board(),
      '/reports?operation=L&state=open',
    );
    expect(find.text('The release notes name 0.3'), findsNothing);
    expect(find.text('Issue gh#12 changed'), findsOneWidget);
  });

  testWidgets('a Report opens beside the list with its assessment and filer', (
    tester,
  ) async {
    await openAt(
      tester,
      await twoOperations().board(),
      '/reports?report_id=r2',
    );
    expect(find.text('Rotate the deploy key'), findsWidgets);
    expect(find.text('ASSESSMENT'), findsOneWidget);
    expect(find.text('Mission'), findsOneWidget);
    expect(find.text('Worth a Mission'), findsOneWidget);
    expect(
      find.text('Judged likely (55-75%), moderate confidence'),
      findsOneWidget,
    );
    expect(find.text('Reopen this Report'), findsOneWidget);
    expect(find.text('Assess this Report'), findsNothing);
  });

  testWidgets('a person assesses an open Report and the Report shows it', (
    tester,
  ) async {
    final gateway = twoOperations();
    gateway.assessed = (
      200,
      report(
        'r1',
        'The release notes name 0.3',
        state: 'assessed',
        priority: 'high',
        assessment: {
          'verdict': 'mission',
          'by': 'person:operator-007',
          'at': 1_700_000_200.0,
          'comment': 'The notes are wrong',
          'judgement': {'likelihood': 'highly_likely', 'confidence': 'high'},
        },
      ),
    );
    await openAt(tester, await gateway.board(), '/reports?report_id=r1');
    expect(find.text('Assess this Report'), findsOneWidget);

    await tester.tap(find.text('Assess this Report'));
    await tester.pumpAndSettle();
    // A Report that is not a Source item must name its Mission.
    await tester.enterText(
      find.byKey(const ValueKey('reason')),
      'The notes are wrong',
    );
    await tester.tap(find.text('Save the assessment'));
    await tester.pumpAndSettle();
    expect(find.text('Name the Mission this Report is about.'), findsOneWidget);
    expect(
      gateway.puts,
      isEmpty,
      reason: 'nothing sent that cannot be accepted',
    );

    await tester.enterText(find.byKey(const ValueKey('mission')), 'm1');
    await tester.tap(find.byKey(const ValueKey('pick-Likelihood')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('highly likely (80-90%)').last);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('pick-Confidence')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('high').last);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('pick-Priority')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('critical').last);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Save the assessment'));
    await tester.pumpAndSettle();

    expect(gateway.puts, [
      {
        'assessment': 'mission',
        'reason': 'The notes are wrong',
        'mission': 'm1',
        'priority': 'critical',
        'likelihood': 'highly_likely',
        'confidence': 'high',
      },
    ]);
    expect(find.text('ASSESSMENT'), findsOneWidget);
    expect(find.text('The notes are wrong'), findsOneWidget);
    expect(find.text('Reopen this Report'), findsOneWidget);
  });

  testWidgets('a Source item is queued with no Mission named', (tester) async {
    final gateway = twoOperations();
    gateway.assessed = (
      200,
      report(
        'r3',
        'Issue gh#12 changed',
        scope: 'operation:L',
        state: 'assessed',
      ),
    );
    await openAt(tester, await gateway.board(), '/reports?report_id=r3');
    await tester.tap(find.text('Assess this Report'));
    await tester.pumpAndSettle();
    expect(
      find.textContaining('Leave empty to queue this Source item'),
      findsOneWidget,
    );
    await tester.enterText(find.byKey(const ValueKey('reason')), 'Queue it');
    await tester.tap(find.text('Save the assessment'));
    await tester.pumpAndSettle();
    expect(gateway.puts, [
      {'assessment': 'mission', 'reason': 'Queue it'},
    ]);
  });

  testWidgets('awaiting takes a question and a deadline, and the refusal text '
      'shows', (tester) async {
    final gateway = twoOperations();
    gateway.assessed = (
      403,
      {'error': 'Assessing a report needs mission.control on its Operation.'},
    );
    await openAt(tester, await gateway.board(), '/reports?report_id=r1');
    await tester.tap(find.text('Assess this Report'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('assessment')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Awaiting an answer').last);
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('question')),
      'Is it deployed?',
    );
    await tester.enterText(find.byKey(const ValueKey('wait')), '2');
    await tester.enterText(find.byKey(const ValueKey('reason')), 'Ask first');
    await tester.tap(find.text('Save the assessment'));
    await tester.pumpAndSettle();
    expect(gateway.puts, [
      {
        'assessment': 'awaiting',
        'reason': 'Ask first',
        'question': 'Is it deployed?',
        'wait_seconds': 7200,
      },
    ]);
    expect(
      find.text('Assessing a report needs mission.control on its Operation.'),
      findsOneWidget,
      reason: 'the gateway\'s own words',
    );
    expect(
      find.text('Save the assessment'),
      findsOneWidget,
      reason: 'still open',
    );
  });

  testWidgets('without mission.control the action is disabled and says what it '
      'needs', (tester) async {
    final gateway = Gateway(
      {
        'operation:G': [report('r1', 'The release notes name 0.3')],
      },
      actions: const ['mission.read'],
    );
    await openAt(tester, await gateway.board(), '/reports?report_id=r1');
    final button = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, 'Assess this Report'),
    );
    expect(button.onPressed, isNull);
    expect(find.textContaining('needs mission.control'), findsOneWidget);
  });

  testWidgets('a source\'s standing opens from the Report', (tester) async {
    await openAt(
      tester,
      await twoOperations().board(),
      '/reports?report_id=r3',
    );
    await tester.tap(find.text('Source standing'));
    await tester.pumpAndSettle();
    expect(find.text('Reliability B'), findsOneWidget);
    expect(find.text('5 of 6 counted Reports confirmed.'), findsOneWidget);
    expect(find.text('An older finding'), findsOneWidget);
    expect(find.text('Mission done'), findsOneWidget);
  });

  testWidgets('reopening asks why and sends it', (tester) async {
    final gateway = twoOperations();
    await openAt(tester, await gateway.board(), '/reports?report_id=r2');
    await tester.tap(find.text('Reopen this Report'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Reopen'));
    await tester.pumpAndSettle();
    expect(find.text('Say why it comes back.'), findsOneWidget);
    await tester.enterText(
      find.byKey(const ValueKey('reopen-reason')),
      'The answer came',
    );
    await tester.tap(find.text('Reopen'));
    await tester.pumpAndSettle();
    final posts = [
      for (final request in gateway.seen)
        if (request.method == 'POST' && request.url.path.contains('/reports/'))
          request,
    ];
    expect(posts.single.url.path, '/v1/reports/r2/reopen');
    expect(jsonDecode(posts.single.body), {'reason': 'The answer came'});
  });

  testWidgets('a Mission page shows the Reports it resolves and those filed '
      'during it', (tester) async {
    final gateway = Gateway({
      'operation:G': [
        report(
          'r-spawn',
          'The notes are stale',
          state: 'assessed',
          filer: {'type': 'person', 'person': 'ana'},
          links: [
            {'kind': 'mission', 'id': 'm1'},
          ],
        ),
        report(
          'r-during',
          'A flaky test turned up',
          filer: {
            'type': 'agent',
            'agent': '007',
            'run': 'r1',
            'mission': 'm1',
          },
          links: [
            {'kind': 'mission', 'id': 'm1'},
          ],
        ),
        report('r-other', 'Unrelated'),
      ],
    });
    await openAt(
      tester,
      await gateway.board(),
      '/missions?operation_id=G&mission_id=m1',
      size: const Size(1400, 1800),
    );
    expect(find.text('REPORTS'), findsOneWidget);
    expect(find.text('It was begun for, or resolves'), findsOneWidget);
    expect(find.textContaining('The notes are stale'), findsOneWidget);
    expect(find.text('Filed while it ran'), findsOneWidget);
    expect(find.textContaining('A flaky test turned up'), findsOneWidget);
    expect(find.textContaining('Unrelated'), findsNothing);
    final asked = [
      for (final request in gateway.seen)
        if (request.url.path == '/v1/reports') request.url.queryParameters,
    ];
    expect(asked.any((query) => query['mission'] == 'm1'), isTrue);

    await tester.tap(find.textContaining('The notes are stale'));
    await tester.pumpAndSettle();
    expect(find.text('Body of The notes are stale'), findsOneWidget);
  });

  test('a Reports route keeps its filters in the link', () {
    final route = OoRoute.parseUri(
      'app:/reports?operation=G&state=open,assessed&report_id=r1',
    );
    expect(route.listOf('state'), ['open', 'assessed']);
    expect(
      route.toUri(),
      'app:/reports?report_id=r1&operation=G&state=open,assessed',
    );
  });

  test('the likelihood reads in the yardstick\'s words with its band', () {
    const judgement = ReportJudgement(
      likelihood: 'realistic_possibility',
      confidence: 'low',
    );
    expect(judgement.words, 'realistic possibility (40-50%), low confidence');
  });
}
