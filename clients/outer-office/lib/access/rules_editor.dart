import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_route.dart';
import '../widgets/oo_data_table.dart';
import '../widgets/oo_principals.dart';
import 'access_api.dart';

/// The ordered rules of a policy, as a table. Order is meaning (the first rule that matches
/// decides), so the priority column is the rule's place in the list whatever the table is sorted
/// by, rows move by the move buttons, and Save sends the whole list with the revision it was
/// read at.
class RulesEditor extends StatefulWidget {
  const RulesEditor({
    required this.view,
    required this.operationId,
    required this.canEdit,
    required this.onSave,
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;
  final AccessView view;
  final String? operationId;
  final bool canEdit;
  final ValueChanged<List<AccessRule>> onSave;

  @override
  State<RulesEditor> createState() => _RulesEditorState();
}

class _RulesEditorState extends State<RulesEditor> {
  late List<AccessRule> _rules = [...widget.view.rules];

  bool get _dirty {
    final saved = widget.view.rules;
    if (saved.length != _rules.length) return true;
    for (var i = 0; i < saved.length; i++) {
      if (saved[i].subject != _rules[i].subject ||
          saved[i].scope != _rules[i].scope ||
          saved[i].role != _rules[i].role ||
          saved[i].propagate != _rules[i].propagate) {
        return true;
      }
    }
    return false;
  }

  @override
  void didUpdateWidget(RulesEditor oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.view != widget.view) _rules = [...widget.view.rules];
  }

  void _move(int from, int to) {
    if (to < 0 || to >= _rules.length) return;
    setState(() => _rules.insert(to, _rules.removeAt(from)));
  }

  Future<void> _add() async {
    final rule = await showDialog<AccessRule>(
      context: context,
      builder: (context) => _RuleDialog(
        principals: widget.board.principals,
        roles: [for (final role in widget.view.roles) role.name],
        scope: widget.operationId == null
            ? '*'
            : 'operation:${widget.operationId}',
      ),
    );
    if (rule != null) setState(() => _rules.add(rule));
  }

  @override
  Widget build(BuildContext context) {
    final edit = widget.canEdit;
    final principals = widget.board.principals;
    final indexed = [for (final (index, rule) in _rules.indexed) (index, rule)];
    return ListenableBuilder(
      listenable: principals,
      builder: (context, _) => _table(context, edit, principals, indexed),
    );
  }

  Widget _table(
    BuildContext context,
    bool edit,
    OoPrincipals principals,
    List<(int, AccessRule)> indexed,
  ) {
    String kind(AccessRule rule) => OoPrincipalRef.parse(rule.subject).kind;
    return Column(
      children: [
        Expanded(
          child: OoDataTable<(int, AccessRule)>(
            rows: indexed,
            rowId: (row) => '${row.$1}',
            noun: 'rules',
            route: widget.route,
            onRoute: widget.onRoute,
            defaultSort: ('priority', true),
            searchText: (row) =>
                '${ooPrincipalText(principals, row.$2.subject)} ${row.$2.subject} '
                '${row.$2.scope} ${row.$2.role}',
            facets: [
              OoFacet(
                id: 'subject',
                label: 'Subject kind',
                values: (row) => [kind(row.$2)],
              ),
            ],
            columns: [
              OoColumn(
                id: 'priority',
                label: '#',
                width: 56,
                sortKey: (row) => row.$1,
                cell: (context, row) => Text('${row.$1 + 1}'),
              ),
              OoColumn(
                id: 'subject',
                label: 'Subject',
                flex: 3,
                sortKey: (row) =>
                    ooPrincipalText(principals, row.$2.subject).toLowerCase(),
                cell: (context, row) => OoPrincipalBadge(row.$2.subject),
              ),
              OoColumn(
                id: 'scope',
                label: 'Scope',
                flex: 2,
                sortKey: (row) => row.$2.scope,
                cell: (context, row) => Text(row.$2.scope),
              ),
              OoColumn(
                id: 'role',
                label: 'Role',
                flex: 2,
                sortKey: (row) => row.$2.role,
                cell: (context, row) => Text(
                  '${row.$2.role}${row.$2.propagate ? '' : ' (this scope only)'}'
                  '${row.$2.editor == null ? '' : ' - ${row.$2.editor}'}',
                ),
              ),
              if (edit)
                OoColumn(
                  id: 'actions',
                  label: 'Actions',
                  width: 150,
                  cell: (context, row) => Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      IconButton(
                        tooltip: 'Move rule ${row.$1 + 1} up',
                        icon: const Icon(Icons.arrow_upward),
                        onPressed: row.$1 == 0
                            ? null
                            : () => _move(row.$1, row.$1 - 1),
                      ),
                      IconButton(
                        tooltip: 'Move rule ${row.$1 + 1} down',
                        icon: const Icon(Icons.arrow_downward),
                        onPressed: row.$1 == _rules.length - 1
                            ? null
                            : () => _move(row.$1, row.$1 + 1),
                      ),
                      IconButton(
                        tooltip: 'Delete rule ${row.$1 + 1}',
                        icon: const Icon(Icons.delete_outline),
                        onPressed: () =>
                            setState(() => _rules.removeAt(row.$1)),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
        if (edit)
          Padding(
            padding: const EdgeInsets.all(16),
            child: Row(
              children: [
                OutlinedButton.icon(
                  icon: const Icon(Icons.add),
                  label: const Text('Add rule'),
                  onPressed: _add,
                ),
                const SizedBox(width: 12),
                FilledButton(
                  onPressed: _dirty ? () => widget.onSave([..._rules]) : null,
                  child: const Text('Save'),
                ),
              ],
            ),
          ),
        if (widget.view.orphaned.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (final (item, reason) in widget.view.orphaned)
                  Text('Not applied - $item: $reason'),
              ],
            ),
          ),
      ],
    );
  }
}

class _RuleDialog extends StatefulWidget {
  const _RuleDialog({
    required this.roles,
    required this.scope,
    required this.principals,
  });

  final OoPrincipals principals;
  final List<String> roles;
  final String scope;

  @override
  State<_RuleDialog> createState() => _RuleDialogState();
}

class _RuleDialogState extends State<_RuleDialog> {
  final _subject = TextEditingController();
  late final _scope = TextEditingController(text: widget.scope);
  late String _role = widget.roles.contains('operator')
      ? 'operator'
      : widget.roles.first;
  bool _propagate = true;

  @override
  void dispose() {
    _subject.dispose();
    _scope.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Add rule'),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        OoPrincipalPicker(controller: _subject, principals: widget.principals),
        TextField(
          controller: _scope,
          decoration: const InputDecoration(
            labelText: 'Scope (*, operation:ID, station:ID, config:NAME)',
          ),
        ),
        DropdownButton<String>(
          value: _role,
          isExpanded: true,
          items: [
            for (final role in widget.roles)
              DropdownMenuItem(value: role, child: Text(role)),
          ],
          onChanged: (role) => setState(() => _role = role ?? _role),
        ),
        SwitchListTile(
          title: const Text('Reaches Operations under the scope'),
          value: _propagate,
          onChanged: (on) => setState(() => _propagate = on),
        ),
      ],
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: () => Navigator.pop(
          context,
          AccessRule(
            subject: _subject.text.trim(),
            scope: _scope.text.trim(),
            role: _role,
            propagate: _propagate,
          ),
        ),
        child: const Text('Add'),
      ),
    ],
  );
}
