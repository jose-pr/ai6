import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:coqui_aos/coqui_aos.dart'
    as aos
    show Settings, Split, readFrontMatter, splitFrontMatter;
import 'package:coqui_acp_chat_ui/coqui_acp_chat_ui.dart' show AgentMarkdown;
import 'package:markdown/markdown.dart' as md;
import 'package:material_ui/material_ui.dart';

import '../assistant/chat_look.dart';
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
/// Mission, Operation or Run ID the reader may open.
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

  /// The frontmatter's text when it is there but is not a mapping the overlay specification reads.
  final String? unreadable;

  final String body;
}

/// Splits [markdown] into frontmatter and body by the Agent Overlay Specification's rule: a
/// first line of `---`, then everything to the next `---` or `...` line. The metadata is read as
/// the specification reads it, so a tag, an anchor, an alias or a merge key is refused and
/// nothing is expanded. A block that does not read is [SplitDocument.unreadable], kept as text;
/// text with no such block is all body.
SplitDocument splitFrontmatter(String markdown) {
  final data = Uint8List.fromList(
    utf8.encode(markdown.replaceAll('\r\n', '\n')),
  );
  final settings = aos.Settings();
  final aos.Split split;
  try {
    split = aos.splitFrontMatter(data, settings);
  } on Object {
    return SplitDocument(body: utf8.decode(data));
  }
  String body() => utf8.decode(split.body(data), allowMalformed: true);
  switch (split.splitCase) {
    case 'present':
      final metadata = utf8.decode(split.metadata!, allowMalformed: true);
      try {
        final read = aos.readFrontMatter(data, settings);
        if (read.frontMatterCase == 'mapping' && read.data != null) {
          return SplitDocument(
            frontmatter: _plain(read.data) as Map<String, Object?>,
            body: body(),
          );
        }
      } on Object {
        // Shown as written below.
      }
      return SplitDocument(unreadable: metadata, body: body());
    case 'empty':
      return SplitDocument(body: body());
    default:
      return SplitDocument(body: utf8.decode(data, allowMalformed: true));
  }
}

Object? _plain(Object? value) => switch (value) {
  final Map<Object?, Object?> map => <String, Object?>{
    for (final entry in map.entries) '${entry.key}': _plain(entry.value),
  },
  final List<Object?> list => [for (final item in list) _plain(item)],
  _ => value,
};

/// What a reference is called in a link's address, so the one tap handler knows it.
const _referenceScheme = 'oo-reference:';

/// [references] longest first, so the longer of two that start alike wins.
List<DocReference> _longestFirst(List<DocReference> references) =>
    List.of(references)..sort((a, b) => b.text.length.compareTo(a.text.length));

/// The characters Markdown lets a backslash escape, which a label written as plain text carries.
const _escapable = r'\`*_{}[]()#+-.!|~>';

/// [words] as a pattern that also matches them written with those characters escaped.
String _escapedPattern(String words) => [
  for (final char in words.split(''))
    _escapable.contains(char)
        ? '\\\\?${RegExp.escape(char)}'
        : RegExp.escape(char),
].join();

/// A backslash before a character Markdown lets it escape.
final _escape = RegExp(r'\\(?=[\\`*_{}\[\]()#+\-.!|~>])');

/// Matches the words of the document's references, longest first, at word edges, as written or
/// with Markdown's escapes in them, and makes each a link the document's one handler opens.
class _ReferenceSyntax extends md.InlineSyntax {
  _ReferenceSyntax(List<DocReference> sorted)
    : _sorted = sorted,
      super(
        '(?<![\\w/#-])(${sorted.map((r) => _escapedPattern(r.text)).join('|')})(?![\\w-])',
      );

  final List<DocReference> _sorted;

  @override
  bool onMatch(md.InlineParser parser, Match match) {
    final words = match[1]!.replaceAll(_escape, '');
    final index = _sorted.indexWhere((r) => r.text == words);
    if (index < 0) return false;
    parser.addNode(
      md.Element('a', [md.Text(words)])
        ..attributes['href'] = '$_referenceScheme$index',
    );
    return true;
  }
}

/// A Markdown document for a person to read: its frontmatter as a panel of fields, then the
/// body drawn by the shared chat's Markdown (`coqui_acp_chat_ui`'s `AgentMarkdown`), which draws
/// headings, paragraphs, lists, task boxes, quotes, code, tables and rules.
///
/// The text is untrusted. Nothing is fetched (no images) and no HTML is rendered; invisible
/// characters are shown. `app:/…` links and [references] open in the app through [onOpen]. A
/// web link opens in the browser: at once when its host is one of [trackers], after the reader
/// sees the full address otherwise, and never when [webLinks] is off. Other schemes are text.
class OoMarkdown extends StatelessWidget {
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
  Widget build(BuildContext context) {
    final split = splitFrontmatter(markdown);
    final tokens = OoTokens.of(context);
    final column = OoChatTheme(
      look: ooChatLook(tokens),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: 8,
        children: [
          if (split.frontmatter case final fields? when fields.isNotEmpty)
            _Frontmatter(
              fields: fields,
              value: (text) => _inline(context, text),
            ),
          if (split.unreadable case final yaml?) ...[
            Text(
              'Frontmatter not readable',
              style: Theme.of(context).textTheme.labelMedium,
            ),
            _code(tokens, yaml),
          ],
          _inline(context, split.body, soft: true),
        ],
      ),
    );
    return selectable ? SelectionArea(child: column) : column;
  }

  /// [text] as Markdown, with this document's references and links.
  Widget _inline(BuildContext context, String text, {bool soft = false}) =>
      AgentMarkdown(
        // The Markdown body parses again when its text or style changes, not its syntaxes; the
        // references arrive after the first frame, so a changed set builds the widget anew.
        key: ValueKey((references.length, trackers.length)),
        text: revealInvisible(text),
        inlineSyntaxes: [
          if (references.isNotEmpty)
            _ReferenceSyntax(_longestFirst(references)),
        ],
        onTapLink: (href, label) => _tapped(context, href, label),
        softLineBreak: soft,
      );

  Widget _code(OoTokens tokens, String text) => Container(
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

  void _tapped(BuildContext context, String href, String label) {
    if (href.startsWith(_referenceScheme)) {
      final index = int.tryParse(href.substring(_referenceScheme.length));
      final sorted = _longestFirst(references);
      if (index != null && index >= 0 && index < sorted.length) {
        _referenceOpener(context, sorted[index])?.call();
      }
      return;
    }
    _opener(context, href)?.call();
  }

  VoidCallback? _opener(BuildContext context, String target) {
    if (target.startsWith('app:/')) {
      final open = onOpen;
      return open == null ? null : () => open(target);
    }
    final url = Uri.tryParse(target);
    if (url == null || !const {'http', 'https'}.contains(url.scheme)) {
      return null;
    }
    if (!webLinks || url.host.isEmpty) return null;
    return () => unawaited(_openWeb(context, url));
  }

  VoidCallback? _referenceOpener(BuildContext context, DocReference reference) {
    final route = reference.route;
    final open = onOpen;
    final webs = webLinks
        ? [...reference.issues, ...reference.boards]
        : const <DocLink>[];
    if (webs.isEmpty) {
      if (route != null && open != null) return () => open(route);
      if (reference.url case final url? when webLinks) {
        return () => unawaited(_openWeb(context, url));
      }
      return null;
    }
    return () => unawaited(_referenceMenu(context, reference));
  }

  Future<void> _referenceMenu(
    BuildContext context,
    DocReference reference,
  ) async {
    final route = reference.route;
    final open = onOpen;
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
    if (!trackers.contains(url.host.toLowerCase())) {
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
    await launcher(url);
  }
}

/// The frontmatter's fields, one row each; a list is its items and a mapping is indented.
class _Frontmatter extends StatelessWidget {
  const _Frontmatter({required this.fields, required this.value});

  final Map<String, Object?> fields;

  /// A value as the document draws text: links and references work in it.
  final Widget Function(String text) value;

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
                Expanded(child: _value(field)),
              ],
            ),
          ),
    ];
  }

  Widget _value(Object? field) => switch (field) {
    final List<Object?> items => Wrap(
      spacing: 6,
      runSpacing: 4,
      children: [for (final item in items) value(_scalar(item))]
          .separatedBy(const Text('·')),
    ),
    _ => value(_scalar(field)),
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
