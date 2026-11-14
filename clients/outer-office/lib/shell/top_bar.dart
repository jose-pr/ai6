import 'operation_names.dart';

import 'dart:async';

import 'package:flutter/foundation.dart'
    show defaultTargetPlatform, TargetPlatform;
import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../assistant/chat_layout.dart' show keyTypesCharacter;
import '../controls/mission_actions.dart';
import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';
import '../routes/oo_route.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_theme.dart';
import 'attention_summary.dart';
import 'oo_freshness.dart';
import 'oo_shell.dart';

/// One thing that needs the person: a blocked Run, a green ink request or Mission
/// proposal waiting for a decision, a held brief, a dark Station, a Station's refused
/// configuration, a Report waiting for an assessment or a charge whose last turn failed or
/// whose lease lapsed, as the gateway's attention summary lists it, with whether it still
/// holds, what was checked and when.
@immutable
class OoAttention {
  const OoAttention({
    required this.icon,
    required this.color,
    required this.text,
    required this.route,
    required this.condition,
    required this.scope,
    required this.checkedAt,
    this.detail,
    this.at,
    this.editor,
  });

  final OoIconName icon;
  final Color color;
  final String text;
  final OoRoute route;

  /// Whether it holds now, has passed, or could not be confirmed either way.
  final AttentionCondition condition;

  /// What was checked: `operation:G` or `station:NAME`.
  final String scope;

  /// When the gateway read its source, as Unix seconds.
  final double checkedAt;

  /// Why it is what it is, when there is more to say.
  final String? detail;

  /// When it began, as Unix seconds, when the data says.
  final double? at;

  /// Where a change to what a Station refused is reviewed: the record editor on its records.
  final OoRoute? editor;
}

/// The sources the attention list is read from; it says "nothing" only when it is live.
const attentionSources = [BoardSource.attention];

/// [page] with [fields]; the page alone when a value is not one a route takes (an ID from
/// outside AI6 may not be an identifier).
OoRoute _route(OoPage page, [Map<String, String> fields = const {}]) {
  try {
    var route = OoRoute(page);
    for (final MapEntry(:key, :value) in fields.entries) {
      route = route.withField(key, value);
    }
    return route;
  } on FormatException {
    return OoRoute(page);
  }
}

/// How an item's condition is said when it is not simply current, or null when it is.
String? attentionStatus(OoAttention item) => switch (item.condition) {
  AttentionCondition.current => null,
  AttentionCondition.historical => 'Historical, no longer current',
  AttentionCondition.unverified => 'Could not confirm it still holds',
};

OoIconName _iconOf(String kind) => switch (kind) {
  'green_ink' => OoIconName.greenInk,
  'held_brief' || 'mission_proposal' => OoIconName.briefings,
  'blocked_run' => OoIconName.interrupt,
  'login_needs_person' => OoIconName.pools,
  'report' => OoIconName.reports,
  'charge_failed' || 'charge_lapsed' => OoIconName.timeline,
  _ => OoIconName.stations,
};

/// What needs the person's attention: the gateway's summary, the same one Moneypenny reads, so
/// the page and her answer agree. Each item opens its own page; opening one changes nothing.
/// Items that have passed come last and are muted.
List<OoAttention> attentionItems(MissionBoard board, OoTokens tokens) {
  final summary = board.attention;
  if (summary == null) return const [];
  return [
    for (final entry in summary.items)
      OoAttention(
        icon: _iconOf(entry.kind),
        color: switch (entry.condition) {
          AttentionCondition.historical => tokens.muted,
          _
              when entry.kind == 'station_dark' ||
                  entry.kind == 'charge_failed' =>
            tokens.danger,
          _ => tokens.warning,
        },
        // Parts of a title are an Agent's or a Source's own words: show what renders as nothing.
        text: revealInvisible(entry.title),
        route: entry.route ?? const OoRoute(OoPage.overview),
        condition: entry.condition,
        scope: entry.scope,
        checkedAt: entry.checkedAt,
        detail: entry.detail == null ? null : revealInvisible(entry.detail!),
        at: entry.since,
        editor: entry.editor,
      ),
  ];
}

/// What could not be read, named, when the summary is missing a source or is not live: the
/// list may then be missing things, so it never says nothing needs the person.
String? attentionGaps(MissionBoard board) {
  final summary = board.attention;
  final stale = board.gaps(attentionSources);
  if (summary == null) return 'Could not be read: Attention.';
  if (stale.isNotEmpty) {
    return 'Showing an earlier read: the latest attention check failed. '
            '${summary.gaps ?? ''}'
        .trim();
  }
  return summary.gaps;
}

/// The bell, "Needs attention": a count of [attentionItems] and a menu listing them, each
/// opening its page when the person may open it, then the controls the person asked for
/// that have not reached their end (each with the outcome so far).
class AttentionBell extends StatelessWidget {
  const AttentionBell({
    required this.board,
    required this.pages,
    required this.onOpen,
    super.key,
  });

  final MissionBoard board;
  final List<OoPage> pages;
  final ValueChanged<OoRoute> onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final items = attentionItems(board, tokens);
    final gaps = attentionGaps(board);
    final requests = board.openRequests;
    final open = items
        .where((item) => item.condition != AttentionCondition.historical)
        .length;
    return MenuAnchor(
      alignmentOffset: const Offset(-260, 4),
      menuChildren: [
        if (open == 0)
          MenuItemButton(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 320),
              child: Text(
                gaps == null
                    ? 'Nothing needs your attention.'
                    : 'Nothing found in what loaded. $gaps',
              ),
            ),
          ),
        for (final item in items)
          MenuItemButton(
            leadingIcon: IconTheme.merge(
              data: IconThemeData(color: item.color, size: 20),
              child: OoIcon(item.icon),
            ),
            onPressed: pages.contains(item.route.page)
                ? () => onOpen(item.route)
                : null,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 320),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(item.text, maxLines: 2, overflow: TextOverflow.ellipsis),
                  if (attentionStatus(item) case final status?)
                    Text(status, style: TextStyle(color: tokens.muted)),
                  OoTime(item.at),
                ],
              ),
            ),
          ),
        if (open != 0 && gaps != null)
          MenuItemButton(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 320),
              child: Text(gaps),
            ),
          ),

        if (requests.isNotEmpty) ...[
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 10, 16, 4),
            child: Text('YOUR REQUESTS', style: ooSectionLabel(context)),
          ),
          for (final request in requests)
            MenuItemButton(
              onPressed: request.stage == ControlStage.unknown
                  ? () => unawaited(
                      board.checkControl(
                        request.operationId,
                        request.missionId,
                      ),
                    )
                  : null,
              trailingIcon: request.stage == ControlStage.unknown
                  ? const Text('Check again')
                  : null,
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 320),
                child: Text(
                  '${request.effect} · ${request.target} · ${request.stage.label}',
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ),
        ],
      ],
      builder: (context, controller, _) => IconButton(
        tooltip: open == 0 ? 'Needs attention' : 'Needs attention: $open',
        onPressed: () =>
            controller.isOpen ? controller.close() : controller.open(),
        icon: Badge(
          isLabelVisible: open != 0,
          label: Text('$open'),
          child: const OoIcon(OoIconName.notifications),
        ),
      ),
    );
  }
}

/// Search across what the board holds: Operations, Missions, Runs and Stations, each
/// opening its page; it says it covers what loaded, not everything. A field when [expanded],
/// else a button opening the same view. Ctrl+K (⌘K on macOS) opens it from anywhere, unless
/// the key would type a character into a text field.
class OoSearch extends StatefulWidget {
  const OoSearch({
    required this.board,
    required this.pages,
    required this.onOpen,
    required this.expanded,
    super.key,
  });

  final MissionBoard board;
  final List<OoPage> pages;

  /// Opens what a result names.
  final ValueChanged<OoRoute> onOpen;
  final bool expanded;

  static const hint = 'Search operations, missions, runs, stations…';

  /// What the search covers, for the button that stands in for its field.
  static const tooltip = 'Search Operations, Missions, Runs and Stations';

  @override
  State<OoSearch> createState() => _OoSearchState();
}

class _OoSearchState extends State<OoSearch> {
  final _controller = SearchController();

  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_key);
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_key);
    _controller.dispose();
    super.dispose();
  }

  bool _key(KeyEvent event) {
    if (event is! KeyDownEvent || event.logicalKey != LogicalKeyboardKey.keyK) {
      return false;
    }
    final keyboard = HardwareKeyboard.instance;
    final mac = defaultTargetPlatform == TargetPlatform.macOS;
    if (!(mac ? keyboard.isMetaPressed : keyboard.isControlPressed) ||
        keyboard.isAltPressed ||
        keyboard.isShiftPressed) {
      return false;
    }
    if (keyTypesCharacter(event, FocusManager.instance.primaryFocus)) {
      return false;
    }
    if (!_controller.isOpen) _controller.openView();
    return true;
  }

  /// Everything searchable, as (icon, title, detail, route).
  List<(OoIconName, String, String, OoRoute)> _entries() => [
    for (final operation
        in widget.board.userSession?.operations ??
            const <UserOperationAccess>[])
      (
        OoIconName.operations,
        'Operation ${OperationNames.of(operation.operationId)}',
        'Operation',
        _route(OoPage.operations, {'operation_id': operation.operationId}),
      ),
    for (final mission in widget.board.missionGroups())
      (
        OoIconName.missions,
        'Mission ${widget.board.handleOf(mission.operationId, mission.missionId)}',
        [
          ?widget.board
              .missionBrief(mission.operationId, mission.missionId)
              ?.name,
          'Operation ${OperationNames.of(mission.operationId)}',
          mission.state,
        ].join(' · '),
        _route(OoPage.missions, {
          'operation_id': mission.operationId,
          'mission_id': mission.missionId,
        }),
      ),
    for (final run in widget.board.runs)
      (
        OoIconName.runs,
        'Run ${run.runId}',
        'Mission ${widget.board.handleOf(run.operationId, run.missionId)} · '
            '${run.state ?? 'working'}',
        _route(OoPage.runs, {
          'operation_id': run.operationId,
          'run_id': run.runId,
        }),
      ),
    for (final station in widget.board.stationBoard.stations)
      (
        OoIconName.stations,
        'Station ${station.stationId}',
        station.healthy ? 'online' : 'dark',
        _route(OoPage.stations, {'station_id': station.stationId}),
      ),
  ];

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return SearchAnchor(
      searchController: _controller,
      viewHintText: OoSearch.hint,
      viewBackgroundColor: tokens.surface,
      dividerColor: tokens.border,
      // The field only opens the search view, which takes the typing: the top bar holds no
      // text field of its own.
      builder: (context, controller) => widget.expanded
          ? Semantics(
              button: true,
              label: 'Search',
              excludeSemantics: true,
              child: Material(
                color: tokens.raised,
                shape: StadiumBorder(side: BorderSide(color: tokens.border)),
                clipBehavior: Clip.antiAlias,
                child: InkWell(
                  onTap: controller.openView,
                  child: SizedBox(
                    height: 44,
                    child: Row(
                      children: [
                        const SizedBox(width: 16),
                        IconTheme.merge(
                          data: IconThemeData(color: tokens.muted, size: 18),
                          child: const OoIcon(OoIconName.search),
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Text(
                            OoSearch.hint,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(color: tokens.muted, fontSize: 14),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            )
          : IconButton(
              tooltip: OoSearch.tooltip,
              onPressed: controller.openView,
              icon: const OoIcon(OoIconName.search),
            ),
      suggestionsBuilder: (context, controller) {
        final query = controller.text.trim().toLowerCase();
        final found = _entries()
            .where(
              (entry) =>
                  widget.pages.contains(entry.$4.page) &&
                  (query.isEmpty ||
                      '${entry.$2} ${entry.$3}'.toLowerCase().contains(query)),
            )
            .toList();
        final limit = widget.board.limits.pages.search;
        final matches = found.take(limit).toList();
        if (matches.isEmpty) {
          return [
            ListTile(
              title: Text(
                query.isEmpty
                    ? 'Nothing on the board yet.'
                    : 'No matches in the loaded board',
              ),
            ),
          ];
        }
        return [
          ListTile(
            dense: true,
            title: Text(
              found.length > limit
                  ? 'First $limit of ${found.length} results in the loaded board'
                  : 'Results in the loaded board',
              style: TextStyle(color: tokens.muted),
            ),
          ),
          for (final (icon, title, detail, route) in matches)
            ListTile(
              leading: IconTheme.merge(
                data: IconThemeData(color: tokens.accent, size: 20),
                child: OoIcon(icon),
              ),
              title: Text(title),
              subtitle: Text(detail, style: TextStyle(color: tokens.muted)),
              onTap: () {
                controller.closeView(null);
                widget.onOpen(route);
              },
            ),
        ];
      },
    );
  }
}

/// The signed-in person's chip: initials, and unless [compact] their name and first
/// directory group (the profile names no roles). Opens the account dialog: profile,
/// permissions, theme and sign-out.
class UserChip extends StatelessWidget {
  const UserChip({
    required this.run,
    required this.onPressed,
    this.compact = false,
    super.key,
  });

  final UserSessionSummary? run;
  final VoidCallback onPressed;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final name = _name(run);
    final initials = ooInitials(name);
    final group = run?.groups.firstOrNull;
    final avatar = Container(
      width: 36,
      height: 36,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: tokens.raised,
        shape: BoxShape.circle,
        border: Border.all(color: tokens.border),
      ),
      child: initials == null
          ? Icon(Icons.person_outline, size: 19, color: tokens.text)
          : Text(
              initials,
              style: TextStyle(
                color: tokens.text,
                fontWeight: FontWeight.w600,
                fontSize: 13,
              ),
            ),
    );
    return Tooltip(
      message: 'Your account and settings',
      child: InkWell(
        onTap: onPressed,
        borderRadius: BorderRadius.circular(24),
        child: ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 48, minWidth: 48),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 6),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                avatar,
                if (!compact && name != null) ...[
                  const SizedBox(width: 10),
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 160),
                    // Large text shrinks the two lines to the bar's height.
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      alignment: Alignment.centerLeft,
                      child: ConstrainedBox(
                        constraints: const BoxConstraints(maxWidth: 160),
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              name,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                color: tokens.text,
                                fontSize: 13.5,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                            if (group != null)
                              Text(
                                group,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  color: tokens.muted,
                                  fontSize: 12,
                                ),
                              ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

String? _name(UserSessionSummary? run) {
  final displayName = run?.displayName?.trim();
  final name = displayName?.isNotEmpty == true ? displayName : run?.name.trim();
  return name == null || name.isEmpty ? null : name;
}

/// Up to two initials of [name]: the first letters of its first word and its surname, among the
/// words that start with a letter. A directory gives one string, so the surname is the word after
/// the last middle initial when there is one (`Ana M. Ortiz Vega`), else the last word.
String? ooInitials(String? name) {
  if (name == null) return null;
  final words = [
    for (final word in name.split(RegExp(r'\s+')))
      if (RegExp(r'^\p{L}', unicode: true).hasMatch(word)) word,
  ];
  if (words.isEmpty) return null;
  String letter(String word) =>
      String.fromCharCode(word.runes.first).toUpperCase();
  if (words.length == 1) return letter(words.first);
  final initial = RegExp(r'^\p{L}\.?$', unicode: true);
  final lastInitial = words.lastIndexWhere(initial.hasMatch);
  final surname = lastInitial >= 0 && lastInitial + 1 < words.length
      ? words[lastInitial + 1]
      : words.last;
  return letter(words.first) + letter(surname);
}
