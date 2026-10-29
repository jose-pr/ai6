import 'package:material_ui/material_ui.dart';

import '../theme/oo_decor.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_theme.dart';

/// The pages of the rail, in its order.
enum OoPage {
  overview('overview', 'Overview', OoIconName.overview),
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

  const OoPage(this.id, this.label, this.iconName);

  /// The page's name in Moneypenny's page context, such as `green_ink`.
  final String id;
  final String label;
  final OoIconName iconName;

  /// The page's line icon, for small sizes.
  Widget get icon => OoIcon(iconName);
}

/// Window widths at which the layout changes, in logical pixels.
abstract final class OoBreakpoints {
  /// From here: the labelled rail and the board, with room for a panel beside it.
  static const double wide = 1200;

  /// From here: the compact rail. Below it one view at a time, the rail in a drawer.
  static const double compact = 900;

  /// Below this: a phone. The top bar keeps only what fits, Overview is one column, tables
  /// become lists and Moneypenny opens full screen.
  static const double phone = 600;
}

/// The application frame: a top bar, the rail of [pages] and the current page's [body].
///
/// The top bar holds the wordmark, the [search] field (an icon below
/// [OoBreakpoints.wide]) and [actions]. At [OoBreakpoints.wide] and wider the rail is
/// 226 px with labels beside the icons; down to [OoBreakpoints.compact] it is compact,
/// labels below the icons; narrower, the rail is a modal drawer behind a menu button, since
/// twelve entries do not fit a bottom bar.
class OoShell extends StatelessWidget {
  const OoShell({
    required this.pages,
    required this.page,
    required this.onSelect,
    required this.body,
    this.actions = const [],
    this.search,
    this.scaffoldKey,
    super.key,
  });

  /// The pages this person may open, in rail order.
  final List<OoPage> pages;
  final OoPage page;
  final ValueChanged<OoPage> onSelect;
  final Widget body;

  /// Top-bar buttons, after the search.
  final List<Widget> actions;

  /// The search: a field when given `true` (wide windows), else a button that opens it.
  final Widget Function(bool expanded)? search;
  final GlobalKey<ScaffoldState>? scaffoldKey;

  @override
  Widget build(BuildContext context) {
    final width = MediaQuery.sizeOf(context).width;
    final selected = pages.indexOf(page);
    final wide = width >= OoBreakpoints.wide;
    final phone = width < OoBreakpoints.phone;
    final tokens = OoTokens.of(context);
    return Scaffold(
      key: scaffoldKey,
      drawer: width < OoBreakpoints.compact
          ? Builder(
              builder: (context) => NavigationDrawer(
                selectedIndex: selected < 0 ? null : selected,
                onDestinationSelected: (index) {
                  Scaffold.of(context).closeDrawer();
                  onSelect(pages[index]);
                },
                children: [
                  const _DrawerHeading(),
                  for (final page in pages)
                    NavigationDrawerDestination(
                      icon: OoTile(page.iconName, size: 38),
                      label: Text(page.label),
                    ),
                ],
              ),
            )
          : null,
      appBar: AppBar(
        toolbarHeight: phone ? 60 : 68,
        titleSpacing: phone ? 4 : 20,
        title: Row(
          children: [
            if (wide)
              const FittedBox(fit: BoxFit.scaleDown, child: OoWordmark())
            else if (!phone) ...[
              const OoWordmark(compact: true, size: 26),
              const SizedBox(width: 14),
            ],
            if (!wide)
              Flexible(
                child: Text(
                  page.label,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontFamily: ooDisplayFamily,
                    fontWeight: FontWeight.w700,
                    fontSize: phone ? 18 : 20,
                    letterSpacing: 1.2,
                    color: tokens.text,
                  ),
                ),
              ),
            if (wide && search != null)
              Expanded(
                child: Center(
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 440),
                    child: search!(true),
                  ),
                ),
              ),
          ],
        ),
        actions: [
          if (!wide && search != null) search!(false),
          ...actions,
          SizedBox(width: phone ? 4 : 12),
        ],
      ),
      body: Row(
        children: [
          if (width >= OoBreakpoints.compact) ...[
            OoRail(
              pages: pages,
              selected: selected,
              extended: wide,
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
          padding: const EdgeInsets.fromLTRB(12, 8, 12, 16),
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

/// One labelled rail entry. Selected: the gilt highlight of the design reference, a fill
/// washed with the accent, an accent edge and glow, the icon in accent and the label in
/// `text` (accent on the fill is below 4.5:1 in some themes).
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
    final radius = BorderRadius.circular(10);
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Semantics(
        selected: selected,
        button: true,
        child: DecoratedBox(
          decoration: selected
              ? BoxDecoration(
                  borderRadius: radius,
                  gradient: LinearGradient(
                    colors: [tokens.selectedFill, tokens.raised],
                  ),
                  border: Border.all(
                    color: tokens.accent.withValues(alpha: 0.7),
                  ),
                  boxShadow: tokens.glow(tokens.accent, radius: 10),
                )
              : const BoxDecoration(),
          child: Material(
            type: MaterialType.transparency,
            borderRadius: radius,
            clipBehavior: Clip.antiAlias,
            child: InkWell(
              onTap: onTap,
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: 14,
                  vertical: 12,
                ),
                child: Row(
                  children: [
                    IconTheme.merge(
                      data: IconThemeData(color: iconColor, size: 20),
                      child: page.icon,
                    ),
                    const SizedBox(width: 14),
                    Expanded(
                      child: Text(
                        page.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context).textTheme.labelLarge?.copyWith(
                          color: labelColor,
                          fontWeight: selected ? FontWeight.w600 : null,
                        ),
                      ),
                    ),
                  ],
                ),
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
    padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
    child: Text(
      'COMMAND CENTER',
      style: ooSectionLabel(context).copyWith(fontSize: 11.5),
    ),
  );
}

/// The menu drawer's heading: the app's mark and the wordmark.
class _DrawerHeading extends StatelessWidget {
  const _DrawerHeading();

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 20, 16, 16),
    child: Row(
      children: [
        ClipRRect(
          borderRadius: BorderRadius.circular(12),
          child: Image.asset(
            ooAppMark,
            width: 52,
            height: 52,
            filterQuality: FilterQuality.medium,
            excludeFromSemantics: true,
          ),
        ),
        const SizedBox(width: 14),
        const Flexible(
          child: FittedBox(fit: BoxFit.scaleDown, child: OoWordmark(size: 22)),
        ),
      ],
    ),
  );
}

/// The Outer Office app icon from the brand pack, 256 px.
const ooAppMark = 'assets/brand/outer-office-256.png';

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
