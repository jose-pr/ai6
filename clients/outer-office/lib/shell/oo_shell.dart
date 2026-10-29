import 'package:material_ui/material_ui.dart';

import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';

/// The pages of the rail, in its order.
enum OoPage {
  overview('overview', 'Overview', null),
  operations('operations', 'Operations', OoIconName.operations),
  missions('missions', 'Missions', OoIconName.missions),
  sessions('sessions', 'Sessions', OoIconName.sessions),
  agents('agents', 'Agents', OoIconName.agents),
  stations('stations', 'Stations', OoIconName.stations),
  greenInk('green_ink', 'Green Ink', OoIconName.greenInk),
  briefings('briefings', 'Briefings', OoIconName.briefings),
  roster('roster', 'Roster', OoIconName.roster),
  pools('pools', 'Account Pools', OoIconName.pools),
  timeline('timeline', 'Timeline', OoIconName.timeline),
  audit('audit', 'Audit', OoIconName.audit);

  const OoPage(this.id, this.label, this._icon);

  /// The page's name in Moneypenny's page context, such as `green_ink`.
  final String id;
  final String label;
  final OoIconName? _icon;

  /// The kit's icon, or a Material one where the kit has none.
  Widget get icon => switch (_icon) {
    final name? => OoIcon(name),
    null => const Icon(Icons.dashboard_outlined),
  };
}

/// Window widths at which the layout changes, in logical pixels.
abstract final class OoBreakpoints {
  /// From here: the labelled rail and the board, with room for a panel beside it.
  static const double wide = 1200;

  /// From here: the compact rail. Below it one view at a time, the rail in a drawer.
  static const double compact = 900;
}

/// The application frame: a top bar, the rail of [pages], the current page's [body], and
/// the right-hand [endDrawer].
///
/// At [OoBreakpoints.wide] and wider the rail is 226 px with labels beside the icons; down
/// to [OoBreakpoints.compact] it is compact, labels below the icons; narrower, the rail is a
/// modal drawer behind a menu button, since twelve entries do not fit a bottom bar.
class OoShell extends StatelessWidget {
  const OoShell({
    required this.pages,
    required this.page,
    required this.onSelect,
    required this.body,
    this.actions = const [],
    this.endDrawer,
    super.key,
  });

  /// The pages this person may open, in rail order.
  final List<OoPage> pages;
  final OoPage page;
  final ValueChanged<OoPage> onSelect;
  final Widget body;

  /// Top-bar buttons, after the page title.
  final List<Widget> actions;
  final Widget? endDrawer;

  @override
  Widget build(BuildContext context) {
    final width = MediaQuery.sizeOf(context).width;
    final selected = pages.indexOf(page);
    return Scaffold(
      drawer: width < OoBreakpoints.compact
          ? Builder(
              builder: (context) => NavigationDrawer(
                selectedIndex: selected < 0 ? null : selected,
                onDestinationSelected: (index) {
                  Scaffold.of(context).closeDrawer();
                  onSelect(pages[index]);
                },
                children: [
                  const _RailHeading(),
                  for (final page in pages)
                    NavigationDrawerDestination(
                      icon: page.icon,
                      label: Text(page.label),
                    ),
                ],
              ),
            )
          : null,
      endDrawer: endDrawer,
      appBar: AppBar(
        title: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const _Mark(),
            const SizedBox(width: 14),
            Flexible(child: Text(page.label, overflow: TextOverflow.ellipsis)),
          ],
        ),
        actions: [...actions, const SizedBox(width: 12)],
      ),
      body: Row(
        children: [
          if (width >= OoBreakpoints.compact) ...[
            OoRail(
              pages: pages,
              selected: selected,
              extended: width >= OoBreakpoints.wide,
              onSelect: (index) => onSelect(pages[index]),
            ),
            const VerticalDivider(width: 1),
          ],
          Expanded(child: body),
        ],
      ),
    );
  }
}

/// The rail of pages beside the board: [width] wide with labels beside the icons when
/// [extended], else a compact [NavigationRail] with labels below them.
class OoRail extends StatelessWidget {
  const OoRail({
    required this.pages,
    required this.selected,
    required this.extended,
    required this.onSelect,
    super.key,
  });

  /// The labelled rail's width, in logical pixels.
  static const double width = 226;

  final List<OoPage> pages;

  /// The index of the current page in [pages], or -1.
  final int selected;
  final bool extended;
  final ValueChanged<int> onSelect;

  @override
  Widget build(BuildContext context) {
    if (!extended) {
      return NavigationRail(
        selectedIndex: selected < 0 ? null : selected,
        onDestinationSelected: onSelect,
        scrollable: true,
        labelType: NavigationRailLabelType.all,
        destinations: [
          for (final page in pages)
            NavigationRailDestination(icon: page.icon, label: Text(page.label)),
        ],
      );
    }
    return Material(
      color: OoTokens.of(context).surface,
      child: SizedBox(
        width: width,
        child: ListView(
          padding: const EdgeInsets.fromLTRB(10, 8, 10, 16),
          children: [
            const _RailHeading(),
            for (final (index, page) in pages.indexed)
              _RailEntry(
                page: page,
                selected: index == selected,
                onTap: () => onSelect(index),
              ),
          ],
        ),
      ),
    );
  }
}

/// One labelled rail entry. Selected: a raised fill, an accent icon and a 3 px accent bar
/// on the left, with the label in `text` (accent on `raised` is below 4.5:1 in Red File).
class _RailEntry extends StatelessWidget {
  const _RailEntry({
    required this.page,
    required this.selected,
    required this.onTap,
  });

  final OoPage page;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final iconColor = selected ? tokens.accent : tokens.muted;
    final labelColor = selected ? tokens.text : tokens.muted;
    final radius = BorderRadius.circular(9);
    return Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: Semantics(
        selected: selected,
        button: true,
        child: Material(
          color: selected ? tokens.raised : const Color(0x00000000),
          borderRadius: radius,
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: onTap,
            child: Container(
              decoration: selected
                  ? BoxDecoration(
                      border: Border(
                        left: BorderSide(color: tokens.accent, width: 3),
                      ),
                    )
                  : null,
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 11),
              child: Row(
                children: [
                  IconTheme.merge(
                    data: IconThemeData(color: iconColor, size: 20),
                    child: page.icon,
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text(
                      page.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.labelLarge
                          ?.copyWith(color: labelColor),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _RailHeading extends StatelessWidget {
  const _RailHeading();

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(16, 12, 16, 10),
    child: Text(
      'COMMAND CENTER',
      style: Theme.of(context).textTheme.labelSmall
          ?.copyWith(letterSpacing: 2, color: OoTokens.of(context).accent),
    ),
  );
}

/// The OO mark of the top bar.
class _Mark extends StatelessWidget {
  const _Mark();

  @override
  Widget build(BuildContext context) => Semantics(
    label: 'Outer Office',
    child: ExcludeSemantics(
      child: Text(
        'OO',
        style: TextStyle(
          fontFamily: 'Georgia',
          fontFamilyFallback: const ['serif'],
          fontWeight: FontWeight.w700,
          fontSize: 24,
          letterSpacing: 1.8,
          color: OoTokens.of(context).accent,
        ),
      ),
    ),
  );
}

/// A page made of [tabs], each a label and its body: one tab is shown without a tab bar,
/// none says [empty].
class OoTabs extends StatelessWidget {
  const OoTabs({required this.tabs, required this.empty, super.key});

  final List<(String, Widget)> tabs;
  final String empty;

  @override
  Widget build(BuildContext context) {
    if (tabs.isEmpty) return Center(child: Text(empty));
    if (tabs.length == 1) return tabs.single.$2;
    return DefaultTabController(
      length: tabs.length,
      child: Column(
        children: [
          TabBar(
            isScrollable: true,
            tabAlignment: TabAlignment.start,
            tabs: [for (final (label, _) in tabs) Tab(text: label)],
          ),
          Expanded(
            child: TabBarView(children: [for (final (_, body) in tabs) body]),
          ),
        ],
      ),
    );
  }
}
