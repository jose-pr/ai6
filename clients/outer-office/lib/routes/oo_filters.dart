import 'package:material_ui/material_ui.dart';

import 'oo_route.dart';

/// How a route field is named on a filter chip.
const _labels = {
  'operation_id': 'Operation',
  'state': 'State',
  'station_id': 'Station',
  'agent': 'Agent',
  'text': 'Text',
  'sort': 'Sorted by',
  'order': 'Order',
  'mission_id': 'Mission',
  'session_id': 'Session',
  'theater_id': 'Theater',
  'role': 'Role',
  'request_id': 'Request',
  'item_id': 'Item',
  'kind': 'Kind',
  'key': 'Record',
  'pool': 'Pool',
  'subtree': 'Subtree',
};

/// Field [key] of [route] as a chip says it, such as `State: failed, blocked`.
String routeFieldLabel(OoRoute route, String key) =>
    '${key == 'tab' ? 'Tab' : _labels[key] ?? key}: '
    '${key == 'state' && route.states.isNotEmpty ? route.states.join(', ') : route[key]}';

/// Every filter of [route] as a removable chip, and "Clear filters"; nothing when the route
/// has none. The tab is not a filter.
class OoFilterBar extends StatelessWidget {
  const OoFilterBar({
    required this.route,
    required this.onRoute,
    this.padding = const EdgeInsets.fromLTRB(24, 0, 24, 8),
    super.key,
  });

  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;
  final EdgeInsetsGeometry padding;

  @override
  Widget build(BuildContext context) {
    if (!route.filtered) return const SizedBox.shrink();
    return Padding(
      padding: padding,
      child: Wrap(
        spacing: 8,
        runSpacing: 8,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          for (final key in route.keys)
            if (key != 'tab')
              InputChip(
                key: ValueKey('filter-$key'),
                label: Text(routeFieldLabel(route, key)),
                onDeleted: () => onRoute(route.without(key)),
                deleteButtonTooltipMessage: 'Remove this filter',
              ),
          TextButton(
            onPressed: () => onRoute(route.cleared),
            child: const Text('Clear filters'),
          ),
        ],
      ),
    );
  }
}

/// The sort and order of a Missions or Sessions route, as a menu.
class OoSortMenu extends StatelessWidget {
  const OoSortMenu({required this.route, required this.onRoute, super.key});

  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  static const _labels = {
    'updated': 'Last updated',
    'created': 'First seen',
    'state': 'State',
    'operation': 'Operation',
    'mission': 'Mission',
    'session': 'Session',
  };

  @override
  Widget build(BuildContext context) => PopupMenuButton<(String?, String?)>(
    tooltip: 'Sort',
    icon: const Icon(Icons.sort),
    onSelected: (choice) => onRoute(
      route.withField('sort', choice.$1).withField('order', choice.$2),
    ),
    itemBuilder: (context) => [
      const PopupMenuItem(value: (null, null), child: Text('Board order')),
      for (final sort in routeWords(route.page, 'sort')) ...[
        PopupMenuItem(
          value: (sort, 'asc'),
          child: Text('${_labels[sort] ?? sort}, ascending'),
        ),
        PopupMenuItem(
          value: (sort, 'desc'),
          child: Text('${_labels[sort] ?? sort}, descending'),
        ),
      ],
    ],
  );
}

/// A page made of [tabs], each an id, a label and its body, showing the tab whose id is
/// [selected] (the first when it is `null` or not among them) and telling [onSelect] when
/// the person picks another. One tab is shown without a tab bar; none says [empty].
class OoTabs extends StatefulWidget {
  const OoTabs({
    required this.tabs,
    required this.empty,
    this.selected,
    this.onSelect,
    super.key,
  });

  final List<(String, String, Widget)> tabs;
  final String empty;
  final String? selected;
  final ValueChanged<String>? onSelect;

  @override
  State<OoTabs> createState() => _OoTabsState();
}

class _OoTabsState extends State<OoTabs> with TickerProviderStateMixin {
  TabController? _controller;

  int get _index {
    final index = widget.tabs.indexWhere((tab) => tab.$1 == widget.selected);
    return index < 0 ? 0 : index;
  }

  void _sync() {
    final controller = _controller;
    if (controller == null || controller.length != widget.tabs.length) {
      controller?.dispose();
      _controller = widget.tabs.length < 2
          ? null
          : (TabController(
              length: widget.tabs.length,
              initialIndex: _index,
              vsync: this,
            )..addListener(_picked));
    } else if (controller.index != _index && !controller.indexIsChanging) {
      controller.index = _index;
    }
  }

  void _picked() {
    final controller = _controller!;
    if (controller.indexIsChanging || controller.index == _index) return;
    widget.onSelect?.call(widget.tabs[controller.index].$1);
  }

  @override
  void initState() {
    super.initState();
    _sync();
  }

  @override
  void didUpdateWidget(OoTabs oldWidget) {
    super.didUpdateWidget(oldWidget);
    _sync();
  }

  @override
  void dispose() {
    _controller?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tabs = widget.tabs;
    if (tabs.isEmpty) return Center(child: Text(widget.empty));
    final controller = _controller;
    if (controller == null) return tabs.single.$3;
    return Column(
      children: [
        TabBar(
          controller: controller,
          isScrollable: true,
          tabAlignment: TabAlignment.start,
          tabs: [for (final (_, label, _) in tabs) Tab(text: label)],
        ),
        Expanded(
          child: TabBarView(
            controller: controller,
            children: [for (final (_, _, body) in tabs) body],
          ),
        ),
      ],
    );
  }
}
