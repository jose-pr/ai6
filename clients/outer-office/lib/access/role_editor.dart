import 'package:material_ui/material_ui.dart';

import 'access_api.dart';

/// What the role editor returns.
@immutable
class RoleDraft {
  const RoleDraft(this.name, this.actions, this.description);

  final String name;
  final List<String> actions;
  final String? description;
}

/// Edits one role: its actions as switches. A switch the person cannot give is greyed with
/// the reason as its tooltip, so the editor matches the gateway's guard (which stays the
/// authority).
class RoleEditor extends StatefulWidget {
  const RoleEditor({
    required this.universe,
    required this.held,
    required this.operationScope,
    required this.cap,
    this.role,
    super.key,
  });

  /// A role being edited, or `null` for a new one.
  final AccessRole? role;

  /// Every action a role may list here.
  final List<String> universe;

  /// What the person holds.
  final Set<String> held;

  /// Whether this is an Operation's own role: Operation actions within [cap] only.
  final bool operationScope;
  final Set<String> cap;

  @override
  State<RoleEditor> createState() => _RoleEditorState();
}

class _RoleEditorState extends State<RoleEditor> {
  late final _name = TextEditingController(text: widget.role?.name ?? '');
  late final Set<String> _chosen = {...?widget.role?.actions};

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final existing = widget.role != null;
    return AlertDialog(
      title: Text(existing ? 'Edit ${widget.role!.name}' : 'New role'),
      content: SizedBox(
        width: 420,
        child: ListView(
          shrinkWrap: true,
          children: [
            TextField(
              controller: _name,
              enabled: !existing,
              decoration: const InputDecoration(labelText: 'Name'),
            ),
            for (final action in widget.universe)
              _ActionSwitch(
                action: action,
                value: _chosen.contains(action),
                block: _chosen.contains(action) && existing
                    ? null
                    : actionBlock(
                        action,
                        held: widget.held,
                        operationScope: widget.operationScope,
                        cap: widget.cap,
                      ),
                onChanged: (on) => setState(() {
                  on ? _chosen.add(action) : _chosen.remove(action);
                }),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: _chosen.isEmpty || _name.text.trim().isEmpty
              ? null
              : () => Navigator.pop(
                  context,
                  RoleDraft(
                    _name.text.trim(),
                    widget.universe.where(_chosen.contains).toList(),
                    widget.role?.description,
                  ),
                ),
          child: const Text('Save'),
        ),
      ],
    );
  }
}

class _ActionSwitch extends StatelessWidget {
  const _ActionSwitch({
    required this.action,
    required this.value,
    required this.block,
    required this.onChanged,
  });

  final String action;
  final bool value;

  /// Why the switch cannot be turned on, when it cannot.
  final String? block;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final tile = SwitchListTile(
      title: Text(action),
      dense: true,
      value: value,
      onChanged: block == null || value ? onChanged : null,
    );
    return block == null || value
        ? tile
        : Tooltip(message: block!, child: tile);
  }
}
