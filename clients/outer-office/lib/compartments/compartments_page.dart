import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';

import '../forms/form_host.dart';
import '../main.dart';
import '../roster/record_editor.dart' show RecordFormSpec;
import '../routes/oo_route.dart';
import '../shell/oo_shell.dart' show OoPage;

/// A change the gateway refused, in its words.
class CompartmentRefused implements Exception {
  const CompartmentRefused(this.reason);

  final String reason;

  @override
  String toString() => reason;
}

/// One cover or read-in as the gateway shows it.
@immutable
class CompartmentEntry {
  const CompartmentEntry(this.entry, {this.propagate = true});

  /// `operation:ID`, `mission:ID`, or a read-in's `person:`, `group:` and so on.
  final String entry;
  final bool propagate;

  factory CompartmentEntry.fromJson(Map<String, dynamic> json) =>
      CompartmentEntry(
        json['entry'] as String,
        propagate: json['propagate'] as bool? ?? true,
      );
}

/// A compartment as the person may see it: what they may not see is neither named nor counted.
@immutable
class CompartmentView {
  const CompartmentView({
    required this.id,
    required this.settings,
    required this.weakenings,
    required this.covers,
    required this.readIn,
    required this.writable,
  });

  final String id;

  /// What the settings form holds: its description, actions, floor and limits.
  final Map<String, Object?> settings;
  final List<String> weakenings;
  final List<CompartmentEntry> covers;
  final List<CompartmentEntry> readIn;
  final bool writable;

  factory CompartmentView.fromJson(Map<String, dynamic> json) {
    List<CompartmentEntry> entries(String key) => [
      for (final entry in json[key] as List<dynamic>? ?? const [])
        CompartmentEntry.fromJson(entry as Map<String, dynamic>),
    ];
    return CompartmentView(
      id: json['id'] as String,
      settings: {
        'description': json['description'],
        'actions': json['actions'],
        'min_clearance': ?json['min_clearance'],
        'limits': ?json['limits'],
      },
      weakenings: (json['weakenings'] as List<dynamic>? ?? const [])
          .cast<String>(),
      covers: entries('covers'),
      readIn: entries('read_in'),
      writable: json['writable'] as bool? ?? false,
    );
  }
}

/// The gateway's compartment routes.
class CompartmentsApi {
  const CompartmentsApi(this.board);

  final MissionBoard board;

  static const _root = '/v1/user/compartments';

  Map<String, dynamic> _object(http.Response response) {
    final Object? decoded = response.body.isEmpty
        ? null
        : jsonDecode(response.body);
    if (response.statusCode >= 200 && response.statusCode < 300) {
      return decoded is Map<String, dynamic> ? decoded : const {};
    }
    final reason = decoded is Map ? decoded['error'] : null;
    throw CompartmentRefused(
      reason is String ? reason : 'The gateway refused this.',
    );
  }

  Future<List<CompartmentView>> list() async {
    final response = await board.accessRequest('GET', _root);
    if (response.statusCode != 200) {
      _object(response);
      return const [];
    }
    return [
      for (final view in jsonDecode(response.body) as List<dynamic>)
        CompartmentView.fromJson(view as Map<String, dynamic>),
    ];
  }

  Future<RecordFormSpec> form() async {
    final response = await board.accessRequest(
      'GET',
      '/v1/user/compartment-form',
    );
    return RecordFormSpec.fromJson(_object(response));
  }

  Future<void> save(String id, Map<String, Object?> settings) async {
    _object(
      await board.accessRequest(
        'PUT',
        '$_root/${Uri.encodeComponent(id)}',
        body: settings,
      ),
    );
  }

  Future<void> remove(String id) async {
    _object(
      await board.accessRequest('DELETE', '$_root/${Uri.encodeComponent(id)}'),
    );
  }

  Future<void> setEntry(
    String id,
    String kind,
    String entry, {
    bool propagate = true,
  }) async {
    _object(
      await board.accessRequest(
        'PUT',
        '$_root/${Uri.encodeComponent(id)}/$kind/${Uri.encodeComponent(entry)}',
        body: {'propagate': propagate},
      ),
    );
  }

  Future<void> removeEntry(String id, String kind, String entry) async {
    _object(
      await board.accessRequest(
        'DELETE',
        '$_root/${Uri.encodeComponent(id)}/$kind/${Uri.encodeComponent(entry)}',
      ),
    );
  }
}

/// The Operations this person may consent for: the only ones a cover may name from here.
List<String> consentingOperations(MissionBoard board) => [
  for (final operation
      in board.userSession?.operations ?? const <UserOperationAccess>[])
    if (operation.actions.contains('compartment.cover')) operation.operationId,
];

/// Compartments: what each grants, what it covers and who is read in.
class CompartmentsPage extends StatefulWidget {
  const CompartmentsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<CompartmentsPage> createState() => _CompartmentsPageState();
}

class _CompartmentsPageState extends State<CompartmentsPage> {
  List<CompartmentView>? _views;
  String? _problem;

  CompartmentsApi get _api => CompartmentsApi(widget.board);

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load({bool keepProblem = false}) async {
    try {
      final views = await _api.list();
      if (mounted) {
        setState(() {
          _views = views;
          if (!keepProblem) _problem = null;
        });
      }
    } on Object catch (error) {
      if (mounted) setState(() => _problem = error.toString());
    }
  }

  void _select(String? id) {
    final base = OoRoute(OoPage.compartments);
    widget.onRoute(id == null ? base : base.withField('compartment_id', id));
  }

  /// Runs one change, and shows what the gateway said; the list reads again either way.
  Future<void> _change(Future<void> Function() change) async {
    var refused = false;
    try {
      await change();
    } on Object catch (error) {
      refused = true;
      if (mounted) setState(() => _problem = error.toString());
    }
    await _load(keepProblem: refused);
  }

  Future<void> _edit(CompartmentView? view) async {
    final saved = await showDialog<String>(
      context: context,
      builder: (_) => _SettingsDialog(api: _api, view: view),
    );
    if (saved != null) {
      _select(saved);
      await _load();
    }
  }

  Future<void> _cover(CompartmentView view) async {
    final picked = await showDialog<(String, bool)>(
      context: context,
      builder: (_) => _CoverDialog(
        operations: consentingOperations(widget.board),
        covered: {for (final cover in view.covers) cover.entry},
      ),
    );
    if (picked == null) return;
    await _change(
      () => _api.setEntry(view.id, 'covers', picked.$1, propagate: picked.$2),
    );
  }

  Future<void> _readIn(CompartmentView view) async {
    final picked = await showDialog<(String, bool)>(
      context: context,
      builder: (_) => const _ReadInDialog(),
    );
    if (picked == null) return;
    await _change(
      () => _api.setEntry(view.id, 'read-in', picked.$1, propagate: picked.$2),
    );
  }

  @override
  Widget build(BuildContext context) {
    final views = _views;
    final selected = widget.route['compartment_id'];
    final manages = widget.board.globalActions.contains('compartment.manage');
    if (views == null) {
      return Center(
        child: _problem == null
            ? const CircularProgressIndicator()
            : Text(_problem!),
      );
    }
    final shown = selected == null
        ? views
        : views.where((view) => view.id == selected).toList();
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Row(
          children: [
            if (selected != null)
              TextButton(
                onPressed: () => _select(null),
                child: const Text('All compartments'),
              ),
            const Spacer(),
            if (manages)
              FilledButton(
                key: const Key('compartment-new'),
                onPressed: () => unawaited(_edit(null)),
                child: const Text('New compartment'),
              ),
          ],
        ),
        if (_problem case final problem?)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Text(
              problem,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          ),
        if (shown.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 48),
            child: Center(
              child: Text(
                selected == null
                    ? 'No compartment is open to this account. A compartment gives '
                          'need-to-know access to other Operations.'
                    : 'Compartment $selected is not open to this account.',
              ),
            ),
          ),
        for (final view in shown)
          _CompartmentCard(
            view: view,
            onEdit: () => unawaited(_edit(view)),
            onCover: () => unawaited(_cover(view)),
            onReadIn: () => unawaited(_readIn(view)),
            onUncover: (entry) => unawaited(
              _change(() => _api.removeEntry(view.id, 'covers', entry)),
            ),
            onReadOut: (entry) => unawaited(
              _change(() => _api.removeEntry(view.id, 'read-in', entry)),
            ),
            onRemove: () => unawaited(_change(() => _api.remove(view.id))),
          ),
      ],
    );
  }
}

class _CompartmentCard extends StatelessWidget {
  const _CompartmentCard({
    required this.view,
    required this.onEdit,
    required this.onCover,
    required this.onReadIn,
    required this.onUncover,
    required this.onReadOut,
    required this.onRemove,
  });

  final CompartmentView view;
  final VoidCallback onEdit;
  final VoidCallback onCover;
  final VoidCallback onReadIn;
  final ValueChanged<String> onUncover;
  final ValueChanged<String> onReadOut;
  final VoidCallback onRemove;

  Widget _entries(
    BuildContext context,
    String title,
    List<CompartmentEntry> entries,
    ValueChanged<String> onRemoved,
    String empty,
  ) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Text(title, style: Theme.of(context).textTheme.titleSmall),
      if (entries.isEmpty) Text(empty),
      Wrap(
        spacing: 8,
        children: [
          for (final entry in entries)
            InputChip(
              label: Text(
                entry.propagate
                    ? entry.entry
                    : '${entry.entry} (this one only)',
              ),
              onDeleted: view.writable ? () => onRemoved(entry.entry) : null,
              deleteButtonTooltipMessage: 'Remove ${entry.entry}',
            ),
        ],
      ),
    ],
  );

  @override
  Widget build(BuildContext context) => Card(
    margin: const EdgeInsets.only(top: 12),
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: 8,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  view.id,
                  style: Theme.of(context).textTheme.titleMedium,
                ),
              ),
              if (view.writable) ...[
                TextButton(onPressed: onEdit, child: const Text('Edit')),
                TextButton(onPressed: onCover, child: const Text('Cover…')),
                TextButton(onPressed: onReadIn, child: const Text('Read in…')),
                IconButton(
                  tooltip: 'Delete ${view.id}',
                  onPressed: onRemove,
                  icon: const Icon(Icons.delete_outline),
                ),
              ],
            ],
          ),
          if ((view.settings['description'] as String? ?? '').isNotEmpty)
            Text(view.settings['description']! as String),
          Wrap(
            spacing: 8,
            children: [
              for (final action
                  in view.settings['actions'] as List<dynamic>? ?? const [])
                Chip(label: Text('$action')),
              for (final weakening in view.weakenings)
                Tooltip(
                  message: weakening,
                  child: Chip(
                    avatar: const Icon(Icons.warning_amber, size: 18),
                    label: const Text('Weakened'),
                    key: Key('compartment-weakening-${view.id}'),
                  ),
                ),
            ],
          ),
          _entries(
            context,
            'Covers',
            view.covers,
            onUncover,
            'Nothing you may see.',
          ),
          _entries(
            context,
            'Read in',
            view.readIn,
            onReadOut,
            'Nobody you may see.',
          ),
        ],
      ),
    ),
  );
}

/// The settings form, drawn from the schema the gateway serves.
class _SettingsDialog extends StatefulWidget {
  const _SettingsDialog({required this.api, required this.view});

  final CompartmentsApi api;

  /// The compartment being edited; `null` for a new one.
  final CompartmentView? view;

  @override
  State<_SettingsDialog> createState() => _SettingsDialogState();
}

class _SettingsDialogState extends State<_SettingsDialog> {
  final _key = TextEditingController();
  RecordFormSpec? _spec;
  FormModel? _model;
  String? _problem;
  Map<String, String> _problems = const {};
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void dispose() {
    _key.dispose();
    _model?.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final spec = await widget.api.form();
      if (!mounted) return;
      setState(() {
        _spec = spec;
        _model = ooFormModel(
          schema: spec.schema,
          form: spec.form,
          value: widget.view?.settings ?? const {},
        );
      });
    } on Object catch (error) {
      if (mounted) setState(() => _problem = error.toString());
    }
  }

  Future<void> _save() async {
    final model = _model!;
    final id = widget.view?.id ?? _key.text.trim();
    if (id.isEmpty) {
      setState(() => _problem = '${_spec!.keyTitle}: this is needed.');
      return;
    }
    final issues = <String, String>{};
    final settings = model.value(issues: issues);
    if (issues.isNotEmpty) {
      setState(() {
        _problems = issues;
        _problem = 'Some fields need another look.';
      });
      return;
    }
    setState(() {
      _saving = true;
      _problem = null;
      _problems = const {};
    });
    try {
      await widget.api.save(id, settings);
      if (mounted) Navigator.of(context).pop(id);
    } on Object catch (error) {
      if (mounted) setState(() => _problem = error.toString());
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final model = _model;
    return AlertDialog(
      title: Text(
        widget.view == null
            ? 'New compartment'
            : 'Compartment ${widget.view!.id}',
      ),
      content: SizedBox(
        width: 560,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            spacing: 12,
            children: [
              if (widget.view == null && _spec != null)
                TextField(
                  key: const Key('compartment-key'),
                  controller: _key,
                  decoration: InputDecoration(
                    labelText: '${_spec!.keyTitle} (required)',
                    helperText: _spec!.keyDescription,
                    helperMaxLines: 3,
                    border: const OutlineInputBorder(),
                  ),
                ),
              if (model == null && _problem == null)
                const Center(child: CircularProgressIndicator()),
              if (model != null)
                OoFormView(
                  key: ObjectKey(model),
                  model: model,
                  problems: _problems,
                  enabled: !_saving,
                ),
              if (_problem case final problem?)
                Text(
                  problem,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: _saving ? null : () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const Key('compartment-save'),
          onPressed: _saving || model == null ? null : () => unawaited(_save()),
          child: Text(_saving ? 'Saving…' : 'Save'),
        ),
      ],
    );
  }
}

/// Picks what to cover: an Operation this person may consent for, or a Mission by ID.
class _CoverDialog extends StatefulWidget {
  const _CoverDialog({required this.operations, required this.covered});

  final List<String> operations;
  final Set<String> covered;

  @override
  State<_CoverDialog> createState() => _CoverDialogState();
}

class _CoverDialogState extends State<_CoverDialog> {
  final _mission = TextEditingController();
  bool _propagate = true;

  @override
  void dispose() {
    _mission.dispose();
    super.dispose();
  }

  void _pick(String entry) => Navigator.of(context).pop((entry, _propagate));

  @override
  Widget build(BuildContext context) {
    final open = [
      for (final operation in widget.operations)
        if (!widget.covered.contains('operation:$operation')) operation,
    ];
    return AlertDialog(
      title: const Text('Cover'),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (open.isEmpty)
              const Text('No other Operation has given you its consent.'),
            for (final operation in open)
              ListTile(
                key: Key('compartment-cover-$operation'),
                title: Text(operation),
                onTap: () => _pick('operation:$operation'),
              ),
            SwitchListTile(
              title: const Text('Include the Operations under it'),
              value: _propagate,
              onChanged: (value) => setState(() => _propagate = value),
            ),
            TextField(
              key: const Key('compartment-cover-mission'),
              controller: _mission,
              decoration: const InputDecoration(
                labelText: 'Or one Mission, by ID',
                border: OutlineInputBorder(),
              ),
              onSubmitted: (mission) {
                if (mission.trim().isNotEmpty) {
                  Navigator.of(context)
                      .pop(('mission:${mission.trim()}', true));
                }
              },
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}

/// Picks who to read in.
class _ReadInDialog extends StatefulWidget {
  const _ReadInDialog();

  @override
  State<_ReadInDialog> createState() => _ReadInDialogState();
}

class _ReadInDialogState extends State<_ReadInDialog> {
  static const _kinds = {
    'operation': 'An Operation',
    'mission': 'A Mission',
    'agent': 'An Agent',
    'group': 'A group of people',
    'person': 'A person',
    'account': 'An account',
  };

  final _name = TextEditingController();
  String _kind = 'operation';
  bool _propagate = true;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _submit() {
    final name = _name.text.trim();
    if (name.isEmpty) return;
    Navigator.of(context).pop(('$_kind:$name', _propagate));
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Read in'),
    content: SizedBox(
      width: 420,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 12,
        children: [
          DropdownButton<String>(
            key: const Key('compartment-read-in-kind'),
            value: _kind,
            isExpanded: true,
            items: [
              for (final kind in _kinds.entries)
                DropdownMenuItem(value: kind.key, child: Text(kind.value)),
            ],
            onChanged: (kind) => setState(() => _kind = kind ?? _kind),
          ),
          TextField(
            key: const Key('compartment-read-in-name'),
            controller: _name,
            decoration: const InputDecoration(
              labelText: 'Its name',
              border: OutlineInputBorder(),
            ),
            onSubmitted: (_) => _submit(),
          ),
          if (_kind == 'operation')
            SwitchListTile(
              title: const Text('Include the Operations under it'),
              value: _propagate,
              onChanged: (value) => setState(() => _propagate = value),
            ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        key: const Key('compartment-read-in-save'),
        onPressed: _submit,
        child: const Text('Read in'),
      ),
    ],
  );
}
