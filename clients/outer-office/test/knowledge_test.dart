import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:outer_office/reports/report_detail.dart';
import 'package:outer_office/reports/reports_model.dart';
import 'package:outer_office/shell/operation_names.dart';
import 'package:outer_office/theme/oo_theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'reports_test.dart' show Gateway, report;
import 'work_views_test.dart' show openAt;

/// What AI6 knows, as a Report of a known kind.
Map<String, Object?> known(
  String id,
  String title, {
  String kind = 'tradecraft',
  String scope = 'global',
  String state = 'standing',
  String summary = '',
  List<String> topics = const [],
  Map<String, Object?>? proposes,
  Map<String, Object?>? filer,
}) => {
  ...report(
    id,
    title,
    scope: scope,
    state: state,
    kind: kind,
    filer: filer ?? {'type': 'person', 'person': 'operator-007'},
  ),
  'summary': summary,
  'topics': topics,
  'revision': 1,
  'updated_by': 'person:operator-007',
  'exposed': false,
  'proposes': proposes,
};

/// The Gateway with what is known beside one finding.
Gateway knowing() => Gateway(
  {
    'operation:G': [report('r1', 'The release notes name 0.3')],
    'global': [
      known(
        'k1',
        'Writing a brief',
        summary: 'What a brief says',
        topics: ['brief', 'writing'],
      ),
      known('k2', 'Prefers tables', kind: 'memory', topics: ['style']),
    ],
    'person:operator-007': [
      known(
        'k3',
        'On call with ana',
        kind: 'memory',
        scope: 'person:operator-007',
      ),
    ],
  },
  searchable: [
    known(
      'built-in.kb.how_to_search.md',
      'How to search',
      summary: 'Searching what AI6 knows',
      topics: ['search'],
      filer: {'type': 'built_in'},
    ),
  ],
);

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    OperationNames.use(const {'G': 'Guatiao', 'L': 'LineupMux'});
  });

  testWidgets('the page lists what is known beside the Reports, by kind and '
      'topic', (tester) async {
    final gateway = knowing();
    await openAt(
      tester,
      await gateway.board(),
      '/reports',
      size: const Size(1400, 2400),
    );
    for (final title in [
      'The release notes name 0.3',
      'Writing a brief',
      'Prefers tables',
      'On call with ana',
      'How to search',
    ]) {
      expect(find.text(title), findsOneWidget, reason: title);
    }
    expect(find.text('Tradecraft'), findsWidgets);
    expect(find.text('Memory'), findsWidgets);
  });

  testWidgets('the kind filter keeps only that kind', (tester) async {
    await openAt(
      tester,
      await knowing().board(),
      '/reports?report_kind=memory',
      size: const Size(1400, 2400),
    );
    expect(find.text('Prefers tables'), findsOneWidget);
    expect(find.text('On call with ana'), findsOneWidget);
    expect(find.text('Writing a brief'), findsNothing);
    expect(find.text('The release notes name 0.3'), findsNothing);
  });

  testWidgets('the topic filter keeps only that topic', (tester) async {
    await openAt(
      tester,
      await knowing().board(),
      '/reports?topic=brief',
      size: const Size(1400, 2400),
    );
    expect(find.text('Writing a brief'), findsOneWidget);
    expect(find.text('Prefers tables'), findsNothing);
  });

  testWidgets('only the signed-in person\'s own memories are asked for', (
    tester,
  ) async {
    final gateway = knowing();
    await openAt(
      tester,
      await gateway.board(),
      '/reports',
      size: const Size(1400, 2400),
    );
    final persons = {
      for (final request in gateway.seen)
        if (request.url.queryParameters['scope']?.startsWith('person:') ??
            false)
          request.url.queryParameters['scope'],
    };
    expect(persons, {'person:operator-007'});
  });

  testWidgets('a built-in guide opens read only and is overridden for this '
      'deployment', (tester) async {
    final gateway = knowing();
    await openAt(
      tester,
      await gateway.board(),
      '/reports?report_id=built-in.kb.how_to_search.md',
    );
    expect(find.text('Override for this deployment'), findsOneWidget);
    expect(find.byKey(const ValueKey('report-built-in')), findsOneWidget);
    expect(find.text('Edit'), findsNothing);
    expect(find.text('Forget'), findsNothing);

    await tester.tap(find.text('Override for this deployment'));
    await tester.pumpAndSettle();
    final title = tester.widget<TextField>(
      find.byKey(const ValueKey('knowledge-title')),
    );
    expect(title.controller!.text, 'How to search');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final posts = [
      for (final request in gateway.seen)
        if (request.method == 'POST' && request.url.path == '/v1/reports')
          request.body,
    ];
    expect(posts, hasLength(1));
    expect(posts.single, contains('"scope":"global"'));
    expect(posts.single, contains('"kind":"tradecraft"'));
    expect(posts.single, contains('"title":"How to search"'));
  });

  testWidgets('a proposal is accepted from the page', (tester) async {
    final gateway = Gateway({
      'global': [
        known(
          'p1',
          'Keep briefs short',
          kind: 'memory',
          state: 'open',
          filer: {'type': 'agent', 'agent': '007', 'run': 'r', 'mission': 'm'},
        ),
      ],
    });
    gateway.assessed = (200, known('p1', 'Keep briefs short', kind: 'memory'));
    await openAt(tester, await gateway.board(), '/reports?report_id=p1');
    expect(find.text('Accept'), findsOneWidget);
    expect(find.text('Decline'), findsOneWidget);
    expect(find.text('Edit'), findsNothing);
    await tester.tap(find.text('Accept'));
    await tester.pumpAndSettle();
    expect(gateway.puts, [
      {'assessment': 'accepted', 'reason': 'Accepted in Outer Office.'},
    ]);
  });

  testWidgets('standing knowledge is edited from the revision opened and '
      'forgotten once confirmed', (tester) async {
    final gateway = knowing();
    await openAt(tester, await gateway.board(), '/reports?report_id=k1');
    await tester.tap(find.byKey(const ValueKey('report-edit')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('knowledge-title')),
      'Writing a good brief',
    );
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final put = gateway.seen.lastWhere(
      (request) =>
          request.method == 'PUT' && request.url.path == '/v1/reports/k1',
    );
    expect(put.body, contains('"revision":1'));
    expect(put.body, contains('"title":"Writing a good brief"'));

    await tester.tap(find.byKey(const ValueKey('report-forget')));
    await tester.pumpAndSettle();
    expect(find.text('Forget "Writing a brief"?'), findsOneWidget);
    await tester.tap(
      find.descendant(
        of: find.byType(AlertDialog),
        matching: find.text('Forget'),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      gateway.seen.where(
        (request) =>
            request.method == 'DELETE' && request.url.path == '/v1/reports/k1',
      ),
      hasLength(1),
    );
  });

  testWidgets('a Report downloads as the Markdown document the gateway '
      'writes', (tester) async {
    final gateway = knowing();
    final board = await gateway.board();
    final saved = <(String, String)>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: ooTheme(OoThemeId.values.first),
        home: Scaffold(
          body: ReportDetail(
            board: board,
            reportId: 'k1',
            onRoute: (_) {},
            saveText: (name, text) async {
              saved.add((name, text));
              return '/downloads/$name';
            },
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('report-download')));
    await tester.pumpAndSettle();
    expect(saved, hasLength(1));
    expect(saved.single.$1, 'k1.md');
    expect(saved.single.$2, startsWith('---\nid: "k1"'));
    expect(saved.single.$2, endsWith('The text of k1'));
    expect(find.textContaining('Saved to /downloads/k1.md'), findsOneWidget);
  });

  test('knowledge is told from observations by its kind', () {
    final memory = ReportSummary.fromJson(
      known('m', 'x', kind: 'memory') as Map<String, dynamic>,
    );
    expect(memory.isKnowledge, isTrue);
    expect(memory.isProposal, isFalse);
    final finding = ReportSummary.fromJson(
      report('f', 'x') as Map<String, dynamic>,
    );
    expect(finding.isKnowledge, isFalse);
    final guide = ReportSummary.fromJson(
      known('built-in.kb.x.md', 'x') as Map<String, dynamic>,
    );
    expect(guide.builtIn, isTrue);
  });
}
