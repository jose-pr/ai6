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
import 'missions_board.dart';
import 'oo_freshness.dart';
import 'oo_shell.dart';

/// One thing that needs the person: a blocked Session, a green ink request or Mission
/// proposal waiting for a decision, a held brief or a dark Station, when it began, and the
/// route of the page that deals with it.
@immutable
class OoAttention {
  const OoAttention({
    required this.icon,
    required this.color,
    required this.text,
    required this.route,
    this.at,
  });

  final OoIconName icon;
  final Color color;
  final String text;
  final OoRoute route;

  /// When it began, as Unix seconds, when the data says.
  final double? at;
}

/// The sources the attention list is read from; it says "nothing" only when all are live.
const attentionSources = [
  BoardSource.board,
  BoardSource.stations,
  BoardSource.heldBriefs,
  BoardSource.greenInk,
  BoardSource.proposals,
];

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

/// What needs the person's attention, derived from what the board has loaded and never
/// stored: Sessions blocked, green ink requests and Mission proposals waiting for a decision,
/// held briefs, and dark Stations. Each opens its own page; opening one changes nothing.
List<OoAttention> attentionItems(MissionBoard board, OoTokens tokens) {
  // A Session held for green ink is the request waiting for it, not a second item.
  final asked = {
    for (final request in board.greenInkRequests)
      (request.operationId, request.sessionId),
  };
  return [
    for (final session in board.sessions.where(
      (session) =>
          session.state == 'blocked' &&
          !(session.reason == 'green-ink' &&
              asked.contains((session.operationId, session.sessionId))),
    ))
      session.reason == 'green-ink'
          ? OoAttention(
              icon: OoIconName.greenInk,
              color: tokens.warning,
              text:
                  'Session ${session.sessionId} of Mission '
                  '${session.missionId} waits for green ink',
              route: _route(OoPage.greenInk, {
                'operation_id': session.operationId,
              }),
              at: session.updatedAt,
            )
          : OoAttention(
              icon: OoIconName.interrupt,
              color: tokens.warning,
              text:
                  'Session ${session.sessionId} of Mission '
                  '${session.missionId} is blocked'
                  '${session.reason == null ? '' : ': ${session.reason!.replaceAll('-', ' ')}'}',
              route: _route(OoPage.missions, {
                'operation_id': session.operationId,
                'mission_id': session.missionId,
              }),
              at: session.updatedAt,
            ),
    for (final request in board.greenInkRequests)
      OoAttention(
        icon: OoIconName.greenInk,
        color: tokens.warning,
        text:
            // The Agent's own text: show what renders as nothing.
            'Agent ${revealInvisible(request.agent)} waits for green ink to '
            '${revealInvisible(request.kind)} ${revealInvisible(request.target)} '
            'in Operation ${request.operationId}',
        route: _route(OoPage.greenInk, {
          'operation_id': request.operationId,
          'request_id': request.id,
        }),
        at: request.requestedAt,
      ),
    for (final brief in board.heldBriefs)
      OoAttention(
        icon: OoIconName.briefings,
        color: tokens.warning,
        text:
            // A brief from a Source is untrusted text: show what renders as nothing.
            'Brief "${revealInvisible(brief.title)}" is held for review in Operation '
            '${brief.operationId}',
        route: _route(OoPage.briefings, {
          'tab': 'held',
          'operation_id': brief.operationId,
          'item_id': brief.key,
        }),
        at: brief.heldAt,
      ),
    for (final proposal in board.missionProposals)
      OoAttention(
        icon: OoIconName.briefings,
        color: tokens.warning,
        text:
            'Mission proposal "${revealInvisible(proposal.title)}" waits for a '
            'decision in Operation ${proposal.operationId}',
        route: _route(OoPage.briefings, {
          'tab': 'proposals',
          'operation_id': proposal.operationId,
          'item_id': proposal.id,
        }),
        at: proposal.createdAt.toDouble(),
      ),
    for (final station in board.stationBoard.stations.where(
      (station) => !station.healthy,
    ))
      OoAttention(
        icon: OoIconName.stations,
        color: tokens.danger,
        text: 'Station ${station.stationId} has gone dark',
        route: _route(OoPage.stations, {'station_id': station.stationId}),
        at: station.seen > 0 ? station.seen : null,
      ),
  ];
}

/// The attention sources that are not live, named, when there are any: the list may then
/// be missing things, so it never says nothing needs the person.
String? attentionGaps(MissionBoard board) {
  final gaps = board.gaps(attentionSources);
  return gaps.isEmpty
      ? null
      : 'Could not be read: ${gaps.map((source) => source.label).join(', ')}.';
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
    return MenuAnchor(
      alignmentOffset: const Offset(-260, 4),
      menuChildren: [
        if (items.isEmpty)
          MenuItemButton(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 320),
              child: Text(
                gaps == null
                    ? 'Nothing needs your attention.'
                    : 'Nothing found in what loaded. $gaps',
              ),
            ),
          )
        else ...[
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
                    Text(
                      item.text,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                    ),
                    OoTime(item.at),
                  ],
                ),
              ),
            ),
          if (gaps != null)
            MenuItemButton(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 320),
                child: Text(gaps),
              ),
            ),
        ],
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
        tooltip: items.isEmpty
            ? 'Needs attention'
            : 'Needs attention: ${items.length}',
        onPressed: () =>
            controller.isOpen ? controller.close() : controller.open(),
        icon: Badge(
          isLabelVisible: items.isNotEmpty,
          label: Text('${items.length}'),
          child: const OoIcon(OoIconName.notifications),
        ),
      ),
    );
  }
}

/// Search across what the board holds: Operations, Missions, Sessions and Stations, each
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
  final ValueChanged<OoPage> onOpen;
  final bool expanded;

  static const hint = 'Search operations, missions, sessions, stations…';

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

  /// Everything searchable, as (icon, title, detail, page).
  List<(OoIconName, String, String, OoPage)> _entries() => [
    for (final operation
        in widget.board.userSession?.operations ??
            const <UserOperationAccess>[])
      (
        OoIconName.operations,
        'Operation ${operation.operationId}',
        'Operation',
        OoPage.operations,
      ),
    for (final mission in groupMissions(widget.board.sessions))
      (
        OoIconName.missions,
        'Mission ${mission.missionId}',
        'Operation ${mission.operationId} · ${mission.state}',
        OoPage.missions,
      ),
    for (final session in widget.board.sessions)
      (
        OoIconName.sessions,
        'Session ${session.sessionId}',
        'Mission ${session.missionId} · ${session.state ?? 'working'}',
        OoPage.sessions,
      ),
    for (final station in widget.board.stationBoard.stations)
      (
        OoIconName.stations,
        'Station ${station.stationId}',
        station.healthy ? 'online' : 'dark',
        OoPage.stations,
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
              tooltip: 'Search',
              onPressed: controller.openView,
              icon: const OoIcon(OoIconName.search),
            ),
      suggestionsBuilder: (context, controller) {
        final query = controller.text.trim().toLowerCase();
        final matches = _entries()
            .where(
              (entry) =>
                  widget.pages.contains(entry.$4) &&
                  (query.isEmpty ||
                      '${entry.$2} ${entry.$3}'.toLowerCase().contains(query)),
            )
            .take(24)
            .toList();
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
              'Results in the loaded board',
              style: TextStyle(color: tokens.muted),
            ),
          ),
          for (final (icon, title, detail, page) in matches)
            ListTile(
              leading: IconTheme.merge(
                data: IconThemeData(color: tokens.accent, size: 20),
                child: OoIcon(icon),
              ),
              title: Text(title),
              subtitle: Text(detail, style: TextStyle(color: tokens.muted)),
              onTap: () {
                controller.closeView(null);
                widget.onOpen(page);
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
    required this.session,
    required this.onPressed,
    this.compact = false,
    super.key,
  });

  final UserSessionSummary? session;
  final VoidCallback onPressed;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final name = _name(session);
    final initials = ooInitials(name);
    final group = session?.groups.firstOrNull;
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
      message: 'Operator account and preferences',
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

String? _name(UserSessionSummary? session) {
  final displayName = session?.displayName?.trim();
  final name = displayName?.isNotEmpty == true
      ? displayName
      : session?.name.trim();
  return name == null || name.isEmpty ? null : name;
}

/// Up to two initials of [name]: the first letter of its first two words that start with
/// a letter ("Ana Ortiz" is "AO", "Operator 007" is "O").
String? ooInitials(String? name) {
  if (name == null) return null;
  final letters = [
    for (final word in name.split(RegExp(r'\s+')))
      if (word.isNotEmpty && RegExp(r'^\p{L}', unicode: true).hasMatch(word))
        String.fromCharCode(word.runes.first).toUpperCase(),
  ];
  return letters.isEmpty ? null : letters.take(2).join();
}
