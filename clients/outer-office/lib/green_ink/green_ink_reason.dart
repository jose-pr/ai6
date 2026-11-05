import 'package:material_ui/material_ui.dart';

import '../assistant/pages.dart' show PageLauncher, launchPage;
import '../shell/oo_markdown.dart';

/// An address an Agent gave with a request.
@immutable
class GreenInkLink {
  const GreenInkLink({required this.label, required this.url});

  final String label;
  final String url;

  factory GreenInkLink.fromJson(Map<String, dynamic> json) =>
      GreenInkLink(label: json['label'] as String, url: json['url'] as String);
}

/// Where a person reads one of an Operation's Sources.
@immutable
class SourcePageSummary {
  const SourcePageSummary({
    required this.provider,
    required this.web,
    this.project,
  });

  /// `github`, `gitlab` or `jira`.
  final String provider;

  /// The repository's or project's page, or a Jira site's root.
  final String web;

  /// A Jira project's key.
  final String? project;

  factory SourcePageSummary.fromJson(Map<String, dynamic> json) =>
      SourcePageSummary(
        provider: json['provider'] as String,
        web: json['web'] as String,
        project: json['project'] as String?,
      );

  bool get _isRepository => provider == 'github' || provider == 'gitlab';

  /// `owner/name`, as a reference names a repository.
  String get _repository => Uri.parse(web).path.replaceFirst('/', '');

  /// GitLab keeps a project's own pages under `/-/`.
  String _page(String tail) =>
      provider == 'gitlab' ? '$web/-/$tail' : '$web/$tail';
}

final _sha = RegExp(r'\b(?=[0-9a-f]*[0-9])(?=[0-9a-f]*[a-f])[0-9a-f]{7,40}\b');
final _issue = RegExp(r'(?<![\w/#-])#(\d+)\b');
final _repositoryIssue = RegExp(r'(?<![\w/#-])([\w.-]+(?:/[\w.-]+)+)#(\d+)\b');
final _fullRef = RegExp(r'\brefs/heads/([\w./-]*\w)');
final _namedBranch = RegExp(r'\bbranch\s+([\w./-]*\w)');

/// What [text] names that one of [pages] holds: Jira keys of a project they list, issues by
/// `#12` or `owner/name#12`, commit SHAs and branch names (`refs/heads/x`, `branch x`). A bare
/// number or SHA goes to the first repository. Nothing is asked of the tracker, so a reference may
/// lead to a page that does not exist; it never leads anywhere but [pages].
List<DocReference> referencesIn(String text, List<SourcePageSummary> pages) {
  final found = <String, Uri>{};
  void add(String words, String address) =>
      found.putIfAbsent(words, () => Uri.parse(address));
  for (final page in pages) {
    if (page.provider == 'jira' && page.project != null) {
      final key = RegExp('\\b${RegExp.escape(page.project!)}-\\d+\\b');
      for (final match in key.allMatches(text)) {
        add(match[0]!, '${page.web}/browse/${match[0]}');
      }
    }
  }
  final repositories = pages.where((page) => page._isRepository).toList();
  for (final match in _repositoryIssue.allMatches(text)) {
    for (final page in repositories) {
      if (page._repository == match[1]) {
        add(match[0]!, page._page('issues/${match[2]}'));
      }
    }
  }
  if (repositories.firstOrNull case final first?) {
    for (final match in _issue.allMatches(text)) {
      add(match[0]!, first._page('issues/${match[1]}'));
    }
    for (final match in _sha.allMatches(text)) {
      add(match[0]!, first._page('commit/${match[0]}'));
    }
    for (final match in _fullRef.allMatches(text)) {
      add(match[0]!, first._page('tree/${match[1]}'));
    }
    for (final match in _namedBranch.allMatches(text)) {
      add(match[1]!, first._page('tree/${match[1]}'));
    }
  }
  return [
    for (final MapEntry(key: words, value: url) in found.entries)
      DocReference(text: words, url: url),
  ];
}

/// Why an Agent asks, as a person reads it: Markdown whose references lead to the Operation's
/// Sources, then the links it gave. All of it is the Agent's: a link to anywhere but a Source shows
/// its address before it opens.
class GreenInkReason extends StatelessWidget {
  const GreenInkReason({
    required this.reason,
    this.links = const [],
    this.sources,
    this.launcher = launchPage,
    super.key,
  });

  final String reason;
  final List<GreenInkLink> links;

  /// The Operation's Sources, when they are being read.
  final Future<List<SourcePageSummary>>? sources;
  final PageLauncher launcher;

  /// A label as Markdown that reads as itself, whatever it holds.
  static String _plain(String label) => label.replaceAllMapped(
    RegExp(r'[\\`*_{}\[\]()#+\-.!|~>]'),
    (match) => '\\${match[0]}',
  );

  @override
  Widget build(BuildContext context) => FutureBuilder<List<SourcePageSummary>>(
    future: sources,
    builder: (context, snapshot) {
      final pages = snapshot.data ?? const <SourcePageSummary>[];
      final hosts = {
        for (final page in pages) Uri.parse(page.web).host.toLowerCase(),
      };
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: 4,
        children: [
          OoMarkdown(
            reason,
            references: referencesIn(reason, pages),
            trackers: hosts,
            launcher: launcher,
          ),
          for (final link in links)
            OoMarkdown(
              _plain(link.label),
              references: [
                DocReference(text: link.label, url: Uri.tryParse(link.url)),
              ],
              trackers: hosts,
              launcher: launcher,
            ),
        ],
      );
    },
  );
}
