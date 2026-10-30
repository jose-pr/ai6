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
  /// A link to page [id]; with [network] `none`, one whose page may reach nothing outside its
  /// own files.
  Future<PageLink> open(String id, {String? network});

  /// Keeps the page with no expiry, or lets it expire in a day.
  Future<void> remember(String id, {required bool remember});

  Future<void> delete(String id);
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

/// A page she published: its kind, title, size and expiry (or "Remembered"), what she read
/// when it was made after outside text, and the feeds it reads. **Open** launches it outside
/// the app; the menu opens it with the network off, remembers or forgets it, and deletes it.
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

  /// The entry's `data`: `{id, kind, title, bytes, expires_at, exposed, exposed_by, feeds}`.
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
    final sources = [
      for (final source in (_data['exposed_by'] as List?) ?? const [])
        '$source',
    ];
    return Card(
      key: ValueKey('page-${_data['id']}'),
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 6,
          children: [
            if (_data['exposed'] == true)
              Container(
                key: const ValueKey('page-exposed'),
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: Color.alphaBlend(
                    tokens.warning.withValues(alpha: 0.14),
                    tokens.raised,
                  ),
                  border: Border(
                    left: BorderSide(color: tokens.warning, width: 3),
                  ),
                ),
                child: Text(
                  revealInvisible(
                    'Made after she read text from outside AI6'
                    '${sources.isEmpty ? '' : ': ${sources.take(5).join(', ')}'}'
                    '${sources.length > 5 ? ' and ${sources.length - 5} more' : ''}',
                  ),
                  style: TextStyle(color: tokens.text),
                ),
              ),
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
                    PopupMenuItem(
                      value: remembered ? 'forget' : 'remember',
                      child: Text(remembered ? 'Forget' : 'Remember'),
                    ),
                    const PopupMenuItem(value: 'delete', child: Text('Delete')),
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
