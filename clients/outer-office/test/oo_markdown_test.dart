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

void main() {
  group('blocks', () {
    test('headings, paragraphs, lists, quotes, code, tables and rules', () {
      final blocks = parseBlocks('''
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
| a \\| b | 2 |

---
''');
      expect(blocks.map((b) => b.runtimeType.toString()), [
        'MdHeading',
        'MdParagraph',
        'MdList',
        'MdList',
        'MdQuote',
        'MdCode',
        'MdTable',
        'MdRule',
      ]);
      expect((blocks[1] as MdParagraph).text, 'Line one\nline two');
      final tasks = blocks[2] as MdList;
      expect(tasks.items.map((i) => i.checked), [true, false]);
      expect(
        (tasks.items[1].children.single as MdList).items.single.text,
        'nested',
      );
      final ordered = blocks[3] as MdList;
      expect(ordered.ordered, isTrue);
      expect(ordered.items.map((i) => i.text), ['first', 'second']);
      expect((blocks[5] as MdCode).info, 'rust');
      final table = blocks[6] as MdTable;
      expect(table.header, ['Key', 'Value']);
      expect(table.align, [MdAlign.left, MdAlign.right]);
      expect(table.rows.single, ['a | b', '2']);
    });

    test('only a list from 1 ends a paragraph, as CommonMark has it', () {
      expect(
        (parseBlocks('Shipped in\n2026. A good year').single as MdParagraph)
            .text,
        'Shipped in\n2026. A good year',
      );
      expect(parseBlocks('Steps\n1. one'), hasLength(2));
    });

    test('an unclosed fence runs to the end', () {
      final code = parseBlocks('```\nopen').single as MdCode;
      expect(code.text, 'open');
    });
  });

  group('inline', () {
    test('marks, code and links', () {
      final runs = parseInline(
        'a **b** *c* ~~d~~ `e*f*` [g](https://x.test/p) <https://y.test> https://z.test/q.',
      );
      expect(runs.firstWhere((r) => r.text == 'b').bold, isTrue);
      expect(runs.firstWhere((r) => r.text == 'c').italic, isTrue);
      expect(runs.firstWhere((r) => r.text == 'd').strike, isTrue);
      expect(runs.firstWhere((r) => r.text == 'e*f*').code, isTrue);
      expect(runs.firstWhere((r) => r.text == 'g').link, 'https://x.test/p');
      expect(runs.where((r) => r.link == 'https://y.test'), hasLength(1));
      expect(
        runs.where((r) => r.link == 'https://z.test/q'),
        hasLength(1),
        reason: 'the sentence\'s full stop is not part of the address',
      );
    });

    test('snake_case and escaped stars stay as written', () {
      final runs = parseInline(r'run_the_gate and \*not italic\*');
      expect(runs.every((r) => !r.italic), isTrue);
      expect(runs.map((r) => r.text).join(), 'run_the_gate and *not italic*');
    });

    test('with markup off only links are found', () {
      final runs = parseInline('**x** https://a.test', markup: false);
      expect(runs.first.text, '**x** ');
      expect(runs.last.link, 'https://a.test');
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
