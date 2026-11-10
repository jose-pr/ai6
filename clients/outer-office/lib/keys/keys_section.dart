import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../shell/oo_freshness.dart';
import '../theme/oo_theme.dart';

/// A key the gateway lists. It never carries the key's value.
class KeyView {
  const KeyView({
    required this.id,
    required this.name,
    required this.scopes,
    required this.createdAt,
    required this.expiresAt,
    required this.lastUsedAt,
    required this.ownerId,
    required this.ownerName,
  });

  final String id;
  final String name;

  /// `ACTION@SCOPE`, such as `config.manage@config:plugin`.
  final List<String> scopes;
  final double createdAt;
  final double? expiresAt;
  final double? lastUsedAt;
  final String ownerId;
  final String ownerName;

  factory KeyView.fromJson(Map<String, dynamic> json) => KeyView(
    id: json['id'] as String,
    name: json['name'] as String,
    scopes: (json['scopes'] as List<dynamic>).cast<String>(),
    createdAt: (json['created_at'] as num).toDouble(),
    expiresAt: (json['expires_at'] as num?)?.toDouble(),
    lastUsedAt: (json['last_used_at'] as num?)?.toDouble(),
    ownerId: json['owner_id'] as String? ?? '',
    ownerName: json['owner_name'] as String? ?? '',
  );
}

/// A change to the keys the gateway refused, in its words.
class KeysRefused implements Exception {
  const KeysRefused(this.reason);

  final String reason;

  @override
  String toString() => reason;
}

/// The signed-in person's keys, through `/v1/user/keys`.
class KeysApi {
  const KeysApi(this.board);

  final MissionBoard board;

  KeysRefused _refused(http.Response response) {
    var reason = 'The gateway refused it (HTTP ${response.statusCode}).';
    try {
      final body = jsonDecode(response.body);
      if (body is Map && body['error'] is String) {
        reason = body['error'] as String;
      }
    } on FormatException {
      // The gateway's plain refusals have no body.
    }
    return KeysRefused(reason);
  }

  Map<String, dynamic> _ok(http.Response response) {
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw _refused(response);
    }
    return response.body.isEmpty
        ? const {}
        : jsonDecode(response.body) as Map<String, dynamic>;
  }

  Future<List<KeyView>> list({bool all = false}) async {
    final response = await board.accessRequest(
      'GET',
      all ? '/v1/user/keys?all=true' : '/v1/user/keys',
    );
    if (response.statusCode != 200) throw _refused(response);
    return [
      for (final key in jsonDecode(response.body) as List<dynamic>)
        KeyView.fromJson(key as Map<String, dynamic>),
    ];
  }

  /// The new key's secret, for the caller to show once and keep nowhere.
  Future<String> create({
    required String name,
    required List<String> scopes,
    int? expiresIn,
  }) async {
    final answer = _ok(
      await board.accessRequest(
        'POST',
        '/v1/user/keys',
        body: {'name': name, 'scopes': scopes, 'expires_in': ?expiresIn},
      ),
    );
    return answer['secret'] as String;
  }

  Future<void> revoke(String id) async {
    _ok(
      await board.accessRequest(
        'DELETE',
        '/v1/user/keys/${Uri.encodeComponent(id)}',
      ),
    );
  }
}

/// Seconds for a lifetime typed as a number and a unit (`30d`, `12h`, `90m`, `45s`), or `0`
/// for `never`; `null` for blank (the gateway's default). Throws [FormatException] otherwise.
int? parseKeyLifetime(String text) {
  final value = text.trim().toLowerCase();
  if (value.isEmpty) return null;
  if (value == 'never') return 0;
  final match = RegExp(r'^(\d+)\s*([smhd])$').firstMatch(value);
  if (match == null) {
    throw const FormatException('Use 30d, 12h, 90m, 45s or never.');
  }
  final unit = const {'s': 1, 'm': 60, 'h': 3600, 'd': 86400}[match.group(2)]!;
  return int.parse(match.group(1)!) * unit;
}

/// One `ACTION@SCOPE` per line (or comma separated), blanks dropped.
List<String> parseKeyScopes(String text) => [
  for (final part in text.split(RegExp(r'[\n,]')))
    if (part.trim().isNotEmpty) part.trim(),
];

/// The Keys section of the person's own profile: their keys, a form for a new one and a
/// revoke per row. A new key's secret is held only by the dialog that shows it.
class KeysSection extends StatefulWidget {
  const KeysSection({required this.board, super.key});

  final MissionBoard board;

  @override
  State<KeysSection> createState() => _KeysSectionState();
}

class _KeysSectionState extends State<KeysSection> {
  List<KeyView>? _keys;
  String? _error;
  String? _notice;
  bool _all = false;

  KeysApi get _api => KeysApi(widget.board);

  bool get _canSeeAll => widget.board.globalActions.contains('account.manage');

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final keys = await _api.list(all: _all);
      if (mounted) {
        setState(() {
          _keys = keys;
          _error = null;
        });
      }
    } on KeysRefused catch (error) {
      if (mounted) setState(() => _error = error.reason);
    } on Object {
      if (mounted) setState(() => _error = 'Keys are unavailable.');
    }
  }

  Future<void> _create() async {
    final secret = await showDialog<String>(
      context: context,
      builder: (context) => _CreateKeyDialog(api: _api),
    );
    if (secret == null || !mounted) return;
    await showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (context) => _SecretDialog(secret: secret),
    );
    await _load();
  }

  Future<void> _revoke(KeyView key) async {
    final sure = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Revoke ${key.name}?'),
        content: const Text(
          'Anything using this key stops working at once. '
          'This cannot be undone.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            style: ooDangerButton(OoTokens.of(context)),
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Revoke'),
          ),
        ],
      ),
    );
    if (sure != true) return;
    try {
      await _api.revoke(key.id);
      if (mounted) setState(() => _notice = null);
    } on KeysRefused catch (error) {
      if (mounted) setState(() => _notice = error.reason);
      return;
    }
    await _load();
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final muted = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: tokens.muted);
    final keys = _keys;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Semantics(
          header: true,
          child: Text('Keys', style: Theme.of(context).textTheme.titleSmall),
        ),
        const SizedBox(height: 4),
        Text(
          'A key signs in a script or a tool with only the scopes you give it.',
          style: muted,
        ),
        if (_canSeeAll)
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            dense: true,
            title: const Text('All keys'),
            value: _all,
            onChanged: (value) {
              setState(() {
                _all = value;
                _keys = null;
              });
              unawaited(_load());
            },
          ),
        if (_notice != null)
          Semantics(
            liveRegion: true,
            child: Text(_notice!, style: TextStyle(color: tokens.danger)),
          ),
        if (_error != null)
          Semantics(
            liveRegion: true,
            child: Text(_error!, style: TextStyle(color: tokens.danger)),
          )
        else if (keys == null)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 8),
            child: LinearProgressIndicator(),
          )
        else if (keys.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Text('No keys yet.', style: muted),
          )
        else
          for (final key in keys)
            _KeyRow(
              key: ValueKey(key.id),
              view: key,
              showOwner: _all,
              onRevoke: () => _revoke(key),
            ),
        const SizedBox(height: 8),
        Align(
          alignment: Alignment.centerLeft,
          child: OutlinedButton.icon(
            icon: const Icon(Icons.add),
            label: const Text('Create key'),
            onPressed: _create,
          ),
        ),
      ],
    );
  }
}

class _KeyRow extends StatelessWidget {
  const _KeyRow({
    required this.view,
    required this.showOwner,
    required this.onRevoke,
    super.key,
  });

  final KeyView view;
  final bool showOwner;
  final VoidCallback onRevoke;

  @override
  Widget build(BuildContext context) {
    final muted = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: OoTokens.of(context).muted);
    final expires = view.expiresAt;
    final used = view.lastUsedAt;
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(view.name),
                if (showOwner) Text('Owner: ${view.ownerName}', style: muted),
                for (final scope in view.scopes) Text(scope, style: muted),
                OoTime(view.createdAt, prefix: 'Created '),
                expires == null
                    ? Text('Expires: never', style: muted)
                    : OoTime(expires, prefix: 'Expires '),
                used == null
                    ? Text('Last used: never used', style: muted)
                    : OoTime(used, prefix: 'Last used '),
              ],
            ),
          ),
          IconButton(
            tooltip: 'Revoke ${view.name}',
            icon: const Icon(Icons.delete_outline),
            onPressed: onRevoke,
          ),
        ],
      ),
    );
  }
}

/// Asks for a name, scopes and a lifetime, creates the key, and returns its secret.
class _CreateKeyDialog extends StatefulWidget {
  const _CreateKeyDialog({required this.api});

  final KeysApi api;

  @override
  State<_CreateKeyDialog> createState() => _CreateKeyDialogState();
}

class _CreateKeyDialogState extends State<_CreateKeyDialog> {
  final _name = TextEditingController();
  final _scopes = TextEditingController();
  final _lifetime = TextEditingController();
  String? _error;
  bool _busy = false;

  @override
  void dispose() {
    _name.dispose();
    _scopes.dispose();
    _lifetime.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    final int? lifetime;
    try {
      lifetime = parseKeyLifetime(_lifetime.text);
    } on FormatException catch (error) {
      setState(() => _error = error.message);
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final secret = await widget.api.create(
        name: _name.text.trim(),
        scopes: parseKeyScopes(_scopes.text),
        expiresIn: lifetime,
      );
      if (mounted) Navigator.pop(context, secret);
    } on KeysRefused catch (error) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = error.reason;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Create key'),
    content: SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          TextField(
            controller: _name,
            decoration: const InputDecoration(labelText: 'Key name'),
          ),
          TextField(
            controller: _scopes,
            minLines: 2,
            maxLines: 5,
            decoration: const InputDecoration(
              labelText: 'Scopes, one ACTION@SCOPE per line',
              hintText: 'config.manage@config:plugin',
            ),
          ),
          TextField(
            controller: _lifetime,
            decoration: const InputDecoration(
              labelText: 'Lasts (30d, 12h, 90m, never; blank for the default)',
            ),
          ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Semantics(
                liveRegion: true,
                child: Text(
                  _error!,
                  style: TextStyle(color: OoTokens.of(context).danger),
                ),
              ),
            ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: _busy ? null : () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: _busy ? null : _submit,
        child: const Text('Create'),
      ),
    ],
  );
}

/// Shows a new key's secret once. Only this dialog holds it; closing it drops it.
class _SecretDialog extends StatelessWidget {
  const _SecretDialog({required this.secret});

  final String secret;

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Your new key'),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text(
          'Shown once, not stored. Copy it now; it cannot be shown again.',
        ),
        const SizedBox(height: 12),
        SelectableText(secret),
      ],
    ),
    actions: [
      TextButton(
        onPressed: () => Clipboard.setData(ClipboardData(text: secret)),
        child: const Text('Copy'),
      ),
      FilledButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Done'),
      ),
    ],
  );
}
