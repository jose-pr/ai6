import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;
import 'package:outer_office/widgets/oo_data_table.dart';
import 'package:shared_preferences/shared_preferences.dart';

class _Agent {
  const _Agent(this.name, this.handler, this.done, {this.roles = const []});

  final String name;
  final String handler;
  final int? done;
  final List<String> roles;
}

const _agents = [
  _Agent('007', 'claude', 12, roles: ['case-officer']),
  _Agent('008', 'codex', 3, roles: ['reviewer', 'case-officer']),
  _Agent('0011', 'claude', null, roles: ['planner']),
  _Agent('006', 'acp', 7),
];

final _columns = <OoColumn<_Agent>>[
  OoColumn(
    id: 'agent',
    label: 'Agent',
    sortKey: (row) => row.name,
    cell: (context, row) => Text(row.name),
  ),
  OoColumn(
    id: 'handler',
    label: 'Handler',
    sortKey: (row) => row.handler,
    cell: (context, row) => Text(row.handler),
  ),
  OoColumn(
    id: 'done',
    label: 'Done',
    sortKey: (row) => row.done,
    cell: (context, row) => Text('${row.done ?? '-'} done'),
  ),
  OoColumn(
    id: 'roles',
    label: 'Roles',
    defaultVisible: false,
    cell: (context, row) => Text(row.roles.join(' ')),
  ),
];

final _facets = <OoFacet<_Agent>>[
  OoFacet(id: 'handler', label: 'Handler', values: (row) => [row.handler]),
];

/// A table of [_agents] whose route is kept the way a page keeps it.
class _Harness extends StatefulWidget {
  const _Harness({
    this.opened,
    this.bulk = const [],
    this.route = const OoRoute(OoPage.agents),
    this.onChange,
  });

  final void Function(_Agent)? opened;
  final List<OoBulkAction<_Agent>> bulk;
  final OoRoute route;
  final ValueChanged<OoRoute>? onChange;

  @override
  State<_Harness> createState() => _HarnessState();
}

class _HarnessState extends State<_Harness> {
  late OoRoute route = widget.route;

  @override
  Widget build(BuildContext context) => MaterialApp(
    home: Scaffold(
      body: OoDataTable<_Agent>(
        rows: _agents,
        rowId: (row) => row.name,
        columns: _columns,
        facets: _facets,
        route: route,
        onRoute: (next) {
          widget.onChange?.call(next);
          setState(() => route = next);
        },
        searchText: (row) =>
            '${row.name} ${row.handler} ${row.roles.join(' ')}',
        bulkActions: widget.bulk,
        onOpen: widget.opened,
        noun: 'agents',
      ),
    ),
  );
}

/// The Agents the table lists, top to bottom.
List<String> _listed(WidgetTester tester) {
  final rows = [
    for (final row in tester.widgetList<Material>(
      find.byWidgetPredicate(
        (widget) =>
            widget.key is ValueKey &&
            '${(widget.key! as ValueKey).value}'.startsWith('row-'),
      ),
    ))
      '${(row.key! as ValueKey).value}'.substring('row-'.length),
  ];
  return rows;
}

Future<void> _show(WidgetTester tester, Widget table) async {
  SharedPreferences.setMockInitialValues({});
  tester.view
    ..physicalSize = const Size(1400, 900)
    ..devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  OoTablePreferences.density.value = OoDensity.comfortable;
  await tester.pumpWidget(table);
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'a header sorts ascending, then descending, then back to the default',
    (tester) async {
      final routes = <OoRoute>[];
      await _show(tester, _Harness(onChange: routes.add));
      expect(_listed(tester), ['007', '008', '0011', '006']);

      await tester.tap(find.byKey(const ValueKey('header-agent')));
      await tester.pumpAndSettle();
      expect(_listed(tester), ['0011', '006', '007', '008']);
      expect(routes.last.toUri(), 'app:/agents?sort=agent&order=asc');

      await tester.tap(find.byKey(const ValueKey('header-agent')));
      await tester.pumpAndSettle();
      expect(_listed(tester), ['008', '007', '006', '0011']);
      expect(routes.last['order'], 'desc');

      await tester.tap(find.byKey(const ValueKey('header-agent')));
      await tester.pumpAndSettle();
      expect(_listed(tester), ['007', '008', '0011', '006']);
      expect(
        routes.last.toUri(),
        'app:/agents',
        reason: 'the sort is in the link',
      );
    },
  );

  testWidgets(
    'a number sorts as a number and a missing one comes last either way',
    (tester) async {
      await _show(tester, const _Harness());
      await tester.tap(find.byKey(const ValueKey('header-done')));
      await tester.pumpAndSettle();
      expect(_listed(tester), ['008', '006', '007', '0011']);
      await tester.tap(find.byKey(const ValueKey('header-done')));
      await tester.pumpAndSettle();
      expect(_listed(tester), ['007', '006', '008', '0011']);
    },
  );

  testWidgets('a table opens on the sort and the filters its link carries', (
    tester,
  ) async {
    await _show(
      tester,
      _Harness(
        route: OoRoute.parseUri(
          'app:/agents?handler=claude,codex&sort=agent&order=desc',
        ),
      ),
    );
    expect(_listed(tester), ['008', '007', '0011']);
    expect(find.text('Handler: claude, codex'), findsOneWidget);
  });

  testWidgets('a facet keeps several values at once and takes them off again', (
    tester,
  ) async {
    final routes = <OoRoute>[];
    await _show(tester, _Harness(onChange: routes.add));

    await tester.tap(find.byKey(const ValueKey('add-filter')));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(SubmenuButton, 'Handler'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('facet-handler-claude')).first);
    await tester.pumpAndSettle();
    expect(_listed(tester), ['007', '0011']);

    // The filter is a chip now; its checklist takes more values.
    await tester.tapAt(const Offset(5, 500));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('facet-chip-handler')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('facet-handler-acp')).first);
    await tester.pumpAndSettle();
    expect(_listed(tester), ['007', '0011', '006']);
    expect(routes.last['handler'], 'claude,acp');
    expect(find.byKey(const ValueKey('table-count')), findsOneWidget);
    expect(find.text('3 of 4 agents'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('facet-handler-claude')).first);
    await tester.pumpAndSettle();
    expect(_listed(tester), ['006']);

    await tester.tapAt(const Offset(5, 500));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('clear-filters')));
    await tester.pumpAndSettle();
    expect(_listed(tester).length, 4);
  });

  testWidgets('a facet value of a row that holds several matches any of them', (
    tester,
  ) async {
    final facets = <OoFacet<_Agent>>[
      OoFacet(id: 'handler', label: 'Role', values: (row) => row.roles),
    ];
    final narrowed = narrowRows<_Agent>(
      rows: _agents,
      route: OoRoute.parseUri('app:/agents?handler=reviewer,planner'),
      searchText: (row) => row.name,
      columns: _columns,
      facets: facets,
    );
    expect([for (final row in narrowed) row.name], ['008', '0011']);
  });

  testWidgets('search takes every word, in any order, and goes to the link', (
    tester,
  ) async {
    final routes = <OoRoute>[];
    await _show(tester, _Harness(onChange: routes.add));
    await tester.enterText(
      find.byKey(const ValueKey('table-search')),
      'REVIEWER codex',
    );
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pumpAndSettle();
    expect(_listed(tester), ['008']);
    expect(routes.last['text'], 'REVIEWER codex');

    await tester.enterText(
      find.byKey(const ValueKey('table-search')),
      'nobody',
    );
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(find.text('No agents match these filters.'), findsOneWidget);
  });

  testWidgets(
    'slash moves to the search unless a field already has the keyboard',
    (tester) async {
      await _show(tester, const _Harness());
      await tester.sendKeyEvent(LogicalKeyboardKey.slash);
      await tester.pump();
      final search = tester.widget<TextField>(
        find.byKey(const ValueKey('table-search')),
      );
      expect(search.focusNode!.hasFocus, isTrue);
    },
  );

  testWidgets('All Operations shows one row for an Agent read in several', (
    tester,
  ) async {
    final reads = [
      ('guatiao', _agents[0]),
      ('guatiao', _agents[1]),
      ('lineupmux', _agents[0]),
      ('lineupmux', _agents[3]),
    ];
    final grouped = groupByEntity(reads, (row) => row.name);
    expect([for (final one in grouped) one.row.name], ['007', '008', '006']);
    expect(grouped.first.operations, ['guatiao', 'lineupmux']);

    await _show(
      tester,
      MaterialApp(
        home: Scaffold(
          body: OoDataTable<OoGrouped<_Agent>>(
            rows: grouped,
            rowId: (row) => row.row.name,
            columns: [
              OoColumn(
                id: 'agent',
                label: 'Agent',
                cell: (context, row) => Text(row.row.name),
              ),
              OoColumn(
                id: 'operations',
                label: 'Operations',
                cell: (context, row) => OoOperationBadges(row.operations),
              ),
            ],
            route: const OoRoute(OoPage.agents),
            onRoute: (_) {},
            searchText: (row) => row.row.name,
          ),
        ),
      ),
    );
    expect(
      find.text('007'),
      findsOneWidget,
      reason: 'once, not once per Operation',
    );
    expect(find.text('guatiao'), findsNWidgets(2));
    expect(find.text('lineupmux'), findsNWidgets(2));
  });

  testWidgets('arrows move the highlight, space selects and enter opens', (
    tester,
  ) async {
    final opened = <String>[];
    final done = <List<String>>[];
    await _show(
      tester,
      _Harness(
        opened: (row) => opened.add(row.name),
        bulk: [
          OoBulkAction(
            label: 'Hold',
            onSelected: (rows) => done.add([for (final row in rows) row.name]),
          ),
        ],
      ),
    );
    await tester.tap(find.byKey(const ValueKey('row-007')));
    await tester.pumpAndSettle();
    expect(opened, ['007'], reason: 'a tap opens a row');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyJ);
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await tester.pumpAndSettle();
    expect(find.text('2 selected'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    expect(opened, ['007', '008']);

    await tester.tap(find.text('Hold'));
    await tester.pumpAndSettle();
    expect(done.single, ['008', '0011']);
    expect(
      find.byKey(const ValueKey('bulk-bar')),
      findsNothing,
      reason: 'cleared once done',
    );
  });

  testWidgets('the header checkbox selects every row that is shown', (
    tester,
  ) async {
    await _show(
      tester,
      _Harness(
        route: OoRoute.parseUri('app:/agents?handler=claude'),
        bulk: [OoBulkAction(label: 'Hold', onSelected: (rows) {})],
      ),
    );
    await tester.tap(find.byKey(const ValueKey('select-all')));
    await tester.pumpAndSettle();
    expect(find.text('2 selected'), findsOneWidget);
    await tester.tap(find.text('Clear selection'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('bulk-bar')), findsNothing);
  });

  testWidgets('density, columns and the view are the person\'s to change', (
    tester,
  ) async {
    await _show(tester, const _Harness());
    expect(
      find.text('case-officer'),
      findsNothing,
      reason: 'Roles starts hidden',
    );
    await tester.tap(find.byKey(const ValueKey('columns')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Roles'));
    await tester.pumpAndSettle();
    expect(find.text('case-officer'), findsOneWidget);

    await tester.tap(find.text('Compact'));
    await tester.pumpAndSettle();
    expect(OoTablePreferences.density.value, OoDensity.compact);
    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getString('oo.table_density'), 'compact');

    await tester.tap(find.byKey(const ValueKey('view-mode')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('card-007')), findsOneWidget);
    expect(find.byKey(const ValueKey('header-agent')), findsNothing);
  });

  testWidgets('a narrow window shows cards by itself', (tester) async {
    SharedPreferences.setMockInitialValues({});
    tester.view
      ..physicalSize = const Size(500, 900)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(const _Harness());
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('card-007')), findsOneWidget);
  });

  testWidgets('a preset narrows to its view and shows as chosen', (
    tester,
  ) async {
    await _show(tester, MaterialApp(home: Scaffold(body: _PresetHarness())));
    expect(_listed(tester).length, 4);
    await tester.tap(find.byKey(const ValueKey('preset-Claude')));
    await tester.pumpAndSettle();
    expect(_listed(tester), ['007', '0011']);
    expect(
      tester
          .widget<ChoiceChip>(find.byKey(const ValueKey('preset-Claude')))
          .selected,
      isTrue,
    );
  });
}

class _PresetHarness extends StatefulWidget {
  @override
  State<_PresetHarness> createState() => _PresetHarnessState();
}

class _PresetHarnessState extends State<_PresetHarness> {
  OoRoute route = const OoRoute(OoPage.agents);

  @override
  Widget build(BuildContext context) => OoDataTable<_Agent>(
    rows: _agents,
    rowId: (row) => row.name,
    columns: _columns,
    facets: _facets,
    route: route,
    onRoute: (next) => setState(() => route = next),
    searchText: (row) => row.name,
    presets: [
      OoPreset(
        label: 'Claude',
        apply: (route) => route.withField('handler', ['claude']),
        active: (route) => route.listOf('handler').join() == 'claude',
      ),
    ],
  );
}
