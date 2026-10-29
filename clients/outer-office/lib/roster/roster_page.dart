import 'dart:async';
import 'dart:convert';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';

/// The roster revision a Station runs, what it refused and which changed files wait for a
/// restart, as it reported them at check-in.
@immutable
class StationConfigStatus {
  const StationConfigStatus({
    this.revision,
    this.refused = const [],
    this.needsRestart = const [],
  });

  /// In decimal: a revision can pass what a JSON number holds exactly.
  final String? revision;
  final List<String> refused;
  final List<String> needsRestart;

  factory StationConfigStatus.fromJson(Map<String, dynamic> json) =>
      StationConfigStatus(
        revision: json['revision'] as String?,
        refused: ((json['refused'] as List<dynamic>?) ?? const [])
            .cast<String>(),
        needsRestart: ((json['needs_restart'] as List<dynamic>?) ?? const [])
            .cast<String>(),
      );
}

/// One roster record: its key and its definition, the keys of its configuration table.
@immutable
class RosterRecord {
  const RosterRecord({required this.key, required this.definition});

  final String key;
  final Map<String, dynamic> definition;

  factory RosterRecord.fromJson(Map<String, dynamic> json) => RosterRecord(
    key: json['key'] as String,
    definition: json['definition'] as Map<String, dynamic>,
  );
}

/// The records of one kind at one roster revision.
@immutable
class RosterRecords {
  const RosterRecords({
    required this.kind,
    required this.revision,
    required this.writable,
    required this.records,
  });

  final String kind;
  final String revision;

  /// Whether the roster takes edits; a configuration directory is edited by hand.
  final bool writable;
  final List<RosterRecord> records;

  factory RosterRecords.fromJson(Map<String, dynamic> json) => RosterRecords(
    kind: json['kind'] as String,
    revision: json['revision'] as String,
    writable: json['writable'] as bool,
    records: [
      for (final record in json['records'] as List<dynamic>)
        RosterRecord.fromJson(record as Map<String, dynamic>),
    ],
  );

  /// Sample records for the preview build.
  factory RosterRecords.demo(String kind) => RosterRecords(
    kind: kind,
    revision: '12',
    writable: false,
    records: switch (kind) {
      'agents' => const [
        RosterRecord(
          key: '007',
          definition: {
            'handler': 'claude',
            'model': 'opus',
            'roles': ['case-officer'],
            'priority': 5,
          },
        ),
      ],
      'operations' => const [
        RosterRecord(
          key: 'operation-alpha',
          definition: {
            'roster': ['007'],
          },
        ),
      ],
      _ => const [],
    },
  );
}

/// The source refused a roster change: the roster it would leave is not acceptable.
class RosterRefused implements Exception {
  const RosterRefused(this.reason);

  final String reason;

  @override
  String toString() => reason;
}

/// An accepted roster change: the revision it moved to, and each setting of the stored
/// `assistant` record looser than its default.
@immutable
class RosterChange {
  const RosterChange({required this.revision, this.weakened = const []});

  /// In decimal.
  final String revision;
  final List<String> weakened;

  factory RosterChange.fromJson(Map<String, dynamic> json) => RosterChange(
    revision: json['revision'] as String,
    weakened: ((json['weakened'] as List<dynamic>?) ?? const []).cast<String>(),
  );
}

const _kinds = {
  'operations': 'Operations',
  'agents': 'Agents',
  'roles': 'Agent roles',
  'covers': 'Cover policies',
  _assistantKind: "Moneypenny's settings",
};

/// The kind of Moneypenny's `[assistant]` settings: at most one record, keyed `settings`.
const _assistantKind = 'assistant';

/// The key of the one record of a family that has one, such as Moneypenny's settings.
const _singletonKey = 'settings';

/// A route's roster `kind` and the kind the gateway's roster API names.
const _routeKinds = {
  'operation': 'operations',
  'agent': 'agents',
  'role': 'roles',
  'cover': 'covers',
  'assistant': _assistantKind,
};

/// The roster every Station reads: Operations, Agents, roles, cover policies and Moneypenny's
/// settings, edited through Universal Exports. Each Station takes up an accepted edit at its
/// next poll; what an edit of her settings weakens stays shown until the next edit.
///
/// The [route] (page `roster`) names the `kind` shown (Agents when absent) and may narrow
/// the list to the record whose key is `key`.
class RosterPage extends StatefulWidget {
  const RosterPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<RosterPage> createState() => _RosterPageState();
}

class _RosterPageState extends State<RosterPage> {
  RosterRecords? _records;
  String? _error;
  bool _loading = false;

  /// What the last accepted edit weakens, as the gateway words it.
  List<String> _weakened = const [];

  /// The API kind the route names.
  String get _kind => _routeKinds[widget.route['kind'] ?? 'agent']!;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void didUpdateWidget(RosterPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['kind'] != widget.route['kind']) {
      setState(() {
        _records = null;
        _weakened = const [];
      });
      unawaited(_load());
    }
  }

  Future<void> _load() async {
    final kind = _kind;
    setState(() => _loading = true);
    try {
      final records = await widget.board.loadRoster(kind);
      if (!mounted || kind != _kind) return;
      setState(() {
        _records = records;
        _error = null;
      });
    } on StateError catch (error) {
      if (mounted) setState(() => _error = error.message);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  void _say(String text) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _edit(RosterRecord? record) async {
    final change = await showDialog<RosterChange>(
      context: context,
      builder: (context) =>
          _RecordEditor(board: widget.board, kind: _kind, record: record),
    );
    if (change == null || !mounted) return;
    setState(() => _weakened = change.weakened);
    _say(
      'Saved as roster revision ${change.revision}; each Station takes it up at its next poll.',
    );
    await _load();
  }

  Future<void> _delete(RosterRecord record) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove ${record.key}?'),
        content: const Text(
          'Every Station stops using it at its next poll. Running Sessions keep '
          'the roster they started with.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    try {
      final change = await widget.board.deleteRosterRecord(_kind, record.key);
      if (!mounted) return;
      setState(() => _weakened = change.weakened);
      _say('Removed; roster revision ${change.revision}.');
      await _load();
    } on Object catch (error) {
      if (mounted) _say(error.toString());
    }
  }

  @override
  Widget build(BuildContext context) {
    final records = _records;
    final key = widget.route['key'];
    final shown = [
      for (final record in records?.records ?? const <RosterRecord>[])
        if (key == null || record.key == key) record,
    ];
    final canEdit =
        widget.board.canManageRoster && (records?.writable ?? false);
    // Moneypenny's settings are one record: once it exists it is edited, not added.
    final canAdd =
        canEdit &&
        (_kind != _assistantKind || (records?.records.isEmpty ?? false));
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 16, 24, 8),
          child: Wrap(
            spacing: 16,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              DropdownButton<String>(
                value: _kind,
                items: [
                  for (final MapEntry(:key, :value) in _kinds.entries)
                    DropdownMenuItem(value: key, child: Text(value)),
                ],
                onChanged: (kind) {
                  if (kind == null || kind == _kind) return;
                  final singular = _routeKinds.entries
                      .firstWhere((entry) => entry.value == kind)
                      .key;
                  widget.onRoute(
                    widget.route.without('key').withField('kind', singular),
                  );
                },
              ),
              IconButton(
                tooltip: 'Refresh the roster',
                onPressed: () => unawaited(_load()),
                icon: const Icon(Icons.refresh),
              ),
              if (canAdd)
                FilledButton.icon(
                  onPressed: () => unawaited(_edit(null)),
                  icon: const Icon(Icons.add),
                  label: const Text('Add'),
                ),
            ],
          ),
        ),
        OoFilterBar(route: widget.route, onRoute: widget.onRoute),
        if (records != null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 24),
            child: _Reach(
              revision: records.revision,
              writable: records.writable,
              stations: widget.board.stationBoard.stations,
            ),
          ),
        if (_error case final error?)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 24),
            child: Text(error),
          ),
        if (_weakened.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
            child: _Weakened(_weakened),
          ),
        Expanded(
          child: switch (records) {
            null => Center(
              child: _loading
                  ? const CircularProgressIndicator()
                  : const Text('The roster is unavailable.'),
            ),
            RosterRecords(records: []) => const Center(
              child: Text('No records of this kind.'),
            ),
            _ when shown.isEmpty => Center(
              child: Text(
                'No ${_kinds[_kind]} record is keyed ${widget.route['key']}.',
              ),
            ),
            _ => ListView(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 24),
              children: [
                for (final record in shown)
                  Card(
                    child: ListTile(
                      title: Text(record.key),
                      subtitle: Text(
                        jsonEncode(record.definition),
                        maxLines: 3,
                        overflow: TextOverflow.ellipsis,
                      ),
                      trailing: canEdit
                          ? Wrap(
                              children: [
                                IconButton(
                                  tooltip: 'Edit ${record.key}',
                                  onPressed: () => unawaited(_edit(record)),
                                  icon: const Icon(Icons.edit_outlined),
                                ),
                                IconButton(
                                  tooltip: 'Remove ${record.key}',
                                  onPressed: () => unawaited(_delete(record)),
                                  icon: const Icon(Icons.delete_outline),
                                ),
                              ],
                            )
                          : null,
                    ),
                  ),
              ],
            ),
          },
        ),
      ],
    );
  }
}

/// Which Stations run this revision, and what each refused or waits to restart for.
class _Reach extends StatelessWidget {
  const _Reach({
    required this.revision,
    required this.writable,
    required this.stations,
  });

  final String revision;
  final bool writable;
  final List<StationSummary> stations;

  @override
  Widget build(BuildContext context) {
    final reporting = [
      for (final station in stations)
        if (station.config case final config?) (station.stationId, config),
    ];
    final current = reporting
        .where((entry) => entry.$2.revision == revision)
        .length;
    final small = Theme.of(context).textTheme.bodySmall;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          [
            'Roster revision $revision',
            if (reporting.isNotEmpty)
              'running on $current of ${reporting.length} Stations',
            if (!writable) 'read-only: edit the configuration files',
          ].join(' · '),
        ),
        for (final (station, config) in reporting) ...[
          for (final reason in config.refused)
            Text('$station refused: $reason', style: small),
          for (final file in config.needsRestart)
            Text('$station: $file changed; restart it to apply', style: small),
        ],
      ],
    );
  }
}

/// The settings the stored record leaves looser than their defaults, one line each, after an
/// accepted edit.
class _Weakened extends StatelessWidget {
  const _Weakened(this.lines);

  final List<String> lines;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Card(
      margin: EdgeInsets.zero,
      color: colors.errorContainer,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          spacing: 4,
          children: [
            Row(
              spacing: 8,
              children: [
                Icon(Icons.warning_amber, color: colors.onErrorContainer),
                Expanded(
                  child: Text(
                    "Moneypenny's settings now weaken these defaults:",
                    style: TextStyle(color: colors.onErrorContainer),
                  ),
                ),
              ],
            ),
            for (final line in lines)
              Text(line, style: TextStyle(color: colors.onErrorContainer)),
          ],
        ),
      ),
    );
  }
}

/// Edits one record's key and definition as a JSON object; the key of Moneypenny's settings
/// is fixed.
class _RecordEditor extends StatefulWidget {
  const _RecordEditor({
    required this.board,
    required this.kind,
    required this.record,
  });

  final MissionBoard board;
  final String kind;
  final RosterRecord? record;

  @override
  State<_RecordEditor> createState() => _RecordEditorState();
}

class _RecordEditorState extends State<_RecordEditor> {
  late final _fixedKey = widget.kind == _assistantKind;
  late final _key = TextEditingController(
    text: widget.record?.key ?? (_fixedKey ? _singletonKey : ''),
  );
  late final _definition = TextEditingController(
    text: const JsonEncoder.withIndent('  ')
        .convert(widget.record?.definition ?? const <String, dynamic>{}),
  );
  String? _problem;
  bool _saving = false;

  @override
  void dispose() {
    _key.dispose();
    _definition.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final key = _key.text.trim();
    Object? definition;
    try {
      definition = jsonDecode(_definition.text);
    } on FormatException catch (error) {
      setState(() => _problem = 'Not JSON: ${error.message}');
      return;
    }
    if (key.isEmpty || definition is! Map<String, dynamic>) {
      setState(
        () => _problem = 'Give a key and a definition that is a JSON object.',
      );
      return;
    }
    setState(() {
      _saving = true;
      _problem = null;
    });
    try {
      final change = await widget.board.saveRosterRecord(
        widget.kind,
        key,
        definition,
      );
      if (mounted) Navigator.of(context).pop(change);
    } on RosterRefused catch (refused) {
      if (mounted) setState(() => _problem = 'Refused: ${refused.reason}');
    } on Object catch (error) {
      if (mounted) setState(() => _problem = error.toString());
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(
      widget.record == null
          ? 'Add to ${_kinds[widget.kind]}'
          : 'Edit ${widget.record!.key}',
    ),
    content: SizedBox(
      width: 480,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (_fixedKey)
            const Text('Key: assistant (the one record of this kind)')
          else
            TextField(
              key: const Key('roster-key'),
              controller: _key,
              enabled: widget.record == null && !_saving,
              decoration: const InputDecoration(labelText: 'Key'),
            ),
          const SizedBox(height: 12),
          TextField(
            key: const Key('roster-definition'),
            controller: _definition,
            enabled: !_saving,
            minLines: 6,
            maxLines: 16,
            decoration: const InputDecoration(
              labelText: 'Definition (JSON)',
              helperText: 'The keys of its table in the configuration files',
            ),
          ),
          if (_problem case final problem?) ...[
            const SizedBox(height: 12),
            Text(
              problem,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          ],
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
        child: Text(_saving ? 'Saving…' : 'Save'),
      ),
    ],
  );
}
