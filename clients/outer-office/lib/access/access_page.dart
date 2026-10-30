import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_route.dart';
import 'access_api.dart';
import 'accounts_page.dart';
import 'role_editor.dart';
import 'rules_editor.dart';

/// The actions the signed-in person holds where a policy is edited: on [operationId], the
/// Operation's own; globally, what the profile shows everywhere (the gateway's guard decides
/// in the end).
Set<String> heldActions(MissionBoard board, String? operationId) {
  final session = board.userSession;
  if (board.demo) return {...operationActions, 'station.configure'};
  if (session == null) return const {};
  if (operationId != null) {
    for (final operation in session.operations) {
      if (operation.operationId == operationId) return {...operation.actions};
    }
    return const {};
  }
  final everywhere = session.operations.isEmpty
      ? <String>{}
      : session.operations
            .map((operation) => operation.actions.toSet())
            .reduce((a, b) => a.intersection(b));
  return {
    ...session.globalActions,
    ...session.roster,
    for (final station in session.stations) ...station.actions,
    ...everywhere,
  };
}

/// The Access page: the roles, the ordered rules, the accounts and an explanation of one
/// answer, for the global policy or, with `operation_id`, one Operation's own.
class AccessPage extends StatefulWidget {
  const AccessPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<AccessPage> createState() => _AccessPageState();
}

class _AccessPageState extends State<AccessPage> {
  AccessView? _view;
  String? _error;
  bool _stale = false;
  bool _loading = false;

  String? get _operation => widget.route['operation_id'];
  AccessApi get _api => AccessApi(widget.board, operationId: _operation);

  bool get _canManage => _operation == null
      ? widget.board.globalActions.contains('access.manage')
      : heldActions(widget.board, _operation).contains('access.manage');

  String get _tab {
    final tab = widget.route['tab'];
    if (tab != null) return tab;
    return widget.board.globalActions.contains('access.read') ||
            _operation != null
        ? 'roles'
        : 'accounts';
  }

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void didUpdateWidget(AccessPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      setState(() => _view = null);
      unawaited(_load());
    }
  }

  Future<void> _load() async {
    if (_tab == 'accounts' && _operation == null && !_needsPolicy) return;
    setState(() => _loading = true);
    try {
      final view = await _api.load();
      if (mounted) {
        setState(() {
          _view = view;
          _error = null;
          _stale = false;
        });
      }
    } on AccessRefused catch (error) {
      if (mounted) setState(() => _error = error.reason);
    } on StateError catch (error) {
      if (mounted) setState(() => _error = error.message);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  bool get _needsPolicy => widget.board.globalActions.contains('access.read');

  /// Runs one write, then reloads; a stale revision asks for a reload instead.
  Future<void> _change(Future<String> Function(String revision) write) async {
    final view = _view;
    if (view == null) return;
    try {
      await write(view.revision);
      await _load();
    } on AccessRefused catch (error) {
      if (!mounted) return;
      if (error.stale) {
        setState(() => _stale = true);
      } else {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.reason)));
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final tabs = [
      if (_operation != null || _needsPolicy) ...['roles', 'rules'],
      if (_operation == null &&
          widget.board.globalActions.contains('account.manage'))
        'accounts',
      if (_operation != null || _needsPolicy) 'check',
    ];
    if (tabs.isEmpty) {
      return ListView(
        padding: const EdgeInsets.all(24),
        children: [
          Text(
            'Operations whose access you may read',
            style: Theme.of(context).textTheme.titleLarge,
          ),
          for (final id in widget.board.accessOperations)
            ListTile(
              title: Text(id),
              onTap: () =>
                  widget.onRoute(widget.route.withField('operation_id', id)),
            ),
        ],
      );
    }
    final tab = tabs.contains(_tab) ? _tab : tabs.first;
    final view = _view;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 16, 24, 8),
          child: Wrap(
            spacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text(
                _operation == null
                    ? 'Access'
                    : 'Access of Operation $_operation',
                style: Theme.of(context).textTheme.titleLarge,
              ),
              for (final id in tabs)
                ChoiceChip(
                  label: Text(switch (id) {
                    'roles' => 'Roles',
                    'rules' => 'Rules',
                    'accounts' => 'Accounts',
                    _ => 'Check',
                  }),
                  selected: id == tab,
                  onSelected: (_) =>
                      widget.onRoute(widget.route.withField('tab', id)),
                ),
              if (!_canManage && tab != 'accounts' && tab != 'check')
                const Chip(label: Text('Read only')),
            ],
          ),
        ),
        if (_stale)
          MaterialBanner(
            content: const Text(
              'The policy changed since you read it. Reload it before editing '
              'again; your change was not saved.',
            ),
            actions: [
              TextButton(onPressed: _load, child: const Text('Reload')),
            ],
          ),
        if (view?.invalid != null)
          MaterialBanner(
            content: Text(
              'The stored policy is not in force (${view!.invalid}); the '
              'last good one still applies.',
            ),
            actions: const [SizedBox.shrink()],
          ),
        if (_error != null)
          Padding(padding: const EdgeInsets.all(24), child: Text(_error!)),
        Expanded(
          child: _loading && view == null
              ? const Center(child: CircularProgressIndicator())
              : switch (tab) {
                  'accounts' => AccountsTab(board: widget.board, api: _api),
                  'check' => _CheckTab(api: _api, operation: _operation),
                  _ when view == null => const SizedBox.shrink(),
                  'rules' => RulesEditor(
                    view: view,
                    operationId: _operation,
                    canEdit: _canManage && view.writable,
                    onSave: (rules) =>
                        _change((revision) => _api.setRules(revision, rules)),
                  ),
                  _ => _RolesTab(
                    view: view,
                    operationId: _operation,
                    board: widget.board,
                    canEdit: _canManage && view.writable,
                    onSet: (name, actions, description) => _change(
                      (revision) => _api.setRole(
                        revision,
                        name,
                        actions,
                        description: description,
                      ),
                    ),
                    onRemove: (name) =>
                        _change((revision) => _api.removeRole(revision, name)),
                    onChief: (subjects) => _change(
                      (revision) => _api.setChief(revision, subjects),
                    ),
                  ),
                },
        ),
      ],
    );
  }
}

class _RolesTab extends StatelessWidget {
  const _RolesTab({
    required this.view,
    required this.operationId,
    required this.board,
    required this.canEdit,
    required this.onSet,
    required this.onRemove,
    required this.onChief,
  });

  final AccessView view;
  final String? operationId;
  final MissionBoard board;
  final bool canEdit;
  final void Function(String, List<String>, String?) onSet;
  final ValueChanged<String> onRemove;
  final ValueChanged<List<String>> onChief;

  @override
  Widget build(BuildContext context) {
    final held = heldActions(board, operationId);
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 24),
      children: [
        if (view.managedBy != null)
          Text('Access is managed by authorizer ${view.managedBy}.'),
        for (final role in view.roles)
          Card(
            child: ListTile(
              title: Row(
                children: [
                  Text(role.name),
                  if (role.builtIn) ...[
                    const SizedBox(width: 8),
                    const Chip(label: Text('built in')),
                  ],
                  if (!role.editable) ...[
                    const SizedBox(width: 8),
                    const Icon(Icons.lock_outline, size: 16),
                  ],
                ],
              ),
              subtitle: Wrap(
                spacing: 6,
                children: [
                  for (final action in role.actions)
                    Chip(
                      label: Text(action),
                      visualDensity: VisualDensity.compact,
                    ),
                ],
              ),
              trailing: canEdit && role.editable
                  ? Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        IconButton(
                          tooltip: 'Edit ${role.name}',
                          icon: const Icon(Icons.edit_outlined),
                          onPressed: () => _edit(context, role, held),
                        ),
                        IconButton(
                          tooltip: 'Delete ${role.name}',
                          icon: const Icon(Icons.delete_outline),
                          onPressed: () => onRemove(role.name),
                        ),
                      ],
                    )
                  : null,
            ),
          ),
        if (canEdit)
          Align(
            alignment: Alignment.centerLeft,
            child: FilledButton.icon(
              icon: const Icon(Icons.add),
              label: const Text('New role'),
              onPressed: () => _edit(context, null, held),
            ),
          ),
        if (view.inherited.isNotEmpty) ...[
          const SizedBox(height: 16),
          Text('Inherited', style: Theme.of(context).textTheme.titleSmall),
          for (final (role, from) in view.inherited) Text('$role, from $from'),
        ],
        if (view.orphaned.isNotEmpty) ...[
          const SizedBox(height: 16),
          Text('Not applied', style: Theme.of(context).textTheme.titleSmall),
          for (final (item, reason) in view.orphaned) Text('$item: $reason'),
        ],
        if (operationId == null) ...[
          const SizedBox(height: 16),
          Text(
            'What Operation roles may hold',
            style: Theme.of(context).textTheme.titleSmall,
          ),
          Wrap(
            spacing: 6,
            children: [
              for (final action in view.delegable)
                Chip(
                  label: Text(action),
                  backgroundColor: view.weakenings.contains(action)
                      ? Theme.of(context).colorScheme.errorContainer
                      : null,
                ),
            ],
          ),
          if (view.weakenings.isNotEmpty)
            Text('Weakened against the default: ${view.weakenings.join(', ')}'),
          const SizedBox(height: 16),
          Text('Chief', style: Theme.of(context).textTheme.titleSmall),
          for (final subject in view.chiefHost)
            Text('$subject (from unex.cfg, fixed)'),
          for (final subject in view.chiefStored) Text(subject),
        ],
      ],
    );
  }

  Future<void> _edit(
    BuildContext context,
    AccessRole? role,
    Set<String> held,
  ) async {
    final result = await showDialog<RoleDraft>(
      context: context,
      builder: (context) => RoleEditor(
        role: role,
        universe: operationId == null
            ? view.roles.firstWhere((r) => r.name == 'chief').actions
            : operationActions,
        held: held,
        operationScope: operationId != null,
        cap: view.delegable.toSet(),
      ),
    );
    if (result != null) {
      onSet(result.name, result.actions, result.description);
    }
  }
}

class _CheckTab extends StatefulWidget {
  const _CheckTab({required this.api, required this.operation});

  final AccessApi api;
  final String? operation;

  @override
  State<_CheckTab> createState() => _CheckTabState();
}

class _CheckTabState extends State<_CheckTab> {
  final _subject = TextEditingController();
  final _action = TextEditingController();
  late final _resource = TextEditingController(
    text: widget.operation == null ? '*' : 'operation:${widget.operation}',
  );
  String? _result;

  @override
  void dispose() {
    _subject.dispose();
    _action.dispose();
    _resource.dispose();
    super.dispose();
  }

  Future<void> _run() async {
    try {
      final answer = await widget.api.check(
        _subject.text.trim(),
        _action.text.trim(),
        _resource.text.trim(),
      );
      setState(() {
        _result = answer.allowed
            ? 'Allowed by ${answer.tier}, rule ${answer.ruleIndex}, '
                  'role ${answer.role}.'
            : 'Denied: no rule of any tier allows it.';
      });
    } on AccessRefused catch (error) {
      setState(() => _result = error.reason);
    }
  }

  @override
  Widget build(BuildContext context) => ListView(
    padding: const EdgeInsets.all(24),
    children: [
      TextField(
        controller: _subject,
        decoration: const InputDecoration(
          labelText: 'Subject (account:NAME, person:ID or group:NAME)',
        ),
      ),
      TextField(
        controller: _action,
        decoration: const InputDecoration(labelText: 'Action'),
      ),
      TextField(
        controller: _resource,
        decoration: const InputDecoration(labelText: 'Resource'),
      ),
      const SizedBox(height: 12),
      Align(
        alignment: Alignment.centerLeft,
        child: FilledButton(onPressed: _run, child: const Text('Check')),
      ),
      if (_result != null)
        Padding(padding: const EdgeInsets.only(top: 12), child: Text(_result!)),
    ],
  );
}
