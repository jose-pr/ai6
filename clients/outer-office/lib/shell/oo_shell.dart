import 'package:material_ui/material_ui.dart';

import '../theme/oo_decor.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_theme.dart';

/// The pages of the rail, in its order.
enum OoPage {
  overview('overview', 'Overview', OoIconName.overview, ''),
  operations(
    'operations',
    'Operations',
    OoIconName.operations,
    'The projects you can see, their policy and the Missions under each.',
  ),
  missions(
    'missions',
    'Missions',
    OoIconName.missions,
    'Each Mission is one brief of work under an Operation.',
  ),
  sessions(
    'sessions',
    'Sessions',
    OoIconName.sessions,
    'Each Session is one run of an Agent on a Mission.',
  ),
  agents(
    'agents',
    'Agents',
    OoIconName.agents,
    'The codenames that take Missions, with their model, clearance and record.',
  ),
  stations(
    'stations',
    'Stations',
    OoIconName.stations,
    'The machines running AI6 and the Theaters they reach.',
  ),
  greenInk(
    'green_ink',
    'Green Ink',
    OoIconName.greenInk,
    'Irreversible steps that wait for a person to sign.',
  ),
  briefings(
    'briefings',
    'Briefings',
    OoIconName.briefings,
    'Work that waits for your decision before it becomes a Mission.',
  ),
  roster(
    'roster',
    'Roster',
    OoIconName.roster,
    'The records every Station reads.',
  ),
  pools(
    'pools',
    'Account Pools',
    OoIconName.pools,
    'The accounts Sessions draw from, and how much of each is used.',
  ),
  timeline(
    'timeline',
    'Timeline',
    OoIconName.timeline,
    'What happened in an Operation, newest first.',
  ),
  audit('audit', 'Audit', OoIconName.audit, 'Who did what, for review.'),
  access(
    'access',
    'Access',
    OoIconName.settings,
    'Who may do what, and the accounts.',
  ),

  compartments(
    'compartments',
    'Compartments',
    OoIconName.settings,
    'Need-to-know access to other Operations: what each grants, and who is read in.',
  ),

  /// Her memory of the person; opened from her chat, not the rail.
  memory('memory', "Moneypenny's memory", OoIconName.moneypenny, ''),

  /// The pages she published for the person and those shared with them; opened from her chat
  /// or a page's link, not the rail.
  pages('pages', 'Her pages', OoIconName.moneypenny, '');

  const OoPage(this.id, this.label, this.iconName, this.purpose);

  /// The page's name in its route, such as `green_ink`.
  final String id;
  final String label;
  final OoIconName iconName;

  /// One line on what the page is for, under its heading; empty on a page that says so itself.
  final String purpose;

  /// The page's line icon, for small sizes.
  Widget get icon => OoIcon(iconName);
}

/// The rail's groups, by what a person is asking, each with its pages in rail order. A group
/// the person may see nothing of is not shown.
enum OoRailGroup {
  work('Work', [
    OoPage.overview,
    OoPage.missions,
    OoPage.sessions,
    OoPage.greenInk,
    OoPage.briefings,
  ]),
  peopleAndPolicy('People and policy', [
    OoPage.operations,
    OoPage.agents,
    OoPage.roster,
    OoPage.pools,
    OoPage.access,
    OoPage.compartments,
  ]),
  machines('Machines', [OoPage.stations]),
  record('Record', [OoPage.timeline, OoPage.audit]);

  const OoRailGroup(this.label, this.pages);

  final String label;
  final List<OoPage> pages;

  /// The groups [allowed] leaves something in, each with only those pages.
  static List<(OoRailGroup, List<OoPage>)> of(Iterable<OoPage> allowed) => [
    for (final group in values)
      if (group.pages.where(allowed.contains).toList() case final pages
          when pages.isNotEmpty)
        (group, pages),
  ];
}

/// A page's heading: its name, where the top bar does not already show it, and what it is for.
class OoPageHeading extends StatelessWidget {
  const OoPageHeading(this.page, {super.key});

  final OoPage page;

  @override
  Widget build(BuildContext context) {
    if (page.purpose.isEmpty) return const SizedBox.shrink();
    final text = Theme.of(context).textTheme;
    final tokens = OoTokens.of(context);
    final titled = MediaQuery.sizeOf(context).width >= OoBreakpoints.wide;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 16, 24, 4),
      child: Align(
        alignment: Alignment.centerLeft,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (titled)
              Semantics(
                header: true,
                child: Text(
                  page.label,
                  style: text.headlineSmall?.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            Text(
              page.purpose,
              style: text.bodyMedium?.copyWith(color: tokens.muted),
            ),
          ],
        ),
      ),
    );
  }
}

/// How many pages a phone's bottom bar holds besides **More**.
const _phoneBarPages = 4;

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
/// 226 px with labels beside the icons, in groups; down to [OoBreakpoints.compact] it is icons
/// with labels on hover; narrower, a modal drawer behind a menu button; on a phone the first
/// pages sit in a bottom bar and **More** opens the drawer.
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
    final groups = OoRailGroup.of(pages);
    final ordered = [for (final (_, pages) in groups) ...pages];
    final selected = ordered.indexOf(page);
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
                  onSelect(ordered[index]);
                },
                children: [
                  const _DrawerHeading(),
                  for (final (group, pages) in groups) ...[
                    _GroupLabel(
                      group.label,
                      padding: const EdgeInsets.fromLTRB(28, 14, 16, 8),
                    ),
                    for (final page in pages)
                      NavigationDrawerDestination(
                        icon: OoTile(page.iconName, size: 38),
                        label: Text(page.label),
                      ),
                  ],
                ],
              ),
            )
          : null,
      bottomNavigationBar: phone && ordered.isNotEmpty
          ? _PhoneBar(pages: ordered, page: page, onSelect: onSelect)
          : null,
      appBar: AppBar(
        automaticallyImplyLeading: !phone,
        toolbarHeight: phone ? 60 : 68,
        titleSpacing: phone ? 16 : 20,
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
                // A long name shrinks to fit between the buttons rather than being cut off.
                child: FittedBox(
                  fit: BoxFit.scaleDown,
                  alignment: Alignment.centerLeft,
                  child: Text(
                    page.label,
                    style: TextStyle(
                      fontFamily: ooDisplayFamily,
                      fontWeight: FontWeight.w700,
                      fontSize: phone ? 18 : 20,
                      letterSpacing: 1.2,
                      color: tokens.text,
                    ),
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
              pages: ordered,
              selected: selected,
              extended: wide,
              onSelect: (index) => onSelect(ordered[index]),
            ),
            const VerticalDivider(width: 1),
          ],
          Expanded(child: body),
        ],
      ),
    );
  }
}

/// A phone's bottom bar: the first pages the person may open and, when there are more,
/// **More**, which opens the drawer with all of them and stands selected for a page not here.
class _PhoneBar extends StatelessWidget {
  const _PhoneBar({
    required this.pages,
    required this.page,
    required this.onSelect,
  });

  final List<OoPage> pages;
  final OoPage page;
  final ValueChanged<OoPage> onSelect;

  @override
  Widget build(BuildContext context) {
    final more = pages.length > _phoneBarPages + 1;
    final shown = more ? pages.take(_phoneBarPages).toList() : pages;
    final at = shown.indexOf(page);
    return NavigationBar(
      height: 64,
      selectedIndex: at >= 0 ? at : (more ? shown.length : 0),
      labelBehavior: NavigationDestinationLabelBehavior.alwaysShow,
      onDestinationSelected: (index) => index < shown.length
          ? onSelect(shown[index])
          : Scaffold.of(context).openDrawer(),
      destinations: [
        for (final page in shown)
          NavigationDestination(icon: page.icon, label: page.label),
        if (more)
          const NavigationDestination(icon: Icon(Icons.menu), label: 'More'),
      ],
    );
  }
}

/// The rail of pages beside the board, in [OoRailGroup]s: [width] wide with labels beside the
/// icons when [extended], else [compactWidth] wide with icons alone and each label on hover.
/// Each entry takes the keyboard's focus in turn, and says its label to a screen reader.
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

  /// The icon rail's width, in logical pixels.
  static const double compactWidth = 72;

  final List<OoPage> pages;

  /// The index of the current page in [pages], or -1.
  final int selected;
  final bool extended;
  final ValueChanged<int> onSelect;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final groups = OoRailGroup.of(pages);
    Widget entry(OoPage page) {
      final index = pages.indexOf(page);
      return extended
          ? _RailEntry(
              page: page,
              selected: index == selected,
              onTap: () => onSelect(index),
            )
          : _IconRailEntry(
              page: page,
              selected: index == selected,
              onTap: () => onSelect(index),
            );
    }

    return Material(
      color: tokens.surface,
      child: SizedBox(
        width: extended ? width : compactWidth,
        child: ListView(
          padding: extended
              ? const EdgeInsets.fromLTRB(12, 4, 12, 16)
              : const EdgeInsets.fromLTRB(10, 8, 10, 16),
          children: [
            for (final (index, (group, pages)) in groups.indexed) ...[
              if (extended)
                _GroupLabel(group.label)
              else if (index > 0)
                Semantics(
                  label: group.label,
                  child: Padding(
                    padding: const EdgeInsets.symmetric(vertical: 6),
                    child: Divider(height: 1, color: tokens.border),
                  ),
                ),
              for (final page in pages) entry(page),
            ],
          ],
        ),
      ),
    );
  }
}

/// A group's quiet heading in the rail or the drawer.
class _GroupLabel extends StatelessWidget {
  const _GroupLabel(
    this.text, {
    this.padding = const EdgeInsets.fromLTRB(14, 14, 14, 6),
  });

  final String text;
  final EdgeInsetsGeometry padding;

  @override
  Widget build(BuildContext context) => Padding(
    padding: padding,
    child: Semantics(
      header: true,
      child: Text(
        text.toUpperCase(),
        style: Theme.of(context).textTheme.labelSmall?.copyWith(
          color: OoTokens.of(context).muted,
          letterSpacing: 1.1,
          fontSize: 11,
        ),
      ),
    ),
  );
}

/// One entry of the icon rail: its label on hover and to a screen reader, and the selected
/// highlight of the labelled rail.
class _IconRailEntry extends StatelessWidget {
  const _IconRailEntry({
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
    final radius = BorderRadius.circular(10);
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Tooltip(
        message: page.label,
        preferBelow: false,
        waitDuration: Duration.zero,
        excludeFromSemantics: true,
        child: Semantics(
          button: true,
          selected: selected,
          label: page.label,
          child: DecoratedBox(
            decoration: selected
                ? BoxDecoration(
                    borderRadius: radius,
                    color: tokens.selectedFill,
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
                key: ValueKey('rail-${page.id}'),
                onTap: onTap,
                focusColor: tokens.accent.withValues(alpha: 0.3),
                child: SizedBox(
                  height: 48,
                  child: Center(
                    child: IconTheme.merge(
                      data: IconThemeData(
                        color: selected ? tokens.accent : tokens.muted,
                        size: 22,
                      ),
                      child: page.icon,
                    ),
                  ),
                ),
              ),
            ),
          ),
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
                    colors: tokens.gradient(tokens.selectedFill, tokens.raised),
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
              key: ValueKey('rail-${page.id}'),
              onTap: onTap,
              focusColor: tokens.accent.withValues(alpha: 0.3),
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: 14,
                  vertical: 10,
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
