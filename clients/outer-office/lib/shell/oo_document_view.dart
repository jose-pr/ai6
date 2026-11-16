import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../assistant/pages.dart' show PageLauncher, launchPage;
import '../routes/oo_route.dart';
import '../theme/oo_tokens.dart';
import 'oo_link.dart';
import 'oo_markdown.dart';

/// A document Universal Exports serves, read when shown: its frontmatter panel and rendered
/// body from [OoMarkdown], and an action that copies the whole text as Markdown.
///
/// A link into the app opens where the person may open that page. When the read fails the reason
/// is shown, and [fallback], if given, as Markdown beneath it.
class OoDocumentView extends StatefulWidget {
  const OoDocumentView({
    required this.load,
    this.fallback,
    this.launcher = launchPage,
    super.key,
  });

  final Future<OoDocument> Function() load;

  /// Markdown to show when the document cannot be read.
  final String? fallback;
  final PageLauncher launcher;

  @override
  State<OoDocumentView> createState() => _OoDocumentViewState();
}

class _OoDocumentViewState extends State<OoDocumentView> {
  late Future<OoDocument> _document = widget.load();

  void _open(String link) {
    final OoRoute route;
    try {
      route = OoRoute.parseUri(link);
    } on FormatException {
      return;
    }
    OoLinks.opener(context, route)?.call();
  }

  Future<void> _copy(OoDocument document) async {
    final messenger = ScaffoldMessenger.maybeOf(context);
    await Clipboard.setData(ClipboardData(text: document.markdown));
    messenger?.showSnackBar(
      const SnackBar(content: Text('Copied as Markdown')),
    );
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<OoDocument>(
    future: _document,
    builder: (context, snapshot) {
      if (snapshot.hasError) {
        final error = snapshot.error;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          spacing: 8,
          children: [
            Text(
              'The document could not be read: '
              '${error is StateError ? error.message : error}',
              style: TextStyle(color: OoTokens.of(context).danger),
            ),
            TextButton.icon(
              onPressed: () => setState(() => _document = widget.load()),
              icon: const Icon(Icons.refresh),
              label: const Text('Try again'),
            ),
            if (widget.fallback case final text?)
              OoMarkdown(text, launcher: widget.launcher, onOpen: _open),
          ],
        );
      }
      final document = snapshot.data;
      if (document == null) return const LinearProgressIndicator();
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Align(
            alignment: Alignment.centerRight,
            child: TextButton.icon(
              onPressed: () => _copy(document),
              icon: const Icon(Icons.copy, size: 16),
              label: const Text('Copy as Markdown'),
            ),
          ),
          OoMarkdown.document(
            document,
            launcher: widget.launcher,
            onOpen: _open,
          ),
        ],
      );
    },
  );
}

/// The first [lines] lines of [markdown], with any code fence the cut left open closed; `null`
/// when the text is no longer than that.
String? markdownPreview(String markdown, int lines) {
  final all = markdown.trimRight().split('\n');
  if (all.length <= lines) return null;
  final kept = all.take(lines).toList();
  final fences = kept.where((line) {
    final trimmed = line.trimLeft();
    return trimmed.startsWith('```') || trimmed.startsWith('~~~');
  }).length;
  if (fences.isOdd) kept.add('```');
  return kept.join('\n');
}

/// Opens [load]'s document on a page of its own, keeping this page's links.
Future<void> openDocumentPage(
  BuildContext context, {
  required String title,
  required Future<OoDocument> Function() load,
  String? fallback,
}) => Navigator.of(context).push(
  MaterialPageRoute<void>(
    builder: (_) => OoLinks.carry(
      context,
      Scaffold(
        appBar: AppBar(title: Text(title)),
        body: ListView(
          padding: const EdgeInsets.all(24),
          children: [OoDocumentView(load: load, fallback: fallback)],
        ),
      ),
    ),
  ),
);
