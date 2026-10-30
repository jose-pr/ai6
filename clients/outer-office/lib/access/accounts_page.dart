import 'dart:async';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../main.dart';
import 'access_api.dart';

/// The accounts people sign in with: disable or enable, groups (only with `access.manage`)
/// and a password reset, whose new password is shown once in a dialog and kept nowhere.
class AccountsTab extends StatefulWidget {
  const AccountsTab({required this.board, required this.api, super.key});

  final MissionBoard board;
  final AccessApi api;

  @override
  State<AccountsTab> createState() => _AccountsTabState();
}

class _AccountsTabState extends State<AccountsTab> {
  List<AccountEntry>? _accounts;
  String? _error;

  bool get _canGroups => widget.board.globalActions.contains('access.manage');

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final accounts = await widget.api.accounts();
      if (mounted) {
        setState(() {
          _accounts = accounts;
          _error = null;
        });
      }
    } on AccessRefused catch (error) {
      if (mounted) setState(() => _error = error.reason);
    }
  }

  Future<void> _run(Future<void> Function() change) async {
    try {
      await change();
      await _load();
    } on AccessRefused catch (error) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(error.reason)));
    }
  }

  Future<void> _create() async {
    final name = TextEditingController();
    final made = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('New account'),
        content: TextField(
          controller: name,
          decoration: const InputDecoration(labelText: 'Account name'),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, name.text.trim()),
            child: const Text('Create'),
          ),
        ],
      ),
    );
    name.dispose();
    if (made == null || made.isEmpty) return;
    // Creating sets the first password the way a reset does.
    await _run(() => widget.api.putAccount(made));
    await _reset(made);
  }

  Future<void> _reset(String name) async {
    try {
      final secret = await widget.api.resetPassword(name);
      if (!mounted) return;
      await showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (context) => _SecretDialog(name: name, secret: secret),
      );
    } on AccessRefused catch (error) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(error.reason)));
    }
  }

  Future<void> _groups(AccountEntry account) async {
    final groups = TextEditingController(text: account.groups.join(', '));
    final chosen = await showDialog<List<String>>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Groups of ${account.name}'),
        content: TextField(
          controller: groups,
          decoration: const InputDecoration(labelText: 'Comma separated'),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, [
              for (final group in groups.text.split(','))
                if (group.trim().isNotEmpty) group.trim(),
            ]),
            child: const Text('Save'),
          ),
        ],
      ),
    );
    groups.dispose();
    if (chosen != null) {
      await _run(() => widget.api.putAccount(account.name, groups: chosen));
    }
  }

  @override
  Widget build(BuildContext context) {
    final accounts = _accounts;
    if (_error != null) return Center(child: Text(_error!));
    if (accounts == null) {
      return const Center(child: CircularProgressIndicator());
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 24),
      children: [
        for (final account in accounts)
          Card(
            child: ListTile(
              title: Text(account.name),
              subtitle: Text(
                account.groups.isEmpty
                    ? 'no groups'
                    : account.groups.join(', '),
              ),
              trailing: Wrap(
                spacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  Switch(
                    value: !account.disabled,
                    onChanged: (enabled) => _run(
                      () => widget.api.putAccount(
                        account.name,
                        disabled: !enabled,
                      ),
                    ),
                  ),
                  if (_canGroups)
                    IconButton(
                      tooltip: 'Groups of ${account.name}',
                      icon: const Icon(Icons.group_outlined),
                      onPressed: () => _groups(account),
                    ),
                  TextButton(
                    onPressed: () => _reset(account.name),
                    child: const Text('Reset password'),
                  ),
                ],
              ),
            ),
          ),
        Align(
          alignment: Alignment.centerLeft,
          child: FilledButton.icon(
            icon: const Icon(Icons.add),
            label: const Text('New account'),
            onPressed: _create,
          ),
        ),
      ],
    );
  }
}

/// Shows a new password once. It is held only by this dialog: closing it drops it.
class _SecretDialog extends StatelessWidget {
  const _SecretDialog({required this.name, required this.secret});

  final String name;
  final String secret;

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text('New password for $name'),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('Shown once. Copy it now; it is not kept anywhere.'),
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
