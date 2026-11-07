import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../forms/form_host.dart';
import 'record_summary.dart';
import '../main.dart';
import '../operations/operation_tree.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../shell/oo_link.dart';
import 'record_editor.dart';

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

/// One roster record: its key, its definition, the keys of its configuration table, and the
/// revision a save of it names.
@immutable
class RosterRecord {
  const RosterRecord({
    required this.key,
    required this.definition,
    this.revision = '0',
  });

  final String key;
  final Map<String, dynamic> definition;
  final String revision;

  factory RosterRecord.fromJson(Map<String, dynamic> json) => RosterRecord(
    key: json['key'] as String,
    definition: json['definition'] as Map<String, dynamic>,
    revision: json['revision'] as String? ?? '0',
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
  'reviews': 'Review policies',
  _assistantKind: "Moneypenny's settings",
};

/// The kind picker's value for every kind: a route without `kind`.
const _allKinds = '\u0000all';

/// The kind of Moneypenny's `[assistant]` settings: at most one record, keyed `settings`.
const _assistantKind = 'assistant';

/// A route's roster `kind` and the kind the gateway's roster API names.
const _routeKinds = {
  'operation': 'operations',
  'agent': 'agents',
  'role': 'roles',
  'cover': 'covers',
  'review': 'reviews',
  'assistant': _assistantKind,
};

/// The roster every Station reads: Operations, Agents, roles, cover policies and Moneypenny's
/// settings, edited through Universal Exports. Each Station takes up an accepted edit at its
/// next poll; what an edit of her settings weakens stays shown until the next edit.
///
/// The [route] (page `roster`) names the `kind` shown and may narrow the list to the record
/// whose key is `key`. With neither it shows every kind's records, each a link to its kind.
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
  RecordFormSpec? _spec;
  String? _error;
  bool _loading = false;

  /// What the last accepted edit weakens, as the gateway words it.
  List<String> _weakened = const [];

  /// Every kind's records, for All: a kind's [RosterRecords], or why it could not be read.
  Map<String, Object> _everything = const {};

  /// Whether every kind is shown: the route names no kind and no record.
  bool get _showAll =>
      widget.route['kind'] == null && widget.route['key'] == null;

  /// The API kind the route names; a key without a kind is an Agent's.
  String get _kind => _routeKinds[widget.route['kind'] ?? 'agent']!;

  /// The route's name for an API [kind].
  static String _singular(String kind) =>
      _routeKinds.entries.firstWhere((entry) => entry.value == kind).key;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void didUpdateWidget(RosterPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['kind'] != widget.route['kind'] ||
        (oldWidget.route['key'] == null) != (widget.route['key'] == null)) {
      setState(() {
        _records = null;
        _everything = const {};
        _spec = null;
        _weakened = const [];
      });
      unawaited(_load());
    }
  }

  Future<void> _load() async {
    if (_showAll) return _loadAll();
    final kind = _kind;
    setState(() => _loading = true);
    try {
      final records = await widget.board.loadRoster(kind);
      if (!mounted || kind != _kind) return;
      setState(() {
        _records = records;
        _error = null;
      });
      await _loadSpec(kind);
    } on StateError catch (error) {
      if (mounted) setState(() => _error = error.message);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _loadAll() async {
    setState(() => _loading = true);
    final kinds = _kinds.keys.toList();
    final read = await Future.wait([
      for (final kind in kinds)
        widget.board
            .loadRoster(kind)
            .then<Object>(
              (records) => records,
              onError: (Object error) =>
                  error is StateError ? error.message : '$error',
            ),
    ]);
    if (!mounted || !_showAll) return;
    setState(() {
      _everything = {
        for (final (index, kind) in kinds.indexed) kind: read[index],
      };
      _loading = false;
    });
  }

  /// The kind picker: All records first, the default, then each kind.
  Widget _picker() => ConstrainedBox(
    constraints: const BoxConstraints(maxWidth: 260),
    child: DropdownButton<String>(
      isExpanded: true,
      value: _showAll ? _allKinds : _kind,
      items: [
        const DropdownMenuItem(value: _allKinds, child: Text('All records')),
        for (final MapEntry(:key, :value) in _kinds.entries)
          DropdownMenuItem(value: key, child: Text(value)),
      ],
      onChanged: (kind) {
        if (kind == null || kind == (_showAll ? _allKinds : _kind)) return;
        final route = widget.route.without('key');
        widget.onRoute(
          kind == _allKinds
              ? route.without('kind')
              : route.withField('kind', _singular(kind)),
        );
      },
    ),
  );

  /// Every kind under its heading, each record a link to it in its kind's view.
  Widget _buildAll(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(24, 16, 24, 8),
        child: Wrap(
          spacing: 16,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            _picker(),
            IconButton(
              tooltip: 'Refresh the roster',
              onPressed: () => unawaited(_load()),
              icon: const Icon(Icons.refresh),
            ),
          ],
        ),
      ),
      Expanded(
        child: _everything.isEmpty
            ? Center(
                child: _loading
                    ? const CircularProgressIndicator()
                    : const Text('The roster is unavailable.'),
              )
            : ListView(
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 24),
                children: [
                  for (final MapEntry(key: kind, value: read)
                      in _everything.entries) ...[
                    Padding(
                      padding: const EdgeInsets.fromLTRB(8, 16, 8, 4),
                      child: Row(
                        children: [
                          Expanded(
                            child: Text(switch (read) {
                              RosterRecords(:final records) =>
                                '${_kinds[kind]} (${records.length})',
                              _ => _kinds[kind]!,
                            }, style: Theme.of(context).textTheme.titleSmall),
                          ),
                          TextButton(
                            onPressed: () => widget.onRoute(
                              widget.route.withField('kind', _singular(kind)),
                            ),
                            child: Text('Open ${_kinds[kind]}'),
                          ),
                        ],
                      ),
                    ),
                    switch (read) {
                      RosterRecords(records: []) => const Padding(
                        padding: EdgeInsets.symmetric(horizontal: 8),
                        child: Text('No records of this kind.'),
                      ),
                      RosterRecords(:final records) => Card(
                        child: Column(
                          children: [
                            for (final record in records)
                              ListTile(
                                dense: true,
                                title: Text(record.key),
                                onTap: () => widget.onRoute(
                                  widget.route
                                      .withField('kind', _singular(kind))
                                      .withField('key', record.key),
                                ),
                              ),
                          ],
                        ),
                      ),
                      final why => Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        child: Text('$why'),
                      ),
                    },
                  ],
                ],
              ),
      ),
    ],
  );

  /// The form of this kind, which rows are summarized by; without it they say how much they
  /// hold.
  Future<void> _loadSpec(String kind) async {
    try {
      final spec = await widget.board.loadRecordForm(kind);
      if (mounted && kind == _kind) setState(() => _spec = spec);
    } on Object {
      if (mounted && kind == _kind) setState(() => _spec = null);
    }
  }

  String _summary(RosterRecord record) {
    final spec = _spec;
    if (spec != null) {
      final lines = summaryOf(spec.schema, record.definition);
      if (lines.isNotEmpty) return lines.join(' · ');
    }
    final count = record.definition.length;
    return count == 0
        ? 'Every setting at its default'
        : '$count setting${count == 1 ? '' : 's'}';
  }

  /// A row's name, a link to what it names. A cover policy and Moneypenny's settings have no
  /// page of their own. A roster record belongs to no Operation, so an Agent or role opens on
  /// the first Operation the person may read.
  Widget _name(String key) => switch (_kind) {
    'agents' => OoLink(
      key,
      route: OoLinkTo.agent(key),
      semanticLabel: 'Open Agent $key',
    ),
    'operations' => OoLink(
      key,
      route: OoLinkTo.operation(key),
      semanticLabel: 'Open Operation $key',
    ),
    'roles' => OoLink(
      key,
      route: OoLinkTo.role(key),
      semanticLabel: 'Open role $key',
    ),
    _ => Text(key),
  };

  void _say(String text) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _edit(RosterRecord? record) async {
    final change = await showRecordEditor(
      context,
      board: widget.board,
      kind: _kind,
      recordKey: record?.key,
    );
    if (change == null || !mounted) return;
    setState(() => _weakened = change.weakened);
    _say(
      'Saved as roster revision ${change.revision}; each Station takes it up at its next poll.',
    );
    await _load();
  }

  Future<void> _showEffective(String operationId) => showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text('Effective policy of $operationId'),
      content: SizedBox(
        width: 520,
        child: FutureBuilder<EffectiveOperationSummary>(
          future: widget.board.effectiveOperation(operationId),
          builder: (context, snapshot) => switch (snapshot) {
            AsyncSnapshot(data: final effective?) => SingleChildScrollView(
              child: EffectivePolicyView(effective: effective),
            ),
            AsyncSnapshot(hasError: true) => Text(
              snapshot.error is StateError
                  ? (snapshot.error! as StateError).message
                  : snapshot.error.toString(),
            ),
            _ => const Center(child: CircularProgressIndicator()),
          },
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Close'),
        ),
      ],
    ),
  );

  Future<void> _showConfiguration(RosterRecord record) async {
    final spec = _spec ?? await widget.board.loadRecordForm(_kind);
    if (!mounted) return;
    await showRecordConfiguration(
      context,
      kind: _kind,
      recordKey: record.key,
      definition: record.definition,
      schema: FieldSchema(spec.schema),
    );
  }

  Future<void> _delete(RosterRecord record) async {
    try {
      final change = await confirmRemoveRecord(
        context,
        board: widget.board,
        kind: _kind,
        recordKey: record.key,
        revision: record.revision,
      );
      if (change == null || !mounted) return;
      setState(() => _weakened = change.weakened);
      _say('Removed; roster revision ${change.revision}.');
      await _load();
    } on RosterRefused catch (refused) {
      if (mounted) _say('Refused: ${refused.reason}');
    } on RosterChanged {
      if (mounted) {
        _say('Someone changed this record meanwhile; nothing was removed.');
        await _load();
      }
    } on Object catch (error) {
      if (mounted) _say(error is StateError ? error.message : error.toString());
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_showAll) return _buildAll(context);
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
              _picker(),
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
                      title: _name(record.key),
                      subtitle: Text(
                        _summary(record),
                        maxLines: 3,
                        overflow: TextOverflow.ellipsis,
                      ),
                      trailing: Wrap(
                        children: [
                          IconButton(
                            tooltip: 'As configuration: ${record.key}',
                            onPressed: () =>
                                unawaited(_showConfiguration(record)),
                            icon: const Icon(Icons.description_outlined),
                          ),
                          if (_kind == 'operations')
                            IconButton(
                              tooltip: 'Effective policy of ${record.key}',
                              onPressed: () =>
                                  unawaited(_showEffective(record.key)),
                              icon: const Icon(Icons.account_tree),
                            ),
                          if (canEdit) ...[
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
                        ],
                      ),
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
