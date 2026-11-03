import 'package:flutter/gestures.dart';
import 'package:material_ui/material_ui.dart';
import 'package:yaml/yaml.dart';

import '../assistant/pages.dart' show PageLauncher, launchPage;
import '../held/held_briefs_page.dart' show revealInvisible;
import '../theme/oo_theme.dart';

/// A web page a reference leads to: a linked issue or a tracker's board.
class DocLink {
  const DocLink({required this.label, required this.url});

  factory DocLink.fromJson(Map<String, dynamic> json) => DocLink(
    label: json['label'] as String,
    url: Uri.parse(json['url'] as String),
  );

  final String label;
  final Uri url;
}

/// Text in a document that Universal Exports resolved for the reader: an issue key, or a
/// Mission, Operation or Session ID the reader may open.
class DocReference {
  const DocReference({
    required this.text,
    this.route,
    this.url,
    this.issues = const [],
    this.boards = const [],
  });

  factory DocReference.fromJson(Map<String, dynamic> json) => DocReference(
    text: json['text'] as String,
    route: json['route'] as String?,
    url: switch (json['url']) {
      final String url => Uri.tryParse(url),
      _ => null,
    },
    issues: [
      for (final issue in (json['issues'] as List<dynamic>? ?? const []))
        DocLink.fromJson(issue as Map<String, dynamic>),
    ],
    boards: [
      for (final board in (json['boards'] as List<dynamic>? ?? const []))
        DocLink.fromJson(board as Map<String, dynamic>),
    ],
  );

  /// The words as they appear in the body.
  final String text;

  /// Its page in the app (`app:/…`), when it has one.
  final String? route;

  /// Its page on a tracker, for an issue key.
  final Uri? url;

  /// A Mission's linked issues.
  final List<DocLink> issues;

  /// The boards of its Operation's Sources.
  final List<DocLink> boards;
}

/// A document as Universal Exports serves it: Markdown with frontmatter, the references it
/// resolved for this reader, and the hosts of the trackers it knows.
class OoDocument {
  const OoDocument({
    required this.markdown,
    this.references = const [],
    this.trackers = const {},
  });

  factory OoDocument.fromJson(Map<String, dynamic> json) => OoDocument(
    markdown: json['markdown'] as String,
    references: [
      for (final reference
          in (json['references'] as List<dynamic>? ?? const []))
        DocReference.fromJson(reference as Map<String, dynamic>),
    ],
    trackers: {
      for (final host in (json['trackers'] as List<dynamic>? ?? const []))
        (host as String).toLowerCase(),
    },
  );

  final String markdown;
  final List<DocReference> references;
  final Set<String> trackers;
}

/// A document's frontmatter and body, split.
class SplitDocument {
  const SplitDocument({this.frontmatter, this.unreadable, required this.body});

  /// The frontmatter, when the document has a readable one.
  final Map<String, Object?>? frontmatter;

  /// The frontmatter's text when it is there but is not a YAML mapping.
  final String? unreadable;

  final String body;
}

/// Splits a leading `---` YAML block from [markdown].
SplitDocument splitFrontmatter(String markdown) {
  final text = markdown.replaceAll('\r\n', '\n');
  if (!text.startsWith('---\n')) return SplitDocument(body: text);
  final close = RegExp(r'^(---|\.\.\.)[ \t]*$', multiLine: true);
  final end = close.firstMatch(text.substring(4));
  if (end == null) return SplitDocument(body: text);
  final yaml = text.substring(4, 4 + end.start);
  var body = text.substring(4 + end.end);
  if (body.startsWith('\n')) body = body.substring(1);
  try {
    final parsed = loadYaml(yaml);
    if (parsed == null) return SplitDocument(body: body);
    if (parsed is YamlMap) {
      return SplitDocument(
        frontmatter: _plain(parsed) as Map<String, Object?>,
        body: body,
      );
    }
  } on YamlException {
    // Shown as written below.
  }
  return SplitDocument(unreadable: yaml, body: body);
}

Object? _plain(Object? value) => switch (value) {
  YamlMap() => <String, Object?>{
    for (final entry in value.entries) '${entry.key}': _plain(entry.value),
  },
  YamlList() => [for (final item in value) _plain(item)],
  _ => value,
};

/// A Markdown document for a person to read: its frontmatter as a panel of fields, then the
/// body's headings, paragraphs, lists, task boxes, quotes, code, tables and rules.
///
/// The text is untrusted. Nothing is fetched (no images) and no HTML is rendered; invisible
/// characters are shown. `app:/…` links and [references] open in the app through [onOpen]. A
/// web link opens in the browser: at once when its host is one of [trackers], after the reader
/// sees the full address otherwise, and never when [webLinks] is off. Other schemes are text.
class OoMarkdown extends StatefulWidget {
  const OoMarkdown(
    this.markdown, {
    this.references = const [],
    this.trackers = const {},
    this.onOpen,
    this.webLinks = true,
    this.selectable = true,
    this.launcher = launchPage,
    super.key,
  });

  /// [document] with its references and trackers.
  OoMarkdown.document(
    OoDocument document, {
    this.onOpen,
    this.webLinks = true,
    this.selectable = true,
    this.launcher = launchPage,
    super.key,
  }) : markdown = document.markdown,
       references = document.references,
       trackers = document.trackers;

  final String markdown;
  final List<DocReference> references;
  final Set<String> trackers;
  final void Function(String link)? onOpen;
  final bool webLinks;

  /// Whether the text sits in its own selection area; off where the page already has one.
  final bool selectable;
  final PageLauncher launcher;

  @override
  State<OoMarkdown> createState() => _OoMarkdownState();
}

class _OoMarkdownState extends State<OoMarkdown> {
  final _recognizers = <TapGestureRecognizer>[];

  @override
  void dispose() {
    _dropRecognizers();
    super.dispose();
  }

  void _dropRecognizers() {
    for (final recognizer in _recognizers) {
      recognizer.dispose();
    }
    _recognizers.clear();
  }

  TapGestureRecognizer _tap(VoidCallback onTap) {
    final recognizer = TapGestureRecognizer()..onTap = onTap;
    _recognizers.add(recognizer);
    return recognizer;
  }

  @override
  Widget build(BuildContext context) {
    _dropRecognizers();
    final split = splitFrontmatter(widget.markdown);
    final blocks = parseBlocks(split.body);
    final column = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      spacing: 8,
      children: [
        if (split.frontmatter case final fields? when fields.isNotEmpty)
          _Frontmatter(fields: fields, value: _valueSpans),
        if (split.unreadable case final yaml?) ...[
          Text(
            'Frontmatter not readable',
            style: Theme.of(context).textTheme.labelMedium,
          ),
          _code(context, yaml),
        ],
        for (final block in blocks) _block(context, block),
      ],
    );
    return widget.selectable ? SelectionArea(child: column) : column;
  }

  Widget _block(BuildContext context, MdBlock block) {
    final theme = Theme.of(context);
    final tokens = OoTokens.of(context);
    return switch (block) {
      MdHeading(:final level, :final text) => Padding(
        padding: EdgeInsets.only(top: level <= 2 ? 6 : 2),
        child: Text.rich(
          TextSpan(children: _inline(context, text)),
          style: switch (level) {
            1 => theme.textTheme.titleLarge,
            2 => theme.textTheme.titleMedium,
            _ => theme.textTheme.titleSmall,
          },
        ),
      ),
      MdParagraph(:final text) => Text.rich(
        TextSpan(children: _inline(context, text)),
      ),
      MdCode(:final text) => _code(context, text),
      MdRule() => Divider(color: tokens.border, height: 12),
      MdQuote(:final blocks) => Container(
        padding: const EdgeInsets.only(left: 12),
        decoration: BoxDecoration(
          border: Border(left: BorderSide(color: tokens.border, width: 3)),
        ),
        child: DefaultTextStyle.merge(
          style: TextStyle(color: tokens.muted),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            spacing: 6,
            children: [for (final inner in blocks) _block(context, inner)],
          ),
        ),
      ),
      MdList() => _list(context, block),
      MdTable() => _table(context, block),
    };
  }

  Widget _list(BuildContext context, MdList list) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    spacing: 4,
    children: [
      for (final (index, item) in list.items.indexed)
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(
              width: 28,
              child: switch (item.checked) {
                final bool done => Icon(
                  done ? Icons.check_box : Icons.check_box_outline_blank,
                  size: 18,
                  semanticLabel: done ? 'Done' : 'Not done',
                ),
                null => Text(
                  list.ordered ? '${list.start + index}.' : '•',
                  textAlign: TextAlign.right,
                ),
              },
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                spacing: 4,
                children: [
                  Text.rich(TextSpan(children: _inline(context, item.text))),
                  for (final inner in item.children) _block(context, inner),
                ],
              ),
            ),
          ],
        ),
    ],
  );

  Widget _table(BuildContext context, MdTable table) {
    final tokens = OoTokens.of(context);
    final columns = table.header.length;
    TextAlign align(int column) =>
        switch (table.align.elementAtOrNull(column)) {
          MdAlign.right => TextAlign.right,
          MdAlign.center => TextAlign.center,
          _ => TextAlign.left,
        };
    TableRow row(List<String> cells, {bool header = false}) => TableRow(
      decoration: header
          ? BoxDecoration(
              border: Border(bottom: BorderSide(color: tokens.border)),
            )
          : null,
      children: [
        for (var column = 0; column < columns; column++)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            child: Text.rich(
              TextSpan(
                children: _inline(context, cells.elementAtOrNull(column) ?? ''),
              ),
              textAlign: align(column),
              style: header
                  ? const TextStyle(fontWeight: FontWeight.bold)
                  : null,
            ),
          ),
      ],
    );
    return SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: Table(
        defaultColumnWidth: const IntrinsicColumnWidth(),
        children: [
          row(table.header, header: true),
          for (final cells in table.rows) row(cells),
        ],
      ),
    );
  }

  Widget _code(BuildContext context, String text) {
    final tokens = OoTokens.of(context);
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: tokens.bg,
        border: Border.all(color: tokens.border),
        borderRadius: BorderRadius.circular(6),
      ),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: Text(
          revealInvisible(text),
          style: TextStyle(fontFamily: 'monospace', color: tokens.text),
        ),
      ),
    );
  }

  /// A frontmatter value: links and references, no emphasis.
  List<InlineSpan> _valueSpans(BuildContext context, String text) =>
      _inline(context, text, markup: false);

  List<InlineSpan> _inline(
    BuildContext context,
    String text, {
    bool markup = true,
  }) {
    final spans = <InlineSpan>[];
    for (final run in parseInline(text, markup: markup)) {
      spans.add(_span(context, run));
    }
    return spans;
  }

  InlineSpan _span(BuildContext context, MdRun run) {
    final tokens = OoTokens.of(context);
    var style = TextStyle(
      fontWeight: run.bold ? FontWeight.bold : null,
      fontStyle: run.italic ? FontStyle.italic : null,
      decoration: run.strike ? TextDecoration.lineThrough : null,
      fontFamily: run.code ? 'monospace' : null,
      backgroundColor: run.code ? tokens.bg : null,
    );
    final shown = revealInvisible(run.text);
    if (run.code) return TextSpan(text: shown, style: style);
    final target = run.link;
    if (target != null) {
      final opener = _opener(context, target, label: run.text);
      if (opener != null) {
        return TextSpan(
          text: shown,
          style: style.merge(_linkStyle(tokens)),
          recognizer: _tap(opener),
          mouseCursor: SystemMouseCursors.click,
        );
      }
      return TextSpan(text: shown, style: style);
    }
    final children = <InlineSpan>[];
    for (final (piece, reference) in _references(run.text)) {
      final words = revealInvisible(piece);
      if (reference == null) {
        children.add(TextSpan(text: words));
        continue;
      }
      final opener = _referenceOpener(context, reference);
      children.add(
        TextSpan(
          text: words,
          style: _linkStyle(tokens),
          recognizer: opener == null ? null : _tap(opener),
          mouseCursor: opener == null ? null : SystemMouseCursors.click,
        ),
      );
    }
    return TextSpan(style: style, children: children);
  }

  TextStyle _linkStyle(OoTokens tokens) => TextStyle(
    color: tokens.accent,
    decoration: TextDecoration.underline,
    decorationColor: tokens.accent,
  );

  /// [text] cut at each reference it holds, longest first, at word edges.
  List<(String, DocReference?)> _references(String text) {
    if (widget.references.isEmpty) return [(text, null)];
    final sorted = [...widget.references]
      ..sort((a, b) => b.text.length.compareTo(a.text.length));
    final pattern = RegExp(
      '(?<![\\w/#-])(${sorted.map((r) => RegExp.escape(r.text)).join('|')})(?![\\w-])',
    );
    final byText = {for (final r in sorted) r.text: r};
    final out = <(String, DocReference?)>[];
    var at = 0;
    for (final match in pattern.allMatches(text)) {
      if (match.start > at) out.add((text.substring(at, match.start), null));
      out.add((match.group(1)!, byText[match.group(1)!]));
      at = match.end;
    }
    if (at < text.length) out.add((text.substring(at), null));
    return out;
  }

  VoidCallback? _opener(
    BuildContext context,
    String target, {
    required String label,
  }) {
    if (target.startsWith('app:/')) {
      final open = widget.onOpen;
      return open == null ? null : () => open(target);
    }
    final url = Uri.tryParse(target);
    if (url == null || !const {'http', 'https'}.contains(url.scheme)) {
      return null;
    }
    if (!widget.webLinks || url.host.isEmpty) return null;
    return () => _openWeb(context, url);
  }

  VoidCallback? _referenceOpener(BuildContext context, DocReference reference) {
    final route = reference.route;
    final open = widget.onOpen;
    final webs = widget.webLinks
        ? [...reference.issues, ...reference.boards]
        : const <DocLink>[];
    if (webs.isEmpty) {
      if (route != null && open != null) return () => open(route);
      if (reference.url case final url? when widget.webLinks) {
        return () => _openWeb(context, url);
      }
      return null;
    }
    return () => _referenceMenu(context, reference);
  }

  Future<void> _referenceMenu(
    BuildContext context,
    DocReference reference,
  ) async {
    final route = reference.route;
    final open = widget.onOpen;
    final choice = await showDialog<Object>(
      context: context,
      builder: (context) => SimpleDialog(
        title: Text(revealInvisible(reference.text)),
        children: [
          if (route != null && open != null)
            SimpleDialogOption(
              onPressed: () => Navigator.pop(context, route),
              child: const Text('Open in Outer Office'),
            ),
          for (final (heading, links) in [
            ('Linked issues', reference.issues),
            ('Boards', reference.boards),
          ])
            if (links.isNotEmpty) ...[
              Padding(
                padding: const EdgeInsets.fromLTRB(24, 12, 24, 4),
                child: Text(heading, style: ooSectionLabel(context)),
              ),
              for (final link in links)
                SimpleDialogOption(
                  onPressed: () => Navigator.pop(context, link.url),
                  child: Row(
                    spacing: 8,
                    children: [
                      Flexible(child: Text(revealInvisible(link.label))),
                      const Icon(Icons.open_in_new, size: 16),
                    ],
                  ),
                ),
            ],
        ],
      ),
    );
    if (!context.mounted) return;
    switch (choice) {
      case final String route:
        open?.call(route);
      case final Uri url:
        await _openWeb(context, url);
    }
  }

  Future<void> _openWeb(BuildContext context, Uri url) async {
    if (!widget.trackers.contains(url.host.toLowerCase())) {
      final go = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('Open this address?'),
          content: SelectableText(revealInvisible(url.toString())),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Open'),
            ),
          ],
        ),
      );
      if (go != true) return;
    }
    await widget.launcher(url);
  }
}

/// The frontmatter's fields, one row each; a list is its items and a mapping is indented.
class _Frontmatter extends StatelessWidget {
  const _Frontmatter({required this.fields, required this.value});

  final Map<String, Object?> fields;
  final List<InlineSpan> Function(BuildContext, String) value;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        border: Border.all(color: tokens.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: 4,
        children: _rows(context, fields, 0),
      ),
    );
  }

  List<Widget> _rows(
    BuildContext context,
    Map<String, Object?> map,
    int depth,
  ) {
    final label = Theme.of(context).textTheme.labelMedium
        ?.copyWith(color: OoTokens.of(context).muted);
    return [
      for (final MapEntry(:key, value: field) in map.entries)
        if (field is Map<String, Object?>) ...[
          Padding(
            padding: EdgeInsets.only(left: depth * 16.0),
            child: Text(revealInvisible(key), style: label),
          ),
          ..._rows(context, field, depth + 1),
        ] else
          Padding(
            padding: EdgeInsets.only(left: depth * 16.0),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  width: 150,
                  child: Text(revealInvisible(key), style: label),
                ),
                Expanded(child: _value(context, field)),
              ],
            ),
          ),
    ];
  }

  Widget _value(BuildContext context, Object? field) => switch (field) {
    final List<Object?> items => Wrap(
      spacing: 6,
      runSpacing: 4,
      children: [
        for (final item in items)
          Text.rich(TextSpan(children: value(context, _scalar(item)))),
      ].separatedBy(const Text('·')),
    ),
    _ => Text.rich(TextSpan(children: value(context, _scalar(field)))),
  };

  static String _scalar(Object? value) => switch (value) {
    null => '—',
    final Map<String, Object?> map =>
      map.entries.map((e) => '${e.key}: ${_scalar(e.value)}').join(', '),
    final List<Object?> list => list.map(_scalar).join(', '),
    _ => '$value',
  };
}

extension on List<Widget> {
  List<Widget> separatedBy(Widget separator) => [
    for (final (index, widget) in indexed) ...[
      if (index > 0) separator,
      widget,
    ],
  ];
}

// The parser: line-based CommonMark blocks a report needs, and inline runs.

sealed class MdBlock {
  const MdBlock();
}

class MdHeading extends MdBlock {
  const MdHeading(this.level, this.text);
  final int level;
  final String text;
}

class MdParagraph extends MdBlock {
  const MdParagraph(this.text);
  final String text;
}

class MdCode extends MdBlock {
  const MdCode(this.text, {this.info});
  final String text;
  final String? info;
}

class MdRule extends MdBlock {
  const MdRule();
}

class MdQuote extends MdBlock {
  const MdQuote(this.blocks);
  final List<MdBlock> blocks;
}

class MdListItem {
  MdListItem(this.text, {this.checked});
  String text;
  final bool? checked;
  final children = <MdBlock>[];
}

class MdList extends MdBlock {
  MdList({required this.ordered, this.start = 1});
  final bool ordered;
  final int start;
  final items = <MdListItem>[];
}

enum MdAlign { left, center, right }

class MdTable extends MdBlock {
  const MdTable(this.header, this.align, this.rows);
  final List<String> header;
  final List<MdAlign?> align;
  final List<List<String>> rows;
}

final _fence = RegExp(r'^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)');
final _heading = RegExp(r'^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$');
final _rule = RegExp(r'^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$');
final _item = RegExp(r'^( *)([-*+]|(\d{1,9})[.)])(?:[ \t]+(.*))?$');
final _task = RegExp(r'^\[([ xX])\][ \t]+');
final _delimiter = RegExp(r'^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$');

/// The blocks of [body].
List<MdBlock> parseBlocks(String body) =>
    _Blocks(body.replaceAll('\r\n', '\n').replaceAll('\t', '    ').split('\n'))
        .parse();

class _Blocks {
  _Blocks(this.lines);
  final List<String> lines;
  var _at = 0;

  List<MdBlock> parse() {
    final blocks = <MdBlock>[];
    while (_at < lines.length) {
      final line = lines[_at];
      if (line.trim().isEmpty) {
        _at++;
      } else if (_fence.firstMatch(line) case final fence?) {
        blocks.add(_code(fence));
      } else if (_heading.firstMatch(line) case final heading?) {
        blocks.add(MdHeading(heading.group(1)!.length, heading.group(2) ?? ''));
        _at++;
      } else if (_rule.hasMatch(line)) {
        blocks.add(const MdRule());
        _at++;
      } else if (line.trimLeft().startsWith('>')) {
        blocks.add(_quote());
      } else if (_item.firstMatch(line) case final item?
          when item.group(4) != null) {
        blocks.add(_list(item.group(1)!.length));
      } else if (_at + 1 < lines.length &&
          line.contains('|') &&
          _delimiter.hasMatch(lines[_at + 1])) {
        blocks.add(_table());
      } else {
        blocks.add(_paragraph());
      }
    }
    return blocks;
  }

  bool _startsBlock(String line) =>
      line.trim().isEmpty ||
      _fence.hasMatch(line) ||
      _heading.hasMatch(line) ||
      _rule.hasMatch(line) ||
      line.trimLeft().startsWith('>') ||
      _interrupts(_item.firstMatch(line));

  /// Whether a list item may end a paragraph: CommonMark lets an ordered one do so only when
  /// it starts at 1, so a line opening with a year carries the paragraph on.
  static bool _interrupts(RegExpMatch? item) =>
      item != null &&
      item.group(4) != null &&
      (item.group(3) == null || item.group(3) == '1');

  MdCode _code(RegExpMatch fence) {
    final marker = fence.group(1)!;
    final info = fence.group(2);
    final body = <String>[];
    _at++;
    while (_at < lines.length) {
      final line = lines[_at];
      final close = line.trimLeft();
      if (close.startsWith(marker[0] * marker.length) &&
          close.replaceAll(marker[0], '').trim().isEmpty) {
        _at++;
        break;
      }
      body.add(line);
      _at++;
    }
    return MdCode(body.join('\n'), info: info == '' ? null : info);
  }

  MdQuote _quote() {
    final inner = <String>[];
    while (_at < lines.length && lines[_at].trimLeft().startsWith('>')) {
      final text = lines[_at].trimLeft().substring(1);
      inner.add(text.startsWith(' ') ? text.substring(1) : text);
      _at++;
    }
    return MdQuote(_Blocks(inner).parse());
  }

  MdList _list(int indent) {
    final first = _item.firstMatch(lines[_at])!;
    final ordered = first.group(3) != null;
    final list = MdList(
      ordered: ordered,
      start: ordered ? int.parse(first.group(3)!) : 1,
    );
    while (_at < lines.length) {
      final line = lines[_at];
      final item = _item.firstMatch(line);
      if (item != null && item.group(1)!.length == indent) {
        if ((item.group(3) != null) != ordered) break;
        var text = item.group(4) ?? '';
        bool? checked;
        if (_task.firstMatch(text) case final task?) {
          checked = task.group(1) != ' ';
          text = text.substring(task.end);
        }
        list.items.add(MdListItem(text, checked: checked));
        _at++;
        continue;
      }
      if (item != null &&
          item.group(1)!.length > indent &&
          list.items.isNotEmpty) {
        list.items.last.children.add(_list(item.group(1)!.length));
        continue;
      }
      if (line.trim().isEmpty) {
        // A blank line ends the list unless the next line carries on inside it.
        final next = _at + 1 < lines.length ? lines[_at + 1] : '';
        final nextItem = _item.firstMatch(next);
        if (nextItem != null && nextItem.group(1)!.length >= indent ||
            next.startsWith(' ' * (indent + 2)) && next.trim().isNotEmpty) {
          _at++;
          continue;
        }
        break;
      }
      if (item == null &&
          list.items.isNotEmpty &&
          (line.startsWith(' ' * (indent + 2)) || !_startsBlock(line))) {
        final last = list.items.last;
        last.text = '${last.text} ${line.trim()}';
        _at++;
        continue;
      }
      break;
    }
    return list;
  }

  MdTable _table() {
    List<String> cells(String line) {
      var text = line.trim();
      if (text.startsWith('|')) text = text.substring(1);
      if (text.endsWith('|') && !text.endsWith(r'\|')) {
        text = text.substring(0, text.length - 1);
      }
      return [
        for (final cell in text.split(RegExp(r'(?<!\\)\|')))
          cell.trim().replaceAll(r'\|', '|'),
      ];
    }

    final header = cells(lines[_at]);
    final align = [
      for (final cell in cells(lines[_at + 1]))
        switch ((cell.startsWith(':'), cell.endsWith(':'))) {
          (true, true) => MdAlign.center,
          (false, true) => MdAlign.right,
          (true, false) => MdAlign.left,
          _ => null,
        },
    ];
    _at += 2;
    final rows = <List<String>>[];
    while (_at < lines.length &&
        lines[_at].contains('|') &&
        lines[_at].trim().isNotEmpty) {
      rows.add(cells(lines[_at]));
      _at++;
    }
    return MdTable(header, align, rows);
  }

  MdParagraph _paragraph() {
    final text = <String>[lines[_at].trim()];
    _at++;
    while (_at < lines.length && !_startsBlock(lines[_at])) {
      if (_at + 1 < lines.length &&
          lines[_at].contains('|') &&
          _delimiter.hasMatch(lines[_at + 1])) {
        break;
      }
      text.add(lines[_at].trim());
      _at++;
    }
    // A line break is kept, as tracker comments keep it: agents write one line per thought.
    return MdParagraph(text.join('\n'));
  }
}

/// A run of inline text with its marks; [link] is the target of a link run.
class MdRun {
  const MdRun(
    this.text, {
    this.bold = false,
    this.italic = false,
    this.strike = false,
    this.code = false,
    this.link,
  });

  final String text;
  final bool bold;
  final bool italic;
  final bool strike;
  final bool code;
  final String? link;

  MdRun marked({bool bold = false, bool italic = false, bool strike = false}) =>
      MdRun(
        text,
        bold: this.bold || bold,
        italic: this.italic || italic,
        strike: this.strike || strike,
        code: code,
        link: link,
      );
}

final _inlinePattern = RegExp(
  r'`([^`]+)`'
  r'|\[([^\]]+)\]\(<?([^)\s>]+)>?(?:\s+"[^"]*")?\)'
  r'|<(https?://[^>\s]+)>'
  r'|(https?://[^\s<>()\[\]]+)'
  r'|\*\*(.+?)\*\*'
  r'|__(.+?)__'
  r'|~~(.+?)~~'
  r'|(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])'
  r'|(?<!\w)_(?!\s)(.+?)(?<!\s)_(?!\w)',
);

/// The runs of [text]; with [markup] off only links are found.
List<MdRun> parseInline(String text, {bool markup = true}) {
  final runs = <MdRun>[];
  var at = 0;
  void plain(String piece) {
    if (piece.isEmpty) return;
    runs.add(
      MdRun(
        piece.replaceAllMapped(
          RegExp(r'\\([\\`*_{}\[\]()#+\-.!|~>])'),
          (m) => m.group(1)!,
        ),
      ),
    );
  }

  for (final match in _inlinePattern.allMatches(text)) {
    final isLink =
        match.group(2) != null ||
        match.group(4) != null ||
        match.group(5) != null;
    if (!markup && !isLink) continue;
    if (match.start > 0 && text[match.start - 1] == r'\') continue;
    plain(text.substring(at, match.start));
    if (match.group(1) case final code?) {
      runs.add(MdRun(code, code: true));
    } else if (match.group(2) case final label?) {
      runs.add(MdRun(label, link: match.group(3)));
    } else if (match.group(4) ?? match.group(5) case final url?) {
      final trimmed = url.replaceFirst(RegExp(r'[.,;:!?]+$'), '');
      runs.add(MdRun(trimmed, link: trimmed));
      plain(url.substring(trimmed.length));
    } else if (match.group(6) ?? match.group(7) case final inner?) {
      runs.addAll(parseInline(inner).map((r) => r.marked(bold: true)));
    } else if (match.group(8) case final inner?) {
      runs.addAll(parseInline(inner).map((r) => r.marked(strike: true)));
    } else if (match.group(9) ?? match.group(10) case final inner?) {
      runs.addAll(parseInline(inner).map((r) => r.marked(italic: true)));
    }
    at = match.end;
  }
  plain(text.substring(at));
  return runs;
}
