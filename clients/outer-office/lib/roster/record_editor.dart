import 'dart:async';
import 'dart:convert';

import 'package:material_ui/material_ui.dart';
import 'package:toml/toml.dart';

import '../forms/form_host.dart';
import '../main.dart';
import '../operations/operation_tree.dart';
import 'roster_page.dart';

/// What the gateway's roster route says about one kind of record, for a form and a summary.
@immutable
class RecordFormSpec {
  const RecordFormSpec({
    required this.schema,
    required this.form,
    required this.keyTitle,
    required this.keyDescription,
    this.fixedKey,
  });

  final Map<String, Object?> schema;
  final Map<String, Object?> form;

  /// What a record's key is called, and what it is for.
  final String keyTitle;
  final String keyDescription;

  /// The one key of a family with one record; a person does not choose it.
  final String? fixedKey;

  factory RecordFormSpec.fromJson(Map<String, dynamic> json) {
    final key = json['key'] as Map<String, dynamic>;
    return RecordFormSpec(
      schema: json['schema'] as Map<String, Object?>,
      form: json['form'] as Map<String, Object?>,
      keyTitle: key['title'] as String,
      keyDescription: key['description'] as String? ?? '',
      fixedKey: key['fixed'] as String?,
    );
  }
}

/// A change another person made first: the record no longer has the revision read.
class RosterChanged implements Exception {
  const RosterChanged();
}

/// The word the roster's files file a kind's table under.
const _tables = {
  'operations': 'operation',
  'agents': 'agent',
  'roles': 'role',
  'covers': 'cover',
  'reviews': 'review',
};

/// The kinds of record a form may name, and a person's word for one of each.
const _recordNames = {
  'operations': 'Operation',
  'agents': 'Agent',
  'roles': 'Role',
  'covers': 'Cover policy',
  'reviews': 'Review policy',
  'assistant': "Moneypenny's settings",
};

const _recordFormat = 'record:';
const _reportedFormat = 'reported:';
const _usedFormat = 'used:';

/// Reads what a form of `kind` may offer: the other records, and what the Stations report.
Future<ChoicesFor> _choicesFor(
  MissionBoard board,
  String kind,
  String? editing,
  RecordFormSpec spec,
) async {
  final kinds = <String>{};
  void walk(FieldSchema at) {
    final format = at.format ?? at.items?.format;
    if (format != null && format.startsWith(_recordFormat)) {
      kinds.add(format.substring(_recordFormat.length));
    }
    final properties = at.node['properties'];
    if (properties is Map) {
      for (final key in properties.keys) {
        walk(at.member(key as String)!);
      }
    }
    final inner = at.values ?? at.memberArm;
    if (inner != null) walk(inner);
  }

  walk(FieldSchema(spec.schema));
  kinds.add(kind);
  final records = <String, List<RosterRecord>>{};
  await Future.wait(
    kinds.map((name) async {
      try {
        records[name] = (await board.loadRoster(name)).records;
      } on StateError {
        records[name] = const [];
      }
    }),
  );
  final stations = board.stationBoard.stations;
  return (format) {
    if (format.startsWith(_recordFormat)) {
      final named = format.substring(_recordFormat.length);
      final keys = [
        for (final record in records[named] ?? const []) record.key,
      ];
      if (named == kind && kind == 'operations' && editing != null) {
        return FieldChoices(
          parentChoices({
            for (final record in records[named] ?? const <RosterRecord>[])
              record.key: record.definition['parent'] as String?,
          }, editing),
        );
      }
      return FieldChoices([
        for (final key in keys)
          if (named != kind || key != editing) key,
      ]);
    }
    if (format.startsWith(_reportedFormat)) {
      final found = switch (format.substring(_reportedFormat.length)) {
        'handlers' => {for (final station in stations) ...station.handlers},
        'theaters' => {
          for (final station in stations)
            for (final theater in station.theaters) theater.id,
        },
        'stations' => {for (final station in stations) station.stationId},
        _ => const <String>{},
      };
      return FieldChoices(found.toList()..sort(), open: true);
    }
    if (format.startsWith(_usedFormat)) {
      final field = format.substring(_usedFormat.length);
      final found = {
        for (final record in records[kind] ?? const <RosterRecord>[])
          if (record.definition[field] case final String value) value,
      };
      return FieldChoices(found.toList()..sort(), open: true);
    }
    return null;
  };
}

/// Opens the form for a new record of `kind`, or for the one keyed `recordKey`. Answers the
/// accepted change.
Future<RosterChange?> showRecordEditor(
  BuildContext context, {
  required MissionBoard board,
  required String kind,
  String? recordKey,
}) {
  final narrow = MediaQuery.sizeOf(context).width < 600;
  return showDialog<RosterChange>(
    context: context,
    builder: (context) {
      final editor = _RecordEditor(
        board: board,
        kind: kind,
        recordKey: recordKey,
        narrow: narrow,
      );
      return narrow
          ? Dialog.fullscreen(child: editor)
          : Dialog(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 680),
                child: editor,
              ),
            );
    },
  );
}

class _RecordEditor extends StatefulWidget {
  const _RecordEditor({
    required this.board,
    required this.kind,
    required this.recordKey,
    required this.narrow,
  });

  final MissionBoard board;
  final String kind;
  final String? recordKey;
  final bool narrow;

  @override
  State<_RecordEditor> createState() => _RecordEditorState();
}

class _RecordEditorState extends State<_RecordEditor> {
  final _key = TextEditingController();
  RecordFormSpec? _spec;
  FormModel? _model;
  ChoicesFor? _choices;

  /// The revision of the record read, sent with a save: `0` for a record that must be new.
  String _revision = '0';
  String? _failure;
  String? _problem;
  Map<String, String> _problems = const {};
  bool _saving = false;
  bool _changed = false;

  bool get _isNew => widget.recordKey == null;

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
      final spec = await widget.board.loadRecordForm(widget.kind);
      var definition = const <String, Object?>{};
      var revision = '0';
      if (widget.recordKey case final key?) {
        final records = await widget.board.loadRoster(widget.kind);
        final record = records.records.where((record) => record.key == key);
        if (record.isEmpty) {
          throw StateError('$key is no longer in the roster.');
        }
        definition = record.single.definition;
        revision = record.single.revision;
      }
      final choices = await _choicesFor(
        widget.board,
        widget.kind,
        widget.recordKey,
        spec,
      );
      if (!mounted) return;
      final previous = _model;
      setState(() {
        _spec = spec;
        _choices = choices;
        _revision = revision;
        _model = ooFormModel(
          schema: spec.schema,
          form: spec.form,
          value: definition,
        );
        _changed = false;
        _problem = null;
        _problems = const {};
        _failure = null;
      });
      previous?.dispose();
    } on StateError catch (error) {
      if (mounted) setState(() => _failure = error.message);
    } on Object catch (error) {
      if (mounted) {
        setState(() => _failure = 'This form cannot be shown here: $error');
      }
    }
  }

  Future<void> _save() async {
    final model = _model!;
    final spec = _spec!;
    final key = spec.fixedKey ?? widget.recordKey ?? _key.text.trim();
    if (key.isEmpty) {
      setState(() => _problem = '${spec.keyTitle}: this is needed.');
      return;
    }
    final issues = <String, String>{};
    final definition = model.value(issues: issues);
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
      _changed = false;
    });
    try {
      final change = await widget.board.saveRosterRecord(
        widget.kind,
        key,
        definition,
        revision: _revision,
      );
      if (mounted) Navigator.of(context).pop(change);
    } on RosterRefused catch (refused) {
      if (!mounted) return;
      final field = model.fieldNamedIn(refused.reason);
      setState(() {
        if (field == null) {
          _problem = refused.reason;
        } else {
          _problems = {field: refused.reason};
          _problem = 'The roster refused this. See the field marked below.';
        }
      });
    } on RosterChanged {
      if (mounted) setState(() => _changed = true);
    } on Object catch (error) {
      if (mounted) {
        setState(
          () =>
              _problem = error is StateError ? error.message : error.toString(),
        );
      }
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  void _showConfiguration() {
    final model = _model;
    if (model == null) return;
    final key = _spec?.fixedKey ?? widget.recordKey ?? _key.text.trim();
    unawaited(
      showRecordConfiguration(
        context,
        kind: widget.kind,
        recordKey: key.isEmpty ? 'new' : key,
        definition: model.value(),
        schema: model.schema,
      ),
    );
  }

  String get _title {
    final name = _recordNames[widget.kind] ?? widget.kind;
    if (_spec?.fixedKey != null) return name;
    return _isNew ? 'Add $name' : 'Edit ${widget.recordKey}';
  }

  @override
  Widget build(BuildContext context) {
    final spec = _spec;
    final model = _model;
    final theme = Theme.of(context);
    return Column(
      mainAxisSize: widget.narrow ? MainAxisSize.max : MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
          child: Semantics(
            header: true,
            child: Text(_title, style: theme.textTheme.titleLarge),
          ),
        ),
        Flexible(
          child: spec == null || model == null
              ? Padding(
                  padding: const EdgeInsets.all(24),
                  child: _failure != null
                      ? Text(_failure!)
                      : const Center(child: CircularProgressIndicator()),
                )
              : SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(24, 0, 24, 12),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      if (_changed)
                        Padding(
                          padding: const EdgeInsets.only(bottom: 12),
                          child: _ChangedNotice(
                            onReload: () => unawaited(_load()),
                          ),
                        ),
                      if (_problem case final problem?)
                        Padding(
                          padding: const EdgeInsets.only(bottom: 12),
                          child: Text(
                            problem,
                            key: const Key('record-form-problem'),
                            style: TextStyle(color: theme.colorScheme.error),
                          ),
                        ),
                      if (spec.fixedKey == null && _isNew)
                        Padding(
                          padding: const EdgeInsets.only(bottom: 16),
                          child: TextField(
                            key: const Key('record-form-key'),
                            controller: _key,
                            enabled: !_saving,
                            decoration: InputDecoration(
                              labelText: '${spec.keyTitle} (required)',
                              helperText: spec.keyDescription,
                              helperMaxLines: 3,
                              border: const OutlineInputBorder(),
                            ),
                          ),
                        ),
                      OoFormView(
                        // A reloaded record starts its boxes over.
                        key: ObjectKey(model),
                        model: model,
                        choicesFor: _choices,
                        problems: _problems,
                        enabled: !_saving,
                      ),
                    ],
                  ),
                ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 4, 16, 12),
          child: Wrap(
            alignment: WrapAlignment.end,
            spacing: 8,
            runSpacing: 4,
            children: [
              if (model != null)
                TextButton(
                  onPressed: _showConfiguration,
                  child: const Text('As configuration'),
                ),
              TextButton(
                onPressed: _saving ? null : () => Navigator.of(context).pop(),
                child: const Text('Cancel'),
              ),
              FilledButton(
                onPressed: _saving || model == null
                    ? null
                    : () => unawaited(_save()),
                child: Text(_saving ? 'Saving…' : 'Save'),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _ChangedNotice extends StatelessWidget {
  const _ChangedNotice({required this.onReload});

  final VoidCallback onReload;

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
          children: [
            Text(
              'Someone changed this record after you opened it, so nothing was '
              'saved. Reload shows what it holds now and drops what you typed here.',
              style: TextStyle(color: colors.onErrorContainer),
            ),
            Align(
              alignment: Alignment.centerRight,
              child: TextButton(
                onPressed: onReload,
                child: Text(
                  'Reload',
                  style: TextStyle(color: colors.onErrorContainer),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Add, edit and remove of one kind of record, for a page that shows those records: gated as
/// the Roster page is, on `roster.manage` and a roster that takes edits, and telling where the
/// Stations stand with what was saved.
mixin RecordActions<T extends StatefulWidget> on State<T> {
  MissionBoard get recordBoard;
  String get recordKind;

  /// Asked after an accepted change, to read the page's records again.
  Future<void> recordsChanged();

  bool _takesEdits = false;

  /// The roster revision the last accepted change moved to, until it is dismissed.
  String? savedRevision;

  bool get canChangeRecords => recordBoard.canManageRoster && _takesEdits;

  Future<void> checkRecordsTakeEdits() async {
    if (!recordBoard.canManageRoster) return;
    try {
      final records = await recordBoard.loadRoster(recordKind);
      if (mounted) setState(() => _takesEdits = records.writable);
    } on StateError {
      if (mounted) setState(() => _takesEdits = false);
    }
  }

  void _saved(RosterChange change) {
    setState(() => savedRevision = change.revision);
    unawaited(recordsChanged());
  }

  Future<void> editRecord([String? key]) async {
    final change = await showRecordEditor(
      context,
      board: recordBoard,
      kind: recordKind,
      recordKey: key,
    );
    if (change != null && mounted) _saved(change);
  }

  Future<void> removeRecord(String key) async {
    final messenger = ScaffoldMessenger.of(context);
    void say(String text) =>
        messenger.showSnackBar(SnackBar(content: Text(text)));
    try {
      final records = await recordBoard.loadRoster(recordKind);
      final found = records.records.where((record) => record.key == key);
      if (!mounted) return;
      final change = await confirmRemoveRecord(
        context,
        board: recordBoard,
        kind: recordKind,
        recordKey: key,
        revision: found.isEmpty ? null : found.single.revision,
      );
      if (change != null && mounted) _saved(change);
    } on RosterRefused catch (refused) {
      say('Refused: ${refused.reason}');
    } on RosterChanged {
      say('Someone changed this record meanwhile; nothing was removed.');
    } on StateError catch (error) {
      say(error.message);
    }
  }

  /// What the last change came to, when there is one to show.
  Widget? recordReachNote() {
    final revision = savedRevision;
    if (revision == null) return null;
    return RosterReachNote(
      revision: revision,
      stations: recordBoard.stationBoard.stations,
      onDismiss: () => setState(() => savedRevision = null),
    );
  }
}

/// The record as the configuration files would hold it, for reading and copying. A secret the
/// schema marks is not shown.
Future<void> showRecordConfiguration(
  BuildContext context, {
  required String kind,
  required String recordKey,
  required Map<String, Object?> definition,
  required FieldSchema schema,
}) {
  final shown = <String, Object?>{
    for (final MapEntry(:key, :value) in definition.entries)
      if (value != null && schema.member(key)?.sensitive != true) key: value,
  };
  final table = _tables[kind];
  final file = table == null
      ? {kind: shown}
      : {
          table: {recordKey: shown},
        };
  String text;
  try {
    text = TomlDocument.fromMap(file).toString();
  } on Object {
    text = const JsonEncoder.withIndent('  ').convert(file);
  }
  return showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: const Text('As configuration'),
      content: SizedBox(
        width: 560,
        child: SingleChildScrollView(child: SelectableText(text)),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Close'),
        ),
      ],
    ),
  );
}

/// Every place another record names the record `key` of `kind`, in words.
Future<List<String>> recordDependents(
  MissionBoard board,
  String kind,
  String key,
) async {
  final format = '$_recordFormat$kind';
  final found = <String>[];
  void walk(String owner, FieldSchema at, Object? value, String title) {
    final named = at.format == format || at.items?.format == format;
    if (named && (value == key || (value is List && value.contains(key)))) {
      found.add('$owner: $title');
    }
    if (value is Map) {
      for (final MapEntry(key: name, :value) in value.entries) {
        final member = at.member(name as String) ?? at.memberArm?.member(name);
        final inner = member ?? at.values;
        if (inner != null) {
          walk(owner, inner, value, member == null ? title : member.title);
        }
      }
    }
  }

  await Future.wait(
    _recordNames.entries.map((entry) async {
      final RecordFormSpec spec;
      final RosterRecords records;
      try {
        spec = await board.loadRecordForm(entry.key);
        records = await board.loadRoster(entry.key);
      } on Object {
        return;
      }
      for (final record in records.records) {
        if (entry.key == kind && record.key == key) continue;
        walk(
          '${entry.value} ${record.key}',
          FieldSchema(spec.schema),
          record.definition,
          entry.value,
        );
      }
    }),
  );
  return found..sort();
}

/// Asks before removing the record `key` of `kind`, naming what depends on it, and removes it.
/// Answers the accepted change; a failure is thrown as the gateway words it.
Future<RosterChange?> confirmRemoveRecord(
  BuildContext context, {
  required MissionBoard board,
  required String kind,
  required String recordKey,
  String? revision,
}) async {
  final dependents = await recordDependents(board, kind, recordKey);
  if (!context.mounted) return null;
  final confirmed = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text('Remove $recordKey?'),
      content: SizedBox(
        width: 480,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            spacing: 8,
            children: [
              const Text(
                'Every Station stops using it at its next poll. Running Sessions keep '
                'the roster they started with.',
              ),
              if (dependents.isEmpty)
                const Text('Nothing else in the roster names it.')
              else ...[
                const Text(
                  'These name it, and the roster may refuse the removal:',
                ),
                for (final line in dependents) Text('• $line'),
              ],
            ],
          ),
        ),
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
  if (confirmed != true) return null;
  return board.deleteRosterRecord(kind, recordKey, revision: revision);
}

/// Where the Stations stand with a roster revision: which run it, which do not yet, and what
/// any refused or waits to restart for.
class RosterReachNote extends StatelessWidget {
  const RosterReachNote({
    required this.revision,
    required this.stations,
    this.onDismiss,
    super.key,
  });

  /// In decimal.
  final String revision;
  final List<StationSummary> stations;
  final VoidCallback? onDismiss;

  @override
  Widget build(BuildContext context) {
    final small = Theme.of(context).textTheme.bodySmall;
    final reporting = [
      for (final station in stations)
        if (station.config case final config?) (station.stationId, config),
    ];
    final applied = [
      for (final (station, config) in reporting)
        if (config.revision == revision) station,
    ];
    final waiting = [
      for (final (station, config) in reporting)
        if (config.revision != revision) station,
    ];
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(12, 8, 4, 8),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                spacing: 2,
                children: [
                  Text(
                    'Saved as roster revision $revision. '
                    'Each Station takes it up at its next poll.',
                  ),
                  if (applied.isNotEmpty)
                    Text('Running it: ${applied.join(', ')}', style: small),
                  if (waiting.isNotEmpty)
                    Text('Not yet: ${waiting.join(', ')}', style: small),
                  for (final (station, config) in reporting) ...[
                    for (final reason in config.refused)
                      Text('$station refused: $reason', style: small),
                    for (final file in config.needsRestart)
                      Text(
                        '$station: $file changed; restart it to apply',
                        style: small,
                      ),
                  ],
                ],
              ),
            ),
            if (onDismiss != null)
              IconButton(
                tooltip: 'Dismiss',
                onPressed: onDismiss,
                icon: const Icon(Icons.close),
              ),
          ],
        ),
      ),
    );
  }
}
