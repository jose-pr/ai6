import 'package:material_ui/material_ui.dart';

import 'access_api.dart';

/// The ordered rules of a policy. Order is meaning (the first rule that matches decides), so
/// rows move by drag handle or by the move buttons, and Save sends the whole list with the
/// revision it was read at.
class RulesEditor extends StatefulWidget {
  const RulesEditor({
    required this.view,
    required this.operationId,
    required this.canEdit,
    required this.onSave,
    super.key,
  });

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
    return Column(
      children: [
        Expanded(
          child: ReorderableListView(
            padding: const EdgeInsets.symmetric(horizontal: 24),
            buildDefaultDragHandles: edit,
            onReorderItem: (from, to) => setState(() {
              _rules.insert(to, _rules.removeAt(from));
            }),
            children: [
              for (final (index, rule) in _rules.indexed)
                ListTile(
                  key: ObjectKey(rule),
                  title: Text('${rule.subject}  at  ${rule.scope}'),
                  subtitle: Text(
                    '${rule.role}${rule.propagate ? '' : ' (this scope only)'}'
                    '${rule.editor == null ? '' : ' - ${rule.editor}'}',
                  ),
                  trailing: edit
                      ? Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            IconButton(
                              tooltip: 'Move rule ${index + 1} up',
                              icon: const Icon(Icons.arrow_upward),
                              onPressed: index == 0
                                  ? null
                                  : () => _move(index, index - 1),
                            ),
                            IconButton(
                              tooltip: 'Move rule ${index + 1} down',
                              icon: const Icon(Icons.arrow_downward),
                              onPressed: index == _rules.length - 1
                                  ? null
                                  : () => _move(index, index + 1),
                            ),
                            IconButton(
                              tooltip: 'Delete rule ${index + 1}',
                              icon: const Icon(Icons.delete_outline),
                              onPressed: () =>
                                  setState(() => _rules.removeAt(index)),
                            ),
                            const SizedBox(width: 32),
                          ],
                        )
                      : null,
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
  const _RuleDialog({required this.roles, required this.scope});

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
        TextField(
          controller: _subject,
          decoration: const InputDecoration(
            labelText: 'Subject (account:NAME, person:ID, group:NAME)',
          ),
        ),
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
