import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_route.dart';
import '../shell/oo_empty.dart';
import '../shell/oo_freshness.dart';
import '../shell/oo_link.dart';
import '../shell/operation_names.dart';
import '../theme/oo_icon.dart';
import '../timeline/timeline_page.dart';
import '../widgets/oo_data_table.dart';
import '../widgets/oo_principals.dart';

/// The actor of a record as a facet value: the person, apart from any key they acted through.
String auditActor(AuditEntrySummary record) =>
    OoPrincipalRef.parse(record.actor).subject;

class AuditLogPage extends StatefulWidget {
  const AuditLogPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;

  /// An Audit route: its `operation_id` or `station_id` keeps only that scope's records, and its
  /// table fields narrow and sort them.
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<AuditLogPage> createState() => _AuditLogPageState();
}

class _AuditLogPageState extends State<AuditLogPage> {
  late Future<List<AuditEntrySummary>> _records;

  @override
  void initState() {
    super.initState();
    _records = widget.board.loadAudit();
  }

  void _refresh() => setState(() => _records = widget.board.loadAudit());

  bool _selected(AuditEntrySummary record) {
    if (widget.route['operation_id'] case final operation?) {
      return record.operation == operation;
    }
    if (widget.route['station_id'] case final station?) {
      return record.resource == 'station:$station';
    }
    return true;
  }

  List<OoColumn<AuditEntrySummary>> _columns(OoPrincipals principals) => [
    OoColumn(
      id: 'when',
      label: 'When',
      width: 150,
      sortKey: (record) => record.at,
      cell: (context, record) => OoTime(record.at),
    ),
    OoColumn(
      id: 'actor',
      label: 'Actor',
      flex: 2,
      sortKey: (record) =>
          ooPrincipalText(principals, record.actor).toLowerCase(),
      cell: (context, record) =>
          OoPrincipalBadge(record.actor, knownName: record.actorName),
    ),
    OoColumn(
      id: 'action',
      label: 'Action',
      flex: 2,
      sortKey: (record) => record.action,
      cell: (context, record) => Tooltip(
        message: record.action,
        child: Text(
          timelineSentence(record.action),
          overflow: TextOverflow.ellipsis,
        ),
      ),
    ),
    OoColumn(
      id: 'scope',
      label: 'Scope and target',
      flex: 3,
      sortKey: (record) => auditPlace(record).$1,
      cell: (context, record) =>
          OoLinkLine([auditPlace(record), (record.target, null)]),
    ),
  ];

  bool get _scoped =>
      widget.route['operation_id'] != null ||
      widget.route['station_id'] != null;

  List<Widget> _scopeChips() => [
    for (final key in ['operation_id', 'station_id'])
      if (widget.route[key] != null)
        InputChip(
          key: ValueKey('filter-$key'),
          label: Text(routeScopeLabel(widget.route, key)),
          onDeleted: () => widget.onRoute(widget.route.without(key)),
          deleteButtonTooltipMessage: 'Remove this filter',
        ),
  ];

  Widget _empty() => OoEmptyState(
    icon: OoIconName.audit,
    title: _scoped
        ? 'No audit records match these filters.'
        : 'No audit records yet.',
    body: _scoped
        ? 'Remove a filter above to see more.'
        : 'Every change a person or Agent makes through AI6 is recorded here.',
  );

  @override
  Widget build(BuildContext context) {
    final principals = widget.board.principals;
    return ListenableBuilder(
      listenable: principals,
      builder: (context, _) => _body(context, principals),
    );
  }

  Widget _body(BuildContext context, OoPrincipals principals) {
    return Column(
      children: [
        Expanded(
          child: FutureBuilder<List<AuditEntrySummary>>(
            future: _records,
            builder: (context, snapshot) {
              if (snapshot.hasError) {
                return OoEmptyState(
                  icon: OoIconName.audit,
                  title: 'Audit records could not be read.',
                  body: '${snapshot.error}',
                  action: ('Try again', _refresh),
                );
              }
              if (!snapshot.hasData) {
                return const Center(child: CircularProgressIndicator());
              }
              final records = snapshot.requireData.where(_selected).toList();
              if (records.isEmpty) {
                return Column(
                  children: [
                    Padding(
                      padding: const EdgeInsets.fromLTRB(20, 8, 20, 0),
                      child: Wrap(spacing: 8, children: _scopeChips()),
                    ),
                    Expanded(child: _empty()),
                  ],
                );
              }
              return OoDataTable<AuditEntrySummary>(
                rows: records,
                rowId: (record) => record.id,
                columns: _columns(principals),
                route: widget.route,
                onRoute: widget.onRoute,
                noun: 'records',
                defaultSort: ('when', false),
                searchText: (record) =>
                    '${ooPrincipalText(principals, record.actor)} '
                    '${record.actor} ${record.action} '
                    '${timelineSentence(record.action)} '
                    '${record.operation} ${OperationNames.of(record.operation)} '
                    '${record.target} '
                    '${record.resource ?? ''}',
                facets: [
                  OoFacet(
                    id: 'actor',
                    label: 'Actor',
                    values: (record) => [auditActor(record)],
                    valueLabel: (value) => ooPrincipalText(principals, value),
                  ),
                  OoFacet(
                    id: 'action',
                    label: 'Action',
                    values: (record) => [record.action],
                    valueLabel: timelineSentence,
                  ),
                ],
                leading: [
                  ..._scopeChips(),
                  IconButton(
                    tooltip: 'Refresh audit log',
                    onPressed: _refresh,
                    icon: const Icon(Icons.refresh),
                  ),
                ],
              );
            },
          ),
        ),
      ],
    );
  }
}

/// `Operation: G` or `Station: hq` for the scope an Audit route is limited to.
String routeScopeLabel(OoRoute route, String key) => key == 'operation_id'
    ? 'Operation: ${OperationNames.of(route[key]!)}'
    : 'Station: ${route[key]}';
