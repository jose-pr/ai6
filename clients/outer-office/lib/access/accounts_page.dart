import 'dart:async';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_route.dart';
import '../widgets/oo_data_table.dart';
import '../widgets/oo_principals.dart';
import 'access_api.dart';

/// A row of the Accounts table: a local account, or a person a directory knows who has signed in.
class AccountRow {
  const AccountRow.local(AccountEntry this.account) : person = null;
  const AccountRow.directory(OoPerson this.person) : account = null;

  final AccountEntry? account;
  final OoPerson? person;

  String get name => account?.name ?? person!.label;
  String get id =>
      account != null ? 'account:${account!.name}' : 'person:${person!.id}';
  String get source => account != null ? 'Local' : 'Directory';
  List<String> get groups => account?.groups ?? person!.groups;
}

/// The accounts people sign in with: disable or enable, groups (only with `access.manage`)
/// and a password reset, whose new password is shown once in a dialog and kept nowhere.
class AccountsTab extends StatefulWidget {
  const AccountsTab({
    required this.board,
    required this.api,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;
  final MissionBoard board;
  final AccessApi api;

  @override
  State<AccountsTab> createState() => _AccountsTabState();
}

class _AccountsTabState extends State<AccountsTab> {
  List<AccountEntry>? _accounts;
  List<OoPerson> _people = const [];
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
      // Who has signed in through a directory; a refusal only leaves the list to local accounts.
      var people = const <OoPerson>[];
      try {
        people = (await widget.board.principals.search('')).people;
      } on Object {
        people = const [];
      }
      if (mounted) {
        setState(() {
          _accounts = accounts;
          _people = people;
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
    final local = {for (final account in accounts) account.name};
    final rows = [
      for (final account in accounts) AccountRow.local(account),
      for (final person in _people)
        if (!local.contains(person.name)) AccountRow.directory(person),
    ];
    return Column(
      children: [
        Expanded(
          child: OoDataTable<AccountRow>(
            rows: rows,
            rowId: (row) => row.id,
            noun: 'accounts and people',
            route: widget.route,
            onRoute: widget.onRoute,
            defaultSort: ('name', true),
            searchText: (row) =>
                '${row.name} ${row.id} ${row.source} ${row.groups.join(' ')}',
            columns: [
              OoColumn(
                id: 'name',
                label: 'Name',
                flex: 3,
                sortKey: (row) => row.name.toLowerCase(),
                cell: (context, row) => Text(row.name),
              ),
              OoColumn(
                id: 'groups',
                label: 'Groups',
                flex: 3,
                sortKey: (row) => row.groups.join(', '),
                cell: (context, row) => Text(
                  row.groups.isEmpty ? 'no groups' : row.groups.join(', '),
                ),
              ),
              OoColumn(
                id: 'status',
                label: 'Source and status',
                flex: 2,
                sortKey: (row) =>
                    '${row.source} ${row.account?.disabled ?? false}',
                cell: (context, row) => Text(
                  row.account == null
                      ? 'Directory'
                      : 'Local, ${row.account!.disabled ? 'disabled' : 'enabled'}',
                ),
              ),
              OoColumn(
                id: 'actions',
                label: 'Actions',
                width: 360,
                cell: (context, row) {
                  final account = row.account;
                  if (account == null) return const SizedBox.shrink();
                  return Wrap(
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
                  );
                },
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.all(16),
          child: Align(
            alignment: Alignment.centerLeft,
            child: FilledButton.icon(
              icon: const Icon(Icons.add),
              label: const Text('New account'),
              onPressed: _create,
            ),
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
