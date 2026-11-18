import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/shell/oo_markdown.dart';
import 'package:outer_office/theme/oo_theme.dart';

Widget _app(Widget child) => MaterialApp(
  theme: ooTheme(OoThemeId.values.first),
  home: Scaffold(body: SingleChildScrollView(child: child)),
);

/// Taps the first span reading [words] inside any rich text.
Future<void> _tapWords(WidgetTester tester, String words) async {
  final finder = find.byWidgetPredicate(
    (widget) => widget is RichText && widget.text.toPlainText().contains(words),
  );
  expect(finder, findsWidgets, reason: 'no text holds "$words"');
  final paragraph = tester.renderObject<RenderParagraph>(finder.first);
  final full = paragraph.text.toPlainText();
  final offset = full.indexOf(words) + 1;
  final boxes = paragraph.getBoxesForSelection(
    TextSelection(baseOffset: offset, extentOffset: offset + 1),
  );
  final origin = paragraph.localToGlobal(boxes.first.toRect().center);
  await tester.tapAt(origin);
  await tester.pumpAndSettle();
}

/// The text of every rich text, joined by `|`.
String _plainText(WidgetTester tester) => tester
    .widgetList<RichText>(find.byType(RichText))
    .map((rich) => rich.text.toPlainText())
    .join('|');

/// Every span that holds text, of every rich text.
List<TextSpan> _spans(WidgetTester tester) {
  final spans = <TextSpan>[];
  for (final rich in tester.widgetList<RichText>(find.byType(RichText))) {
    rich.text.visitChildren((span) {
      if (span is TextSpan && (span.text ?? '').isNotEmpty) spans.add(span);
      return true;
    });
  }
  return spans;
}

void main() {
  group('the body, drawn by the shared chat\'s Markdown', () {
    testWidgets(
      'headings, paragraphs, lists, task boxes, quotes, code, tables and rules draw',
      (tester) async {
        await tester.pumpWidget(
          _app(
            const OoMarkdown(r'''
# Title
Line one
line two

- [x] done
- [ ] open
  - nested
1. first
2. second

> quoted **words**

```rust
let x = 1;
```

| Key | Value |
|:----|------:|
| a \| b | 2 |

---
'''),
          ),
        );
        final text = _plainText(tester);
        expect(text, contains('Title'));
        expect(
          text,
          contains('Line one\nline two'),
          reason: 'a line break is kept',
        );
        expect(text, contains('done'));
        expect(text, contains('nested'));
        expect(text, contains('second'));
        expect(text, contains('quoted words'));
        expect(text, contains('let x = 1;'));
        expect(text, contains('Value'));
        expect(text, contains('a | b'));
        expect(find.byIcon(Icons.check_box), findsOneWidget);
        expect(find.byIcon(Icons.check_box_outline_blank), findsOneWidget);
      },
    );

    testWidgets('only a list from 1 ends a paragraph, as CommonMark has it', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(const OoMarkdown('Shipped in\n2026. A good year')),
      );
      expect(_plainText(tester), contains('Shipped in\n2026. A good year'));
      await tester.pumpWidget(_app(const OoMarkdown('Steps\n1. one')));
      expect(_plainText(tester).split('|'), containsAll(['Steps']));
      expect(_plainText(tester), contains('one'));
    });

    testWidgets('an unclosed fence runs to the end', (tester) async {
      await tester.pumpWidget(_app(const OoMarkdown('```\nopen')));
      expect(_plainText(tester), contains('open'));
    });

    testWidgets('marks, code and links', (tester) async {
      await tester.pumpWidget(
        _app(
          const OoMarkdown(
            'a **b** *c* ~~d~~ `e*f*` [g](https://x.test/p) <https://y.test> https://z.test/q.',
          ),
        ),
      );
      final spans = _spans(tester);
      TextSpan span(String text) => spans.firstWhere((s) => s.text == text);
      expect(span('b').style?.fontWeight, FontWeight.bold);
      expect(span('c').style?.fontStyle, FontStyle.italic);
      expect(span('d').style?.decoration, TextDecoration.lineThrough);
      expect(span('e*f*').style?.fontFamily, 'monospace');
      expect(span('g').recognizer, isNotNull);
      expect(span('https://y.test').recognizer, isNotNull);
      expect(
        span('https://z.test/q').recognizer,
        isNotNull,
        reason: 'the sentence\'s full stop is not part of the address',
      );
    });

    testWidgets('snake_case and escaped stars stay as written', (tester) async {
      await tester.pumpWidget(
        _app(const OoMarkdown(r'run_the_gate and \*not italic\*')),
      );
      expect(_plainText(tester), contains('run_the_gate and *not italic*'));
      expect(
        _spans(tester).every((s) => s.style?.fontStyle != FontStyle.italic),
        isTrue,
      );
    });

    testWidgets('an image is not fetched and HTML is not drawn', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(
          const OoMarkdown(
            '![secret](https://evil.test/?d=1) <script>alert(1)</script>',
          ),
        ),
      );
      expect(find.byType(Image), findsNothing);
      expect(_plainText(tester), isNot(contains('evil.test')));
    });
  });

  group('frontmatter', () {
    test('a mapping is split from the body', () {
      final split = splitFrontmatter(
        '---\nmission: M1\ntags: [a, b]\n---\n# Body',
      );
      expect(split.frontmatter, {
        'mission': 'M1',
        'tags': ['a', 'b'],
      });
      expect(split.body, '# Body');
    });

    test('an unreadable block is kept as text and the body still renders', () {
      final split = splitFrontmatter('---\n: : [\n---\nBody');
      expect(split.frontmatter, isNull);
      expect(split.unreadable, contains(': : ['));
      expect(split.body, 'Body');
    });

    test('a rule with no frontmatter is not mistaken for one', () {
      final split = splitFrontmatter('Intro\n---\nmore');
      expect(split.frontmatter, isNull);
      expect(split.body, startsWith('Intro'));
    });

    test('anchors and aliases are refused, so nothing is expanded', () {
      final split = splitFrontmatter(
        '---\nlol: &a [x, x, x]\nlol2: &b [*a, *a, *a]\nlol3: &c [*b, *b, *b]\n---\nBody',
      );
      expect(split.frontmatter, isNull);
      expect(split.unreadable, contains('&a'));
      expect(split.body, 'Body');
      final merged = splitFrontmatter(
        '---\nbase: &b {a: 1}\nuse:\n  <<: *b\n---\nBody',
      );
      expect(merged.frontmatter, isNull);
      expect(merged.unreadable, isNotNull);
    });

    testWidgets(
      'a reference in a frontmatter value is a link, and a code span is not read for references',
      (tester) async {
        await tester.pumpWidget(
          _app(
            const OoMarkdown(
              '---\nmission: M-12\n---\nSee `M-12` and M-12.',
              references: [DocReference(text: 'M-12', route: 'app:/missions')],
            ),
          ),
        );
        final linked = [
          for (final s in _spans(tester))
            if (s.recognizer != null) s.text,
        ];
        expect(linked, [
          'M-12',
          'M-12',
        ], reason: 'the value and the sentence, not the code span');
      },
    );

    testWidgets('fields show above the body, and a bad block says so', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(const OoMarkdown('---\nagent: "007"\nstation: hq\n---\n## Why')),
      );
      expect(find.text('agent'), findsOneWidget);
      expect(find.text('Why'), findsOneWidget);
      await tester.pumpWidget(_app(const OoMarkdown('---\n: : [\n---\nBody')));
      expect(find.text('Frontmatter not readable'), findsOneWidget);
      expect(find.text('Body'), findsOneWidget);
    });
  });

  group('links', () {
    testWidgets('a tracker link opens at once', (tester) async {
      final opened = <Uri>[];
      await tester.pumpWidget(
        _app(
          OoMarkdown(
            'See [the issue](https://github.com/o/r/issues/4).',
            trackers: const {'github.com'},
            launcher: (url) async {
              opened.add(url);
              return true;
            },
          ),
        ),
      );
      await _tapWords(tester, 'the issue');
      expect(opened, [Uri.parse('https://github.com/o/r/issues/4')]);
    });

    testWidgets('another host shows its full address first', (tester) async {
      final opened = <Uri>[];
      await tester.pumpWidget(
        _app(
          OoMarkdown(
            '[the issue](https://evil.test/login)',
            trackers: const {'github.com'},
            launcher: (url) async {
              opened.add(url);
              return true;
            },
          ),
        ),
      );
      await _tapWords(tester, 'the issue');
      expect(find.text('Open this address?'), findsOneWidget);
      expect(find.text('https://evil.test/login'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(opened, isEmpty);
      await _tapWords(tester, 'the issue');
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(opened, [Uri.parse('https://evil.test/login')]);
    });

    testWidgets('a script link is text, and web links can be off', (
      tester,
    ) async {
      var opened = 0;
      Future<bool> launcher(Uri url) async {
        opened++;
        return true;
      }

      await tester.pumpWidget(
        _app(
          OoMarkdown(
            '[click](javascript:alert(1)) and https://a.test',
            webLinks: false,
            launcher: launcher,
          ),
        ),
      );
      await _tapWords(tester, 'click');
      await _tapWords(tester, 'a.test');
      expect(opened, 0);
      expect(find.byType(AlertDialog), findsNothing);
    });

    testWidgets('an app link opens in the app', (tester) async {
      final routes = <String>[];
      await tester.pumpWidget(
        _app(
          OoMarkdown(
            '[Mission](app:/missions?mission_id=M1)',
            onOpen: routes.add,
          ),
        ),
      );
      await _tapWords(tester, 'Mission');
      expect(routes, ['app:/missions?mission_id=M1']);
    });

    testWidgets(
      'a Mission reference offers its page, its issues and its boards',
      (tester) async {
        final routes = <String>[];
        final opened = <Uri>[];
        await tester.pumpWidget(
          _app(
            OoMarkdown(
              'Follows M-12 and fixes ABC-7.',
              trackers: const {'github.com', 'acme.atlassian.net'},
              onOpen: routes.add,
              launcher: (url) async {
                opened.add(url);
                return true;
              },
              references: [
                DocReference(
                  text: 'M-12',
                  route: 'app:/missions?operation_id=G&mission_id=M-12',
                  issues: [
                    DocLink(
                      label: 'o/r#4 Login fails',
                      url: Uri.parse('https://github.com/o/r/issues/4'),
                    ),
                  ],
                  boards: [
                    DocLink(
                      label: 'o/r issues',
                      url: Uri.parse('https://github.com/o/r/issues'),
                    ),
                  ],
                ),
                DocReference(
                  text: 'ABC-7',
                  url: Uri.parse('https://acme.atlassian.net/browse/ABC-7'),
                ),
              ],
            ),
          ),
        );
        await _tapWords(tester, 'ABC-7');
        expect(opened, [Uri.parse('https://acme.atlassian.net/browse/ABC-7')]);

        await _tapWords(tester, 'M-12');
        expect(find.text('Open in Outer Office'), findsOneWidget);
        expect(find.text('o/r#4 Login fails'), findsOneWidget);
        expect(find.text('o/r issues'), findsOneWidget);
        await tester.tap(find.text('o/r issues'));
        await tester.pumpAndSettle();
        expect(opened.last, Uri.parse('https://github.com/o/r/issues'));

        await _tapWords(tester, 'M-12');
        await tester.tap(find.text('Open in Outer Office'));
        await tester.pumpAndSettle();
        expect(routes, ['app:/missions?operation_id=G&mission_id=M-12']);
      },
    );

    testWidgets('a reference inside a longer word is not linked', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(
          const OoMarkdown(
            'XABC-7 and ABC-70',
            references: [DocReference(text: 'ABC-7', route: 'app:/x')],
          ),
        ),
      );
      final rich = tester.widgetList<RichText>(find.byType(RichText));
      final linked = <String>[];
      for (final text in rich) {
        text.text.visitChildren((span) {
          if (span is TextSpan && span.recognizer != null) {
            linked.add(span.text ?? '');
          }
          return true;
        });
      }
      expect(linked, isEmpty);
    });
  });

  testWidgets('invisible characters are shown', (tester) async {
    await tester.pumpWidget(_app(const OoMarkdown('safe\u{E0041}text')));
    expect(
      find.byWidgetPredicate(
        (w) => w is RichText && w.text.toPlainText().contains('<tags:'),
      ),
      findsOneWidget,
    );
  });
}
