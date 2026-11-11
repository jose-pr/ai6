import 'dart:async';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../routes/oo_filters.dart' show routeFieldLabel;
import '../routes/oo_route.dart';
import '../shell/oo_shell.dart' show OoBreakpoints;
import '../theme/oo_tokens.dart';

/// How tall a table's rows are.
enum OoDensity {
  /// One line, 36 px: twenty rows and more on a screen.
  compact,

  /// Two lines where a table has a second, 56 px.
  comfortable,
}

/// The density a person chose, kept for every table of this device.
abstract final class OoTablePreferences {
  static const _key = 'oo.table_density';
  static const _missionsKey = 'oo.missions_table';

  /// Whether a wide Missions queue is the table rather than the cards, which show each Mission's
  /// Sessions.
  static final missionsAsTable = ValueNotifier<bool>(false);

  static Future<void> chooseMissionsView({required bool table}) async {
    missionsAsTable.value = table;
    try {
      await (await SharedPreferences.getInstance()).setBool(
        _missionsKey,
        table,
      );
    } on Object {
      // Kept for this run only.
    }
  }

  /// The density in force; changes are stored.
  static final density = ValueNotifier<OoDensity>(OoDensity.comfortable);
  static bool _loaded = false;

  /// Reads the stored density once; a device that cannot store keeps the default.
  static Future<void> load() async {
    if (_loaded) return;
    _loaded = true;
    try {
      final preferences = await SharedPreferences.getInstance();
      final stored = preferences.getString(_key);
      for (final value in OoDensity.values) {
        if (value.name == stored) density.value = value;
      }
      missionsAsTable.value = preferences.getBool(_missionsKey) ?? false;
    } on Object {
      // Not stored: the default stands.
    }
  }

  static Future<void> choose(OoDensity chosen) async {
    density.value = chosen;
    try {
      await (await SharedPreferences.getInstance()).setString(
        _key,
        chosen.name,
      );
    } on Object {
      // Kept for this run only.
    }
  }
}

/// [route] without what a table shows itself (its sort, search and [facets]), for the chips of
/// the page's own filters above it.
OoRoute withoutTableFields(OoRoute route, List<String> facets) => [
  'text',
  'sort',
  'order',
  ...facets,
].fold(route, (route, key) => route.without(key));

/// One column of an [OoDataTable]. Its [id] is what the route's `sort` names.
class OoColumn<T> {
  const OoColumn({
    required this.id,
    required this.label,
    required this.cell,
    this.sortKey,
    this.width,
    this.flex = 1,
    this.defaultVisible = true,
  });

  final String id;
  final String label;

  /// A fixed width; `null` shares what is left by [flex].
  final double? width;
  final int flex;

  /// What clicking the header sorts by; `null` is not sortable. Rows whose key is `null` come last.
  final Comparable<Object>? Function(T row)? sortKey;
  final bool defaultVisible;
  final Widget Function(BuildContext context, T row) cell;

  bool get sortable => sortKey != null;
}

/// A way to narrow the rows: the route field [id] holds the values kept, a row passing when it
/// holds any of them.
class OoFacet<T> {
  const OoFacet({
    required this.id,
    required this.label,
    required this.values,
    this.valueLabel,
  });

  /// The route field, such as `handler`.
  final String id;
  final String label;

  /// The values of a row for this facet; a row may hold several (an Agent's roles).
  final List<String> Function(T row) values;

  /// How a value reads in the checklist and on the chip; the value itself by default.
  final String Function(String value)? valueLabel;

  String labelOf(String value) => valueLabel?.call(value) ?? value;
}

/// An action on the selected rows.
class OoBulkAction<T> {
  const OoBulkAction({
    required this.label,
    required this.onSelected,
    this.icon,
    this.enabled,
  });

  final String label;
  final IconData? icon;
  final FutureOr<void> Function(List<T> rows) onSelected;

  /// Whether it applies to these rows; always when `null`.
  final bool Function(List<T> rows)? enabled;
}

/// A saved narrowing offered above the table, such as "Failed".
class OoPreset {
  const OoPreset({
    required this.label,
    required this.apply,
    required this.active,
  });

  final String label;
  final OoRoute Function(OoRoute route) apply;
  final bool Function(OoRoute route) active;
}

/// One entity read in several Operations, shown as one row.
class OoGrouped<T> {
  const OoGrouped(this.row, this.reads);

  /// The first read, which the row's own columns show.
  final T row;

  /// Every read, with the Operation it was read in.
  final List<(String operation, T row)> reads;

  List<String> get operations => [
    for (final (operation, _) in reads) operation,
  ];
}

/// One row per entity of [rows], read as `(operation, row)`: with All Operations an Agent or a
/// Pool repeated in each Operation is one row listing its Operations. Rows keep the order of
/// their first read.
List<OoGrouped<T>> groupByEntity<T>(
  Iterable<(String, T)> rows,
  String Function(T row) entity,
) {
  final grouped = <String, List<(String, T)>>{};
  for (final read in rows) {
    grouped.putIfAbsent(entity(read.$2), () => []).add(read);
  }
  return [for (final reads in grouped.values) OoGrouped(reads.first.$2, reads)];
}

/// Operation names as compact badges, with [detail] saying more (a clearance) when given.
class OoOperationBadges extends StatelessWidget {
  const OoOperationBadges(this.operations, {this.detail, super.key});

  final List<String> operations;
  final String? Function(String operation)? detail;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Wrap(
      spacing: 4,
      runSpacing: 2,
      children: [
        for (final operation in operations)
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
            decoration: BoxDecoration(
              border: Border.all(color: tokens.muted),
              borderRadius: BorderRadius.circular(10),
            ),
            child: Text(switch (detail?.call(operation)) {
              final String more => '$operation: $more',
              _ => operation,
            }, style: Theme.of(context).textTheme.labelSmall),
          ),
      ],
    );
  }
}

/// The rows [rows] show under [route]: narrowed by its text and facets, sorted by its sort.
/// What [OoDataTable] shows, apart from the widget, for tests and pages that count.
List<T> narrowRows<T>({
  required List<T> rows,
  required OoRoute route,
  required String Function(T row) searchText,
  required List<OoColumn<T>> columns,
  List<OoFacet<T>> facets = const [],
  (String, bool)? defaultSort,
}) {
  var shown = rows;
  final words = (route['text'] ?? '')
      .toLowerCase()
      .split(RegExp(r'\s+'))
      .where((word) => word.isNotEmpty)
      .toList();
  if (words.isNotEmpty) {
    shown = [
      for (final row in shown)
        if (_holdsAll(searchText(row).toLowerCase(), words)) row,
    ];
  }
  for (final facet in facets) {
    final wanted = route.listOf(facet.id).toSet();
    if (wanted.isEmpty) continue;
    shown = [
      for (final row in shown)
        if (facet.values(row).any(wanted.contains)) row,
    ];
  }
  final sort = route['sort'] ?? defaultSort?.$1;
  final column = columns.where((column) => column.id == sort).firstOrNull;
  final key = column?.sortKey;
  if (key == null) return shown;
  final ascending = route['order'] != null
      ? route['order'] == 'asc'
      : (route['sort'] != null || (defaultSort?.$2 ?? true));
  final sorted = [...shown];
  mergeSort(sorted, (a, b) => _compare(key(a), key(b), ascending));
  return sorted;
}

bool _holdsAll(String text, List<String> words) => words.every(text.contains);

int _compare(Comparable<Object>? a, Comparable<Object>? b, bool ascending) {
  if (a == null && b == null) return 0;
  // Rows with nothing to compare come last either way.
  if (a == null) return 1;
  if (b == null) return -1;
  final left = a is String ? a.toLowerCase() : a;
  final right = b is String ? b.toLowerCase() : b;
  final order = left.compareTo(right);
  return ascending ? order : -order;
}

/// A stable merge sort: rows with equal keys keep their order.
void mergeSort<E>(List<E> list, int Function(E a, E b) compare) {
  final decorated = [for (var i = 0; i < list.length; i++) (i, list[i])];
  decorated.sort((a, b) {
    final order = compare(a.$2, b.$2);
    return order != 0 ? order : a.$1.compareTo(b.$1);
  });
  for (var i = 0; i < list.length; i++) {
    list[i] = decorated[i].$2;
  }
}

/// A collection as a table: sortable columns, facet chips that hold several values, a quick
/// search, compact or comfortable rows, cards on a narrow screen, keyboard navigation, and
/// selection with bulk actions. Its state (`sort`, `order`, `text` and each facet) is in the
/// [route], so a view is a link.
class OoDataTable<T> extends StatefulWidget {
  const OoDataTable({
    required this.rows,
    required this.rowId,
    required this.columns,
    required this.route,
    required this.onRoute,
    required this.searchText,
    this.facets = const [],
    this.presets = const [],
    this.bulkActions = const [],
    this.defaultSort,
    this.onOpen,
    this.detail,
    this.empty,
    this.noun = 'rows',
    this.cardsBelow = OoBreakpoints.compact,
    this.leading = const [],
    this.narrowed = false,
    this.cardBuilder,
    super.key,
  });

  /// Whether the page already narrowed and sorted [rows] by the route (its own rules decide
  /// what a text or a sort means): the table then shows them as given.
  final bool narrowed;

  /// What a row is on a narrow screen instead of the table's card of its columns.
  final Widget Function(BuildContext context, T row)? cardBuilder;

  final List<T> rows;

  /// A row's identity, stable across reads, for selection.
  final String Function(T row) rowId;
  final List<OoColumn<T>> columns;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  /// What quick search looks in: the words of the row's own text.
  final String Function(T row) searchText;
  final List<OoFacet<T>> facets;
  final List<OoPreset> presets;
  final List<OoBulkAction<T>> bulkActions;

  /// The sort a route without `sort` has, as a column id and whether it ascends.
  final (String, bool)? defaultSort;

  /// Called by Enter, or a tap, on a row.
  final void Function(T row)? onOpen;

  /// A second line under the first column in comfortable rows.
  final Widget Function(BuildContext context, T row)? detail;

  /// Shown instead of the table when there are no rows at all; the page's own empty state.
  final Widget? empty;

  /// What the rows are, for "14 of 42 agents".
  final String noun;

  /// Cards replace the table below this width.
  final double cardsBelow;

  /// Controls placed first in the toolbar, such as an Operation picker.
  final List<Widget> leading;

  @override
  State<OoDataTable<T>> createState() => _OoDataTableState<T>();
}

class _OoDataTableState<T> extends State<OoDataTable<T>> {
  final _search = TextEditingController();
  final _searchFocus = FocusNode();
  final _bodyFocus = FocusNode(debugLabel: 'table body');
  Timer? _debounce;
  int _cursor = -1;
  final _selected = <String>{};
  final _hidden = <String>{};
  bool? _cards;

  @override
  void initState() {
    super.initState();
    _search.text = widget.route['text'] ?? '';
    _hidden.addAll([
      for (final column in widget.columns)
        if (!column.defaultVisible) column.id,
    ]);
    unawaited(OoTablePreferences.load());
    OoTablePreferences.density.addListener(_changed);
    HardwareKeyboard.instance.addHandler(_slash);
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_slash);
    OoTablePreferences.density.removeListener(_changed);
    _debounce?.cancel();
    _search.dispose();
    _searchFocus.dispose();
    _bodyFocus.dispose();
    super.dispose();
  }

  @override
  void didUpdateWidget(OoDataTable<T> oldWidget) {
    super.didUpdateWidget(oldWidget);
    final text = widget.route['text'] ?? '';
    if (_search.text.trim() != text && !_searchFocus.hasFocus) {
      _search.text = text;
    }
    final ids = {for (final row in widget.rows) widget.rowId(row)};
    _selected.removeWhere((id) => !ids.contains(id));
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  /// `/` outside a text field moves to the search.
  bool _slash(KeyEvent event) {
    if (event is! KeyDownEvent || !mounted) return false;
    if (event.logicalKey != LogicalKeyboardKey.slash) return false;
    final focused = FocusManager.instance.primaryFocus?.context;
    if (focused?.widget is EditableText ||
        focused?.findAncestorWidgetOfExactType<EditableText>() != null) {
      return false;
    }
    if (ModalRoute.of(context)?.isCurrent == false) return false;
    _searchFocus.requestFocus();
    return true;
  }

  List<T> get _shown => widget.narrowed
      ? widget.rows
      : narrowRows(
          rows: widget.rows,
          route: widget.route,
          searchText: widget.searchText,
          columns: widget.columns,
          facets: widget.facets,
          defaultSort: widget.defaultSort,
        );

  void _route(OoRoute route) {
    if (route != widget.route) widget.onRoute(route);
  }

  void _applySearch() {
    final text = _search.text.trim();
    try {
      _route(widget.route.withField('text', text.isEmpty ? null : text));
    } on FormatException {
      // A text the route refuses (too long) is not applied.
    }
  }

  void _typed(String _) {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 350), _applySearch);
  }

  void _sort(OoColumn<T> column) {
    final route = widget.route;
    final current = route['sort'] ?? widget.defaultSort?.$1;
    final ascending = route['order'] != null
        ? route['order'] == 'asc'
        : (route['sort'] != null || (widget.defaultSort?.$2 ?? true));
    if (current != column.id) {
      _route(route.withField('sort', column.id).withField('order', 'asc'));
    } else if (ascending) {
      _route(route.withField('sort', column.id).withField('order', 'desc'));
    } else if (route['sort'] == null) {
      // The default already descends: the next click is the opposite, not nothing.
      _route(route.withField('sort', column.id).withField('order', 'asc'));
    } else {
      _route(route.withField('sort', null).withField('order', null));
    }
  }

  void _toggleFacet(OoFacet<T> facet, String value, bool on) {
    final held = widget.route.listOf(facet.id).toList();
    if (on && !held.contains(value)) held.add(value);
    if (!on) held.remove(value);
    _route(widget.route.withField(facet.id, held));
  }

  List<T> get _selectedRows => [
    for (final row in widget.rows)
      if (_selected.contains(widget.rowId(row))) row,
  ];

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final shown = _shown;
    if (shown.isEmpty) return KeyEventResult.ignored;
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.arrowDown || key == LogicalKeyboardKey.keyJ) {
      setState(() => _cursor = (_cursor + 1).clamp(0, shown.length - 1));
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.arrowUp || key == LogicalKeyboardKey.keyK) {
      setState(() => _cursor = (_cursor - 1).clamp(0, shown.length - 1));
      return KeyEventResult.handled;
    }
    if (_cursor < 0 || _cursor >= shown.length) return KeyEventResult.ignored;
    if (key == LogicalKeyboardKey.space && widget.bulkActions.isNotEmpty) {
      _toggle(widget.rowId(shown[_cursor]));
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.enter && widget.onOpen != null) {
      widget.onOpen!(shown[_cursor]);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  void _toggle(String id) => setState(() {
    if (!_selected.remove(id)) _selected.add(id);
  });

  @override
  Widget build(BuildContext context) {
    if (widget.rows.isEmpty && widget.empty != null) return widget.empty!;
    final shown = _shown;
    if (_cursor >= shown.length) _cursor = shown.length - 1;
    final tokens = OoTokens.of(context);
    return LayoutBuilder(
      builder: (context, box) {
        final cards = _cards ?? box.maxWidth < widget.cardsBelow;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _toolbar(context, shown.length, cards),
            if (widget.presets.isNotEmpty) _presets(),
            if (_selected.isNotEmpty && widget.bulkActions.isNotEmpty)
              _bulkBar(tokens),
            if (!cards) _header(context, tokens, shown),
            Expanded(
              child: shown.isEmpty
                  ? Center(
                      child: Padding(
                        padding: const EdgeInsets.all(24),
                        child: Text(
                          widget.route.filtered
                              ? 'No ${widget.noun} match these filters.'
                              : 'No ${widget.noun}.',
                        ),
                      ),
                    )
                  : Focus(
                      focusNode: _bodyFocus,
                      onKeyEvent: _key,
                      child: ListView.builder(
                        key: PageStorageKey('table-${widget.noun}'),
                        padding: const EdgeInsets.fromLTRB(16, 0, 16, 24),
                        itemCount: shown.length,
                        itemBuilder: (context, index) => cards
                            ? _card(context, tokens, shown, index)
                            : _row(context, tokens, shown, index),
                      ),
                    ),
            ),
          ],
        );
      },
    );
  }

  Widget _toolbar(BuildContext context, int count, bool cards) {
    final route = widget.route;
    final active = [
      for (final facet in widget.facets)
        if (route.listOf(facet.id).isNotEmpty) facet,
    ];
    final idle = [
      for (final facet in widget.facets)
        if (route.listOf(facet.id).isEmpty) facet,
    ];
    final density = OoTablePreferences.density.value;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 4),
      child: Wrap(
        spacing: 8,
        runSpacing: 4,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          ...widget.leading,
          SizedBox(
            width: 240,
            child: TextField(
              key: const ValueKey('table-search'),
              controller: _search,
              focusNode: _searchFocus,
              onChanged: _typed,
              onSubmitted: (_) => _applySearch(),
              decoration: InputDecoration(
                isDense: true,
                prefixIcon: const Icon(Icons.search, size: 18),
                hintText: 'Search ${widget.noun} (/)',
                suffixIcon: _search.text.isEmpty
                    ? null
                    : IconButton(
                        tooltip: 'Clear search',
                        icon: const Icon(Icons.close, size: 16),
                        onPressed: () {
                          _search.clear();
                          _applySearch();
                        },
                      ),
              ),
            ),
          ),
          for (final facet in active) _facetChip(facet, selected: true),
          if (idle.isNotEmpty)
            MenuAnchor(
              builder: (context, controller, _) => TextButton.icon(
                key: const ValueKey('add-filter'),
                onPressed: () =>
                    controller.isOpen ? controller.close() : controller.open(),
                icon: const Icon(Icons.filter_list),
                label: const Text('Add filter'),
              ),
              menuChildren: [
                for (final facet in idle)
                  SubmenuButton(
                    menuChildren: _checklist(facet),
                    child: Text(facet.label),
                  ),
              ],
            ),
          if (route.filtered && !widget.narrowed)
            TextButton(
              key: const ValueKey('clear-filters'),
              onPressed: () => _route(route.cleared),
              child: const Text('Clear filters'),
            ),
          Text(
            widget.narrowed
                ? '$count ${widget.noun}'
                : '$count of ${widget.rows.length} ${widget.noun}',
            key: const ValueKey('table-count'),
            style: Theme.of(context).textTheme.labelMedium,
          ),
          SegmentedButton<OoDensity>(
            key: const ValueKey('density'),
            showSelectedIcon: false,
            style: const ButtonStyle(
              visualDensity: VisualDensity.compact,
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
            segments: const [
              ButtonSegment(value: OoDensity.compact, label: Text('Compact')),
              ButtonSegment(
                value: OoDensity.comfortable,
                label: Text('Comfortable'),
              ),
            ],
            selected: {density},
            onSelectionChanged: (chosen) =>
                unawaited(OoTablePreferences.choose(chosen.first)),
          ),
          IconButton(
            key: const ValueKey('view-mode'),
            tooltip: cards ? 'Show as a table' : 'Show as cards',
            icon: Icon(cards ? Icons.table_rows_outlined : Icons.view_agenda),
            onPressed: () => setState(() => _cards = !cards),
          ),
          MenuAnchor(
            builder: (context, controller, _) => IconButton(
              key: const ValueKey('columns'),
              tooltip: 'Columns',
              icon: const Icon(Icons.view_column_outlined),
              onPressed: () =>
                  controller.isOpen ? controller.close() : controller.open(),
            ),
            menuChildren: [
              for (final column in widget.columns)
                CheckboxMenuButton(
                  closeOnActivate: false,
                  value: !_hidden.contains(column.id),
                  onChanged: (on) => setState(() {
                    if (on ?? false) {
                      _hidden.remove(column.id);
                    } else if (_visible.length > 1) {
                      _hidden.add(column.id);
                    }
                  }),
                  child: Text(column.label),
                ),
            ],
          ),
        ],
      ),
    );
  }

  List<OoColumn<T>> get _visible => [
    for (final column in widget.columns)
      if (!_hidden.contains(column.id)) column,
  ];

  /// Every value the rows hold for [facet], and the ones the route keeps, sorted.
  List<String> _options(OoFacet<T> facet) {
    final values = <String>{
      ...widget.route.listOf(facet.id),
      for (final row in widget.rows) ...facet.values(row),
    };
    return values.toList()..sort(
      (a, b) => facet
          .labelOf(a)
          .toLowerCase()
          .compareTo(facet.labelOf(b).toLowerCase()),
    );
  }

  List<Widget> _checklist(OoFacet<T> facet) {
    final held = widget.route.listOf(facet.id);
    return [
      for (final value in _options(facet))
        CheckboxMenuButton(
          key: ValueKey('facet-${facet.id}-$value'),
          closeOnActivate: false,
          value: held.contains(value),
          onChanged: (on) => _toggleFacet(facet, value, on ?? false),
          child: Text(facet.labelOf(value)),
        ),
    ];
  }

  Widget _facetChip(OoFacet<T> facet, {required bool selected}) {
    final held = widget.route.listOf(facet.id);
    return MenuAnchor(
      menuChildren: _checklist(facet),
      builder: (context, controller, _) => InputChip(
        key: ValueKey('facet-chip-${facet.id}'),
        selected: selected,
        label: Text(
          '${facet.label}: ${[for (final value in held) facet.labelOf(value)].join(', ')}',
        ),
        onPressed: () =>
            controller.isOpen ? controller.close() : controller.open(),
        onDeleted: () => _route(widget.route.withField(facet.id, null)),
        deleteButtonTooltipMessage: 'Remove this filter',
      ),
    );
  }

  Widget _presets() => Padding(
    padding: const EdgeInsets.fromLTRB(16, 0, 16, 4),
    child: Wrap(
      spacing: 8,
      children: [
        for (final preset in widget.presets)
          ChoiceChip(
            key: ValueKey('preset-${preset.label}'),
            label: Text(preset.label),
            selected: preset.active(widget.route),
            onSelected: (_) => _route(preset.apply(widget.route)),
          ),
      ],
    ),
  );

  Widget _bulkBar(OoTokens tokens) {
    final rows = _selectedRows;
    return Container(
      key: const ValueKey('bulk-bar'),
      color: tokens.surface,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      child: Wrap(
        spacing: 8,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Text('${rows.length} selected'),
          for (final action in widget.bulkActions)
            OutlinedButton.icon(
              onPressed: action.enabled?.call(rows) == false
                  ? null
                  : () async {
                      await action.onSelected(rows);
                      if (mounted) setState(_selected.clear);
                    },
              icon: action.icon == null ? null : Icon(action.icon, size: 16),
              label: Text(action.label),
            ),
          TextButton(
            onPressed: () => setState(_selected.clear),
            child: const Text('Clear selection'),
          ),
        ],
      ),
    );
  }

  static const _checkWidth = 40.0;

  Widget _header(BuildContext context, OoTokens tokens, List<T> shown) {
    final route = widget.route;
    final sort = route['sort'] ?? widget.defaultSort?.$1;
    final ascending = route['order'] != null
        ? route['order'] == 'asc'
        : (route['sort'] != null || (widget.defaultSort?.$2 ?? true));
    final all =
        shown.isNotEmpty &&
        shown.every((row) => _selected.contains(widget.rowId(row)));
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: tokens.muted.withAlpha(80))),
      ),
      child: Row(
        children: [
          if (widget.bulkActions.isNotEmpty)
            SizedBox(
              width: _checkWidth,
              child: Checkbox(
                key: const ValueKey('select-all'),
                value: all,
                onChanged: (on) => setState(() {
                  for (final row in shown) {
                    final id = widget.rowId(row);
                    if (on ?? false) {
                      _selected.add(id);
                    } else {
                      _selected.remove(id);
                    }
                  }
                }),
              ),
            ),
          for (final column in _visible)
            _sized(
              column,
              InkWell(
                key: ValueKey('header-${column.id}'),
                onTap: column.sortable ? () => _sort(column) : null,
                child: Semantics(
                  sortKey: null,
                  label: column.sortable ? 'Sort by ${column.label}' : null,
                  child: Padding(
                    padding: const EdgeInsets.symmetric(vertical: 8),
                    child: Row(
                      children: [
                        Flexible(
                          child: Text(
                            column.label,
                            overflow: TextOverflow.ellipsis,
                            style: Theme.of(context).textTheme.labelLarge,
                          ),
                        ),
                        if (sort == column.id)
                          Icon(
                            ascending
                                ? Icons.arrow_upward
                                : Icons.arrow_downward,
                            size: 14,
                          ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _sized(OoColumn<T> column, Widget child) => column.width != null
      ? SizedBox(width: column.width, child: child)
      : Expanded(flex: column.flex, child: child);

  Widget _row(BuildContext context, OoTokens tokens, List<T> shown, int index) {
    final row = shown[index];
    final id = widget.rowId(row);
    final chosen = _selected.contains(id);
    final compact = OoTablePreferences.density.value == OoDensity.compact;
    final visible = _visible;
    return Material(
      key: ValueKey('row-$id'),
      color: index == _cursor
          ? tokens.accent.withAlpha(36)
          : chosen
          ? tokens.surface
          : Colors.transparent,
      child: InkWell(
        onTap: () {
          setState(() => _cursor = index);
          _bodyFocus.requestFocus();
          widget.onOpen?.call(row);
        },
        child: ConstrainedBox(
          constraints: BoxConstraints(minHeight: compact ? 36 : 56),
          child: Row(
            children: [
              if (widget.bulkActions.isNotEmpty)
                SizedBox(
                  width: _checkWidth,
                  child: Checkbox(
                    key: ValueKey('select-$id'),
                    value: chosen,
                    onChanged: (_) => _toggle(id),
                  ),
                ),
              for (final (at, column) in visible.indexed)
                _sized(
                  column,
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 4),
                    child: at == 0 && !compact && widget.detail != null
                        ? Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              column.cell(context, row),
                              DefaultTextStyle.merge(
                                style: TextStyle(color: tokens.muted),
                                child: widget.detail!(context, row),
                              ),
                            ],
                          )
                        : column.cell(context, row),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _card(
    BuildContext context,
    OoTokens tokens,
    List<T> shown,
    int index,
  ) {
    final row = shown[index];
    final id = widget.rowId(row);
    final visible = _visible;
    if (widget.cardBuilder case final build?) {
      return KeyedSubtree(
        key: ValueKey('card-$id'),
        child: build(context, row),
      );
    }
    return Card(
      key: ValueKey('card-$id'),
      child: InkWell(
        onTap: () {
          setState(() => _cursor = index);
          _bodyFocus.requestFocus();
          widget.onOpen?.call(row);
        },
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (widget.bulkActions.isNotEmpty)
                Checkbox(
                  key: ValueKey('select-$id'),
                  value: _selected.contains(id),
                  onChanged: (_) => _toggle(id),
                ),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    if (visible.isNotEmpty)
                      DefaultTextStyle.merge(
                        style: Theme.of(context).textTheme.titleSmall,
                        child: visible.first.cell(context, row),
                      ),
                    for (final column in visible.skip(1))
                      Padding(
                        padding: const EdgeInsets.only(top: 4),
                        child: Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            SizedBox(
                              width: 110,
                              child: Text(
                                column.label,
                                style: TextStyle(color: tokens.muted),
                              ),
                            ),
                            Expanded(child: column.cell(context, row)),
                          ],
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// How a route field reads as a chip, for pages that show the table's state elsewhere.
String ooTableFieldLabel(OoRoute route, String key) =>
    routeFieldLabel(route, key);
