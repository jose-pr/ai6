import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';
import '../routes/oo_route.dart';
import '../shell/oo_empty.dart';
import '../shell/oo_freshness.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import '../widgets/oo_data_table.dart';
import '../widgets/oo_principals.dart';
import 'report_detail.dart';
import 'reports_model.dart';

/// The order a Report moves through, for sorting by state.
/// The page width from which the selected Report sits beside the list.
const _splitFrom = 1000.0;

const _stateOrder = ['open', 'assessed', 'closed'];

/// The Reports page: one row per Report across the Operations and Stations this person reads,
/// with its filters, and the selected Report beside the list (under it on a narrow screen).
class ReportsPage extends StatefulWidget {
  const ReportsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;

  /// A Reports route: `report_id` selects one, and its table fields narrow and sort the list.
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<ReportsPage> createState() => _ReportsPageState();
}

class _ReportsPageState extends State<ReportsPage> {
  late Future<ReportsListing> _listing = widget.board.loadReports();

  void _refresh() {
    setState(() {
      _listing = widget.board.loadReports();
    });
  }

  @override
  Widget build(BuildContext context) {
    final principals = widget.board.principals;
    return ListenableBuilder(
      listenable: principals,
      builder: (context, _) => FutureBuilder<ReportsListing>(
        future: _listing,
        builder: (context, snapshot) {
          if (snapshot.hasError) {
            return OoEmptyState(
              icon: OoIconName.reports,
              title: 'Reports could not be read.',
              body: '${snapshot.error}',
              action: ('Try again', _refresh),
            );
          }
          final listing = snapshot.data;
          if (listing == null) {
            return const Center(child: CircularProgressIndicator());
          }
          return _body(context, listing, principals);
        },
      ),
    );
  }

  void _select(ReportSummary report) =>
      widget.onRoute(widget.route.withField('report_id', report.id));

  void _closeDetail() => widget.onRoute(widget.route.without('report_id'));

  Widget _body(
    BuildContext context,
    ReportsListing listing,
    OoPrincipals principals,
  ) {
    final selected = widget.route['report_id'];
    return LayoutBuilder(
      builder: (context, box) {
        final split = box.maxWidth >= _splitFrom;
        final detail = selected == null
            ? null
            : ReportDetail(
                key: ValueKey(selected),
                board: widget.board,
                reportId: selected,
                priorities: listing.priorities,
                others: listing.reports,
                onRoute: widget.onRoute,
                onChanged: (_) => _refresh(),
                onClose: split ? null : _closeDetail,
              );
        if (!split && detail != null) return detail;
        final table = Column(
          children: [
            if (listing.unread.isNotEmpty || listing.refused > 0)
              _Unread(listing),
            Expanded(child: _table(context, listing, principals)),
          ],
        );
        if (!split) return table;
        return Row(
          children: [
            Expanded(flex: 3, child: table),
            VerticalDivider(width: 1, color: OoTokens.of(context).border),
            Expanded(
              flex: 2,
              child:
                  detail ??
                  Center(
                    child: Text(
                      'Select a Report to read it.',
                      style: TextStyle(color: OoTokens.of(context).muted),
                    ),
                  ),
            ),
          ],
        );
      },
    );
  }

  Comparable<Object>? _priorityKey(
    ReportSummary report,
    ReportsListing listing,
  ) {
    final at = listing.priorities.indexOf(report.priority ?? '');
    return at < 0 ? null : at;
  }

  List<OoColumn<ReportSummary>> _columns(
    ReportsListing listing,
    OoPrincipals principals,
  ) => [
    OoColumn(
      id: 'priority',
      label: 'Priority',
      width: 96,
      sortKey: (report) => _priorityKey(report, listing),
      cell: (context, report) => Text(report.priority ?? ''),
    ),
    OoColumn(
      id: 'title',
      label: 'Report',
      flex: 4,
      sortKey: (report) => report.title?.toLowerCase(),
      cell: (context, report) => Tooltip(
        message: report.id,
        child: Text(
          revealInvisible(
            report.title ?? 'Marked ${report.classification ?? ''}'.trim(),
          ),
          overflow: TextOverflow.ellipsis,
        ),
      ),
    ),
    OoColumn(
      id: 'operation',
      label: 'Operation',
      flex: 2,
      sortKey: (report) => reportScopeLabel(report.scope).toLowerCase(),
      cell: (context, report) =>
          Text(reportScopeLabel(report.scope), overflow: TextOverflow.ellipsis),
    ),
    OoColumn(
      id: 'state',
      label: 'State',
      width: 100,
      sortKey: (report) => _stateOrder.indexOf(report.state ?? ''),
      cell: (context, report) => Text(report.state ?? ''),
    ),
    OoColumn(
      id: 'source',
      label: 'Filed by',
      flex: 3,
      sortKey: (report) => reportFilerWords(report, principals).toLowerCase(),
      cell: (context, report) => Text(
        report.whole
            ? '${reportFilerWords(report, principals)} · ${report.reliability ?? 'F'}'
            : '',
        overflow: TextOverflow.ellipsis,
      ),
    ),
    OoColumn(
      id: 'updated',
      label: 'Updated',
      width: 120,
      sortKey: (report) => report.updated,
      cell: (context, report) => OoTime(report.updated),
    ),
  ];

  List<OoFacet<ReportSummary>> _facets(
    ReportsListing listing,
    OoPrincipals principals,
  ) => [
    OoFacet(
      id: 'operation',
      label: 'Operation',
      values: (report) => [_scopeValue(report.scope)],
      valueLabel: (value) => reportScopeLabel(_scopeOf(value)),
    ),
    OoFacet(id: 'state', label: 'State', values: (report) => [?report.state]),
    OoFacet(
      id: 'origin',
      label: 'Filed by',
      values: (report) => [report.origin],
      valueLabel: (value) => switch (value) {
        'source' => 'A Source',
        'agent' => 'An Agent',
        'charge' => 'A charge',
        'person' => 'A person',
        _ => value,
      },
    ),
    OoFacet(
      id: 'classification',
      label: 'Classification',
      values: (report) => [report.classification ?? 'unmarked'],
    ),
    OoFacet(
      id: 'report_kind',
      label: 'Kind',
      values: (report) => [?report.kind],
    ),
    OoFacet(
      id: 'priority',
      label: 'Priority',
      values: (report) => [?report.priority],
    ),
  ];

  /// A scope as a route field value: an Operation's ID, `station:ID`, or `m`.
  static String _scopeValue(String scope) => scope.startsWith('operation:')
      ? scope.substring('operation:'.length)
      : scope;

  static String _scopeOf(String value) =>
      value == 'm' || value.startsWith('station:') ? value : 'operation:$value';

  Widget _table(
    BuildContext context,
    ReportsListing listing,
    OoPrincipals principals,
  ) {
    if (listing.reports.isEmpty) {
      return OoEmptyState(
        icon: OoIconName.reports,
        title: 'No Reports yet.',
        body:
            'A Report is what an Agent, a Source or a person tells an Operation: a finding, an '
            'idea, a changed issue. They show here once one is filed.',
        action: ('Refresh', _refresh),
      );
    }
    return OoDataTable<ReportSummary>(
      rows: listing.reports,
      rowId: (report) => report.id,
      columns: _columns(listing, principals),
      route: widget.route,
      onRoute: widget.onRoute,
      onOpen: _select,
      noun: 'Reports',
      defaultSort: ('priority', true),
      searchText: (report) =>
          '${report.title ?? ''} ${report.text ?? ''} ${report.id} '
          '${reportScopeLabel(report.scope)} ${reportFilerWords(report, principals)} '
          '${report.kind ?? ''} ${report.source ?? ''}',
      facets: _facets(listing, principals),
      presets: [
        for (final (label, state) in const [
          ('Needs assessment', 'open'),
          ('Assessed', 'assessed'),
          ('Closed', 'closed'),
        ])
          OoPreset(
            label: label,
            apply: (route) => route.withField('state', [state]),
            active: (route) {
              final states = route.listOf('state');
              return states.length == 1 && states.first == state;
            },
          ),
      ],
      leading: [
        IconButton(
          tooltip: 'Refresh Reports',
          onPressed: _refresh,
          icon: const Icon(Icons.refresh),
        ),
      ],
    );
  }
}

/// A strip saying which scopes could not be read, so a short list is not taken for a whole one.
class _Unread extends StatelessWidget {
  const _Unread(this.listing);

  final ReportsListing listing;

  @override
  Widget build(BuildContext context) {
    final parts = [
      for (final MapEntry(:key, :value) in listing.unread.entries)
        '${reportScopeLabel(key)}: $value',
      if (listing.refused > 0)
        '${listing.refused} not readable by this account',
    ];
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 8, 20, 0),
      child: Text(
        'Some Reports are not listed. ${parts.join('; ')}.',
        style: Theme.of(context).textTheme.bodySmall
            ?.copyWith(color: OoTokens.of(context).muted),
      ),
    );
  }
}
