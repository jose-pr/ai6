import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/green_ink/green_ink_reason.dart';
import 'package:outer_office/shell/oo_markdown.dart';
import 'package:outer_office/theme/oo_theme.dart';

const _pages = [
  SourcePageSummary(
    provider: 'jira',
    web: 'https://example.atlassian.net',
    project: 'GUA',
  ),
  SourcePageSummary(provider: 'github', web: 'https://github.com/o/n'),
  SourcePageSummary(provider: 'gitlab', web: 'https://gitlab.com/g/m'),
];

Map<String, String> _found(
  String text, [
  List<SourcePageSummary> pages = _pages,
]) => {
  for (final reference in referencesIn(text, pages))
    reference.text: reference.url.toString(),
};

/// Taps the first span reading [words] inside any rich text.
Future<void> _tapWords(WidgetTester tester, String words) async {
  final finder = find.byWidgetPredicate(
    (widget) => widget is RichText && widget.text.toPlainText().contains(words),
  );
  expect(finder, findsWidgets, reason: 'no text holds "$words"');
  final paragraph = tester.renderObject<RenderParagraph>(finder.first);
  final offset = paragraph.text.toPlainText().indexOf(words) + 1;
  final boxes = paragraph.getBoxesForSelection(
    TextSelection(baseOffset: offset, extentOffset: offset + 1),
  );
  await tester.tapAt(paragraph.localToGlobal(boxes.first.toRect().center));
  await tester.pumpAndSettle();
}

Future<List<Uri>> _shown(
  WidgetTester tester, {
  required String reason,
  List<GreenInkLink> links = const [],
}) async {
  final opened = <Uri>[];
  await tester.pumpWidget(
    MaterialApp(
      theme: ooTheme(OoThemeId.values.first),
      home: Scaffold(
        body: SingleChildScrollView(
          child: GreenInkReason(
            reason: reason,
            links: links,
            sources: Future.value(_pages),
            launcher: (url) async {
              opened.add(url);
              return true;
            },
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return opened;
}

void main() {
  group('references', () {
    test('a ticket key of a configured project leads to its tracker', () {
      expect(_found('Fixes GUA-12 and OPS-3 under SHA-256.'), {
        'GUA-12': 'https://example.atlassian.net/browse/GUA-12',
      });
    });

    test('issues, commits and branches lead to the first repository', () {
      expect(_found('See #7, commit 1a2b3c4d on branch ai6/fix-it.'), {
        '#7': 'https://github.com/o/n/issues/7',
        '1a2b3c4d': 'https://github.com/o/n/commit/1a2b3c4d',
        'ai6/fix-it': 'https://github.com/o/n/tree/ai6/fix-it',
      });
      expect(_found('Push origin:refs/heads/ai6/x now.'), {
        'refs/heads/ai6/x': 'https://github.com/o/n/tree/ai6/x',
      });
    });

    test('a repository named in full leads to that repository', () {
      expect(_found('Closes g/m#9.'), {
        'g/m#9': 'https://gitlab.com/g/m/-/issues/9',
      });
      expect(_found('Closes other/repo#9.'), isEmpty);
    });

    test('GitLab keeps its pages under its own path', () {
      expect(
        _found('Fixed in abc1234.', const [
          SourcePageSummary(provider: 'gitlab', web: 'https://gl.example/g/m'),
        ]),
        {'abc1234': 'https://gl.example/g/m/-/commit/abc1234'},
      );
    });

    test('words that only look hexadecimal are left alone', () {
      expect(_found('A defaced 1234567 thing at sha256:abc.'), isEmpty);
    });

    test('with no Sources nothing is linked', () {
      expect(_found('GUA-12 #7 1a2b3c4d branch x', const []), isEmpty);
    });
  });

  testWidgets('a linked ticket key opens the tracker at once', (tester) async {
    final opened = await _shown(tester, reason: 'Closes **GUA-12**.');
    await _tapWords(tester, 'GUA-12');
    expect(find.byType(AlertDialog), findsNothing);
    expect(opened, [Uri.parse('https://example.atlassian.net/browse/GUA-12')]);
  });

  testWidgets('a reason is rendered as Markdown', (tester) async {
    await _shown(tester, reason: '# Why\n\nIt **passed**.');
    expect(find.text('Why'), findsOneWidget);
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is RichText &&
            widget.text.toPlainText().contains('It passed.'),
      ),
      findsOneWidget,
    );
  });

  testWidgets('an Agent\'s link shows where it goes before it opens', (
    tester,
  ) async {
    final opened = await _shown(
      tester,
      reason: 'Please look.',
      links: const [
        GreenInkLink(
          label: 'Build [7] (log)',
          url: 'https://ci.example/build(7)',
        ),
        GreenInkLink(label: 'PR 42', url: 'https://github.com/o/n/pull/42'),
      ],
    );
    await _tapWords(tester, 'Build [7] (log)');
    expect(find.byType(AlertDialog), findsOneWidget);
    expect(find.text('https://ci.example/build(7)'), findsOneWidget);
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(opened.single.host, 'ci.example');

    // A link to one of the Operation's Sources opens at once.
    await _tapWords(tester, 'PR 42');
    expect(opened.last, Uri.parse('https://github.com/o/n/pull/42'));
  });

  test('a document widget renders with no Sources read', () {
    expect(const GreenInkReason(reason: 'x').sources, isNull);
    expect(
      OoMarkdown('x', references: referencesIn('x', _pages)).references,
      isEmpty,
    );
  });
}
