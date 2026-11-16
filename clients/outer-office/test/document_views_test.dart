import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/green_ink/green_ink_reason.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/reviews/mission_reviews.dart';
import 'package:outer_office/shell/oo_document_view.dart';
import 'package:outer_office/shell/oo_markdown.dart';

import 'links_test.dart' show demoAt;

/// Whether some text says exactly [words] as a tappable span.
bool _hasLink(WidgetTester tester, String words) {
  var found = false;
  for (final rich in tester.widgetList<RichText>(find.byType(RichText))) {
    rich.text.visitChildren((span) {
      if (span is TextSpan && span.recognizer != null && span.text == words) {
        found = true;
      }
      return true;
    });
  }
  return found;
}

/// A document on screen: its heading, a table cell, a clickable issue key and issue, and the
/// action that copies it.
void _expectDocument(
  WidgetTester tester, {
  required String heading,
  required String cell,
}) {
  expect(
    find.textContaining(heading, findRichText: true),
    findsWidgets,
    reason: 'heading $heading',
  );
  expect(
    find.textContaining(cell, findRichText: true),
    findsWidgets,
    reason: 'a table cell',
  );
  expect(_hasLink(tester, 'OPS-12'), isTrue, reason: 'the Jira key links');
  expect(_hasLink(tester, 'acme/demo#12'), isTrue, reason: 'the issue links');
  expect(find.text('Copy as Markdown'), findsOneWidget);
}

Future<void> _clipboardTo(WidgetTester tester, List<String> copied) async {
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.platform,
    (call) async {
      if (call.method == 'Clipboard.setData') {
        copied.add((call.arguments as Map)['text'] as String);
      }
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      null,
    ),
  );
}

void main() {
  testWidgets('a Mission brief is a document, its launch context from the '
      'frontmatter', (tester) async {
    final copied = <String>[];
    await _clipboardTo(tester, copied);
    await demoAt(tester, '/missions');
    await tester.tap(find.byTooltip('View Mission brief and launch context'));
    await tester.pumpAndSettle();
    _expectDocument(
      tester,
      heading: 'Review the requested change',
      cell: 'Tests',
    );
    expect(find.textContaining('working_directory'), findsWidgets);
    expect(find.text('AI6_MISSION_ID'), findsWidgets);
    await tester.tap(find.text('Copy as Markdown'));
    await tester.pumpAndSettle();
    expect(copied.single, startsWith('---\nkind: brief'));
  });

  testWidgets('a green ink request opens as a document', (tester) async {
    await demoAt(tester, '/green_ink');
    expect(find.text('Open as document'), findsOneWidget);
    await tester.tap(find.text('Open as document'));
    await tester.pumpAndSettle();
    _expectDocument(tester, heading: 'Why this push', cell: 'Plan');
  });

  testWidgets('a held brief shows its text as a document', (tester) async {
    await demoAt(tester, '/briefings?tab=held');
    await tester.tap(find.text('Show the brief text'));
    await tester.pumpAndSettle();
    _expectDocument(tester, heading: 'Crash on save', cell: 'Save it');
  });

  testWidgets('a Station reads its report as a document', (tester) async {
    await demoAt(tester, '/stations?station_id=station-control-01');
    await tester.tap(find.text('Read the report').first);
    await tester.pumpAndSettle();
    _expectDocument(
      tester,
      heading: 'Head of Station report',
      cell: 'build-box',
    );
  });

  testWidgets('a long green ink reason shows its first lines and Read all', (
    tester,
  ) async {
    final reason = [for (var i = 1; i <= 12; i++) 'line $i'].join('\n');
    expect(markdownPreview(reason, 3), 'line 1\nline 2\nline 3');
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: GreenInkReason(
            reason: reason,
            readAll: () async => OoDocument(markdown: reason),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('line 1', findRichText: true), findsWidgets);
    expect(find.textContaining('line 12', findRichText: true), findsNothing);
    await tester.tap(find.text('Read all'));
    await tester.pumpAndSettle();
    expect(find.textContaining('line 12', findRichText: true), findsWidgets);
  });

  test('a cut inside a code fence closes it', () {
    expect(markdownPreview('```\na\nb\nc', 2), '```\na\n```');
    expect(markdownPreview('a\nb', 2), isNull);
  });

  testWidgets('the review section renders the reviews document', (
    tester,
  ) async {
    const document =
        '---\nkind: review\n---\n\n## Output review\n\n'
        'See OPS-12 and acme/demo#12.\n\n| Check | Result |\n| --- | --- |\n'
        '| Tests | pass |\n';
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      pollInterval: null,
      client: MockClient((request) async {
        final path = request.url.path;
        http.Response json(Object body, [int status = 200]) => http.Response(
          jsonEncode(body),
          status,
          headers: {'content-type': 'application/json'},
        );
        return switch (path) {
          '/v1/auth/login' => json({'token': 'user-token'}, 201),
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
          '/v1/user/operations/G/missions/m1/reviews' => json([
            {
              'gate': 'output',
              'policy': 'two-models',
              'pass_at': 0.66,
              'distinct_models': 2,
              'rounds': 2,
              'redo_limit': 1,
              'status': 'person',
              'iteration': 1,
              'round': 1,
              'sent_back': 0,
              'panel': [],
              'votes': [],
              'turns': 1,
              'tokens': 10,
              'overrule': null,
            },
          ]),
          '/v1/user/operations/G/missions/m1/reviews/document' => json({
            'markdown': document,
            'references': [
              {
                'text': 'OPS-12',
                'kind': 'issue',
                'url': 'https://tracker.example.com/browse/OPS-12',
              },
              {
                'text': 'acme/demo#12',
                'kind': 'issue',
                'url': 'https://git.example.com/acme/demo/issues/12',
              },
            ],
            'trackers': ['tracker.example.com', 'git.example.com'],
          }),
          _ => json(const <Object>[]),
        };
      }),
    );
    await board.login('operator', 'secret');
    addTearDown(board.dispose);
    tester.view.physicalSize = const Size(1280, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: MissionReviewsSection(
              board: board,
              operationId: 'G',
              missionId: 'm1',
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    _expectDocument(tester, heading: 'Output review', cell: 'Tests');
  });
}
