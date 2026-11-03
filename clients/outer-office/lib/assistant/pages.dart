import 'dart:async';

import 'package:material_ui/material_ui.dart';
import 'package:url_launcher/url_launcher.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../shell/oo_freshness.dart';
import '../theme/oo_tokens.dart';
import 'entry_kind.dart';

/// A link to a page: where to open it and when it stops working.
class PageLink {
  const PageLink(this.url, this.expiresAt);

  final Uri url;

  /// Unix seconds.
  final double expiresAt;
}

/// What the app asks of Universal Exports about the pages Moneypenny published.
abstract interface class PageActions {
  /// The person's own pages, newest first, then the pages shared with them (each naming whose
  /// it is in `shared_by`): `{id, kind, title, bytes, expires_at, feeds}` and, for their own,
  /// `shares`.
  Future<List<Map<String, Object?>>> list();

  /// A link to page [id]; with [network] `none`, one whose page may reach nothing outside its
  /// own files.
  Future<PageLink> open(String id, {String? network});

  /// Keeps the page with no expiry, or lets it expire in a day.
  Future<void> remember(String id, {required bool remember});

  Future<void> delete(String id);

  /// Shares page [id] with [subjects] in place of whom it was shared with, and answers whom it
  /// is now shared with. A refusal throws with the gateway's reason.
  Future<List<String>> share(String id, List<String> subjects);
}

/// How a link is launched: a new browser tab on the web, the system browser on a desktop,
/// never a webview in the app.
typedef PageLauncher = Future<bool> Function(Uri url);

/// Launches [url] outside the app, inside the click that asked.
Future<bool> launchPage(Uri url) => launchUrl(
  url,
  mode: LaunchMode.externalApplication,
  webOnlyWindowName: '_blank',
);

/// A page she published: its kind, title, size and expiry (or "Remembered"), the feeds it
/// reads and whom it is shared with. **Open** launches it outside the app; the menu opens it
/// with the network off, remembers or forgets it, shares it and deletes it. A page someone
/// shared with the person says whose it is, and its menu only opens it.
///
/// The link is minted when the card is shown and again before it ends, so a click launches at
/// once, within the click's activation, where a browser lets a window open.
class PageCard extends StatefulWidget {
  const PageCard({
    required this.data,
    required this.actions,
    this.launcher = launchPage,
    super.key,
  });

  /// The entry's `data`: `{id, kind, title, bytes, expires_at, feeds}`, with `shares` on the
  /// person's own page and `shared_by` on one shared with them.
  final Map<String, Object?>? data;
  final PageActions actions;
  final PageLauncher launcher;

  @override
  State<PageCard> createState() => _PageCardState();
}

class _PageCardState extends State<PageCard> {
  PageLink? _link;
  Object? _failed;
  Timer? _renew;
  bool? _remembered;
  List<String>? _shares;
  bool _deleted = false;

  Map<String, Object?> get _data => widget.data ?? const {};
  String get _id => _data['id'] as String? ?? '';

  @override
  void initState() {
    super.initState();
    unawaited(_mint());
  }

  @override
  void dispose() {
    _renew?.cancel();
    super.dispose();
  }

  Future<void> _mint() async {
    _renew?.cancel();
    try {
      final link = await widget.actions.open(_id);
      if (!mounted) return;
      setState(() {
        _link = link;
        _failed = null;
      });
      final left =
          link.expiresAt - DateTime.now().millisecondsSinceEpoch / 1000 - 60;
      if (left > 0) {
        _renew = Timer(
          Duration(seconds: left.round()),
          () => unawaited(_mint()),
        );
      }
    } on Object catch (error) {
      if (mounted) setState(() => _failed = error);
    }
  }

  Future<void> _open({String? network}) async {
    final ready = _link;
    if (network == null && ready != null) {
      await widget.launcher(ready.url);
      return;
    }
    // A link of another profile is minted on demand: it launches at the end of the click's
    // turn, which a browser may take for a pop-up, so the plain Open stays the quick path.
    final link = await widget.actions.open(_id, network: network);
    await widget.launcher(link.url);
  }

  Future<void> _share() async {
    final shares = await showDialog<List<String>>(
      context: context,
      builder: (context) => ShareDialog(
        id: _id,
        kind: _data['kind'] as String? ?? 'file',
        shares: _currentShares,
        actions: widget.actions,
      ),
    );
    if (shares != null && mounted) setState(() => _shares = shares);
  }

  List<String> get _currentShares =>
      _shares ??
      [for (final share in (_data['shares'] as List?) ?? const []) '$share'];

  String _size() {
    final bytes = (_data['bytes'] as num?)?.toInt() ?? 0;
    if (bytes < 1024) return '$bytes bytes';
    if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KiB';
    return '${(bytes / 1024 / 1024).toStringAsFixed(1)} MiB';
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tokens = OoTokens.of(context);
    if (_deleted) {
      return Card(
        margin: EdgeInsets.zero,
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Text('Page deleted.', style: theme.textTheme.bodySmall),
        ),
      );
    }
    final kind = _data['kind'] as String? ?? 'file';
    final expires = (_data['expires_at'] as num?)?.toDouble();
    final remembered = _remembered ?? expires == null;
    final feeds = [
      for (final feed in (_data['feeds'] as List?) ?? const []) '$feed',
    ];
    final sharedBy = _data['shared_by'] as String?;
    final shares = _currentShares;
    return Card(
      key: ValueKey('page-${_data['id']}'),
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 6,
          children: [
            Row(
              spacing: 8,
              children: [
                const EntryKindIcon(EntryKind.pageLink),
                Expanded(
                  child: Text(
                    revealInvisible(_data['title'] as String? ?? ''),
                    style: theme.textTheme.titleMedium,
                  ),
                ),
                FilledButton(
                  onPressed: _id.isEmpty ? null : () => unawaited(_open()),
                  child: const Text('Open'),
                ),
                PopupMenuButton<String>(
                  tooltip: 'More',
                  onSelected: (choice) async {
                    switch (choice) {
                      case 'none':
                        await _open(network: 'none');
                      case 'share':
                        await _share();
                      case 'remember':
                        await widget.actions.remember(_id, remember: true);
                        if (mounted) setState(() => _remembered = true);
                      case 'forget':
                        await widget.actions.remember(_id, remember: false);
                        if (mounted) setState(() => _remembered = false);
                      case 'delete':
                        await widget.actions.delete(_id);
                        if (mounted) setState(() => _deleted = true);
                    }
                  },
                  itemBuilder: (context) => [
                    const PopupMenuItem(
                      value: 'none',
                      child: Text('Open with network off'),
                    ),
                    if (sharedBy == null) ...[
                      PopupMenuItem(
                        value: remembered ? 'forget' : 'remember',
                        child: Text(remembered ? 'Forget' : 'Remember'),
                      ),
                      const PopupMenuItem(
                        value: 'share',
                        child: Text('Share…'),
                      ),
                      const PopupMenuItem(
                        value: 'delete',
                        child: Text('Delete'),
                      ),
                    ],
                  ],
                ),
              ],
            ),
            Wrap(
              spacing: 12,
              children: [
                Text(kind, style: theme.textTheme.labelMedium),
                Text(_size(), style: theme.textTheme.bodySmall),
                if (remembered)
                  Text('Remembered', style: theme.textTheme.bodySmall)
                else if (expires != null)
                  OoTime(expires, prefix: 'Expires '),
              ],
            ),
            if (feeds.isNotEmpty)
              Text(
                'Reads live: ${feeds.join(', ')}',
                style: theme.textTheme.bodySmall,
              ),
            if (sharedBy != null)
              Text(
                'Shared by ${revealInvisible(sharedBy)}',
                style: theme.textTheme.bodySmall,
              )
            else if (shares.isNotEmpty)
              Text(
                'Shared with ${shares.map(revealInvisible).join(', ')}',
                style: theme.textTheme.bodySmall,
              ),
            if (_failed != null)
              Text(
                'The link could not be made; open it again later.',
                style: theme.textTheme.bodySmall,
              ),
            Text(
              'Opens in a new tab or your browser, outside Outer Office.',
              style: theme.textTheme.bodySmall?.copyWith(color: tokens.muted),
            ),
          ],
        ),
      ),
    );
  }
}

/// Whom a page is shared with, one subject a line: `person:`, `group:` or `account:` and an
/// ID, `compartment:` and one the person is read into, or `operation:` and one where they may
/// read Missions. Save sends the whole list; an empty one shares the page with nobody.
class ShareDialog extends StatefulWidget {
  const ShareDialog({
    required this.id,
    required this.kind,
    required this.shares,
    required this.actions,
    super.key,
  });

  final String id;

  /// The page's kind: a report says it shows what its author could read.
  final String kind;
  final List<String> shares;
  final PageActions actions;

  @override
  State<ShareDialog> createState() => _ShareDialogState();
}

class _ShareDialogState extends State<ShareDialog> {
  late final _text = TextEditingController(text: widget.shares.join('\n'));
  String? _refused;
  bool _saving = false;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final subjects = [
      for (final line in _text.text.split('\n'))
        if (line.trim().isNotEmpty) line.trim(),
    ];
    setState(() {
      _saving = true;
      _refused = null;
    });
    try {
      final shares = await widget.actions.share(widget.id, subjects);
      if (mounted) Navigator.of(context).pop(shares);
    } on Object catch (error) {
      if (mounted) {
        setState(() {
          _refused = '$error';
          _saving = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AlertDialog(
      title: const Text('Share this page'),
      content: SizedBox(
        width: 480,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 8,
          children: [
            const Text(
              'One a line: person:ID, group:NAME, account:NAME, compartment:ID (one you '
              'are read into) or operation:ID (one where you can read Missions). Leave it '
              'empty to share with nobody.',
            ),
            TextField(
              key: const ValueKey('share-subjects'),
              controller: _text,
              minLines: 3,
              maxLines: 8,
              decoration: const InputDecoration(
                border: OutlineInputBorder(),
                labelText: 'Shared with',
              ),
            ),
            Text(
              widget.kind == 'report'
                  ? 'A report shows what you could read when it was made, to everyone '
                        'you share it with.'
                  : "Each viewer's live data is read with their own permissions.",
              style: theme.textTheme.bodySmall,
            ),
            if (_refused case final refused?)
              Text(
                refused,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.error,
                ),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: _saving ? null : () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: _saving ? null : () => unawaited(_save()),
          child: const Text('Save'),
        ),
      ],
    );
  }
}

/// Her pages: the person's own, then those shared with them, each as a [PageCard].
class HerPagesPage extends StatefulWidget {
  const HerPagesPage({
    required this.actions,
    this.launcher = launchPage,
    super.key,
  });

  final PageActions actions;
  final PageLauncher launcher;

  @override
  State<HerPagesPage> createState() => _HerPagesPageState();
}

class _HerPagesPageState extends State<HerPagesPage> {
  List<Map<String, Object?>>? _pages;
  Object? _failed;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final pages = await widget.actions.list();
      if (mounted) {
        setState(() {
          _pages = pages;
          _failed = null;
        });
      }
    } on Object catch (error) {
      if (mounted) setState(() => _failed = error);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final pages = _pages ?? const <Map<String, Object?>>[];
    final own = [
      for (final page in pages)
        if (page['shared_by'] == null) page,
    ];
    final shared = [
      for (final page in pages)
        if (page['shared_by'] != null) page,
    ];
    Widget card(Map<String, Object?> page) => Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: PageCard(
        key: ValueKey('her-page-${page['id']}'),
        data: page,
        actions: widget.actions,
        launcher: widget.launcher,
      ),
    );
    return Scaffold(
      appBar: AppBar(
        title: const Text('Her pages'),
        actions: [
          IconButton(
            tooltip: 'Refresh',
            onPressed: () => unawaited(_load()),
            icon: const Icon(Icons.refresh),
          ),
        ],
      ),
      body: _pages == null
          ? Center(
              child: _failed == null
                  ? const CircularProgressIndicator()
                  : const Text('Her pages could not be read; try again.'),
            )
          : ListView(
              padding: const EdgeInsets.all(16),
              children: [
                Text('Your pages', style: theme.textTheme.titleMedium),
                const SizedBox(height: 8),
                if (own.isEmpty)
                  const Text('She has published no page for you.')
                else
                  for (final page in own) card(page),
                const SizedBox(height: 16),
                Text('Shared with you', style: theme.textTheme.titleMedium),
                const SizedBox(height: 8),
                if (shared.isEmpty)
                  const Text('Nobody has shared a page with you.')
                else
                  for (final page in shared) card(page),
              ],
            ),
    );
  }
}
