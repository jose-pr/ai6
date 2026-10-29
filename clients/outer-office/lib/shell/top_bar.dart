import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_theme.dart';
import 'oo_shell.dart';

/// One thing that needs the person: a blocked Session, a held brief or a dark Station,
/// and the page that deals with it.
@immutable
class OoAttention {
  const OoAttention({
    required this.icon,
    required this.color,
    required this.text,
    required this.page,
  });

  final OoIconName icon;
  final Color color;
  final String text;
  final OoPage page;
}

/// What needs the person's attention, from what the board has loaded: Sessions waiting
/// for green ink or otherwise blocked, held briefs, and dark Stations.
List<OoAttention> attentionItems(MissionBoard board, OoTokens tokens) => [
  for (final session in board.sessions.where(
    (session) => session.state == 'blocked',
  ))
    session.reason == 'green-ink'
        ? OoAttention(
            icon: OoIconName.greenInk,
            color: tokens.warning,
            text:
                'Session ${session.sessionId} of Mission '
                '${session.missionId} waits for green ink',
            page: OoPage.greenInk,
          )
        : OoAttention(
            icon: OoIconName.interrupt,
            color: tokens.warning,
            text:
                'Session ${session.sessionId} of Mission '
                '${session.missionId} is blocked'
                '${session.reason == null ? '' : ': ${session.reason!.replaceAll('-', ' ')}'}',
            page: OoPage.missions,
          ),
  for (final brief in board.heldBriefs)
    OoAttention(
      icon: OoIconName.briefings,
      color: tokens.warning,
      text:
          // A brief from a Source is untrusted text: show what renders as nothing.
          'Brief "${revealInvisible(brief.title)}" is held for review in Operation '
          '${brief.operationId}',
      page: OoPage.briefings,
    ),
  for (final station in board.stationBoard.stations.where(
    (station) => !station.healthy,
  ))
    OoAttention(
      icon: OoIconName.stations,
      color: tokens.danger,
      text: 'Station ${station.stationId} has gone dark',
      page: OoPage.stations,
    ),
];

/// The notifications bell: a count of [attentionItems] and a menu listing them, each
/// opening its page when the person may open it.
class AttentionBell extends StatelessWidget {
  const AttentionBell({
    required this.board,
    required this.pages,
    required this.onOpen,
    super.key,
  });

  final MissionBoard board;
  final List<OoPage> pages;
  final ValueChanged<OoPage> onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final items = attentionItems(board, tokens);
    return MenuAnchor(
      alignmentOffset: const Offset(-260, 4),
      menuChildren: [
        if (items.isEmpty)
          const MenuItemButton(child: Text('Nothing needs your attention.'))
        else
          for (final item in items)
            MenuItemButton(
              leadingIcon: IconTheme.merge(
                data: IconThemeData(color: item.color, size: 20),
                child: OoIcon(item.icon),
              ),
              onPressed: pages.contains(item.page)
                  ? () => onOpen(item.page)
                  : null,
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 320),
                child: Text(
                  item.text,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ),
      ],
      builder: (context, controller, _) => IconButton(
        tooltip: items.isEmpty
            ? 'Notifications'
            : 'Notifications: ${items.length} need your attention',
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
/// opening its page. A field when [expanded], else a button opening the same view.
class OoSearch extends StatelessWidget {
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

  /// Everything searchable, as (icon, title, detail, page).
  List<(OoIconName, String, String, OoPage)> _entries() => [
    for (final operation
        in board.userSession?.operations ?? const <UserOperationAccess>[])
      (
        OoIconName.operations,
        'Operation ${operation.operationId}',
        'Operation',
        OoPage.operations,
      ),
    for (final mission in groupMissions(board.sessions))
      (
        OoIconName.missions,
        'Mission ${mission.missionId}',
        'Operation ${mission.operationId} · ${mission.state}',
        OoPage.missions,
      ),
    for (final session in board.sessions)
      (
        OoIconName.sessions,
        'Session ${session.sessionId}',
        'Mission ${session.missionId} · ${session.state ?? 'working'}',
        OoPage.sessions,
      ),
    for (final station in board.stationBoard.stations)
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
      viewHintText: hint,
      viewBackgroundColor: tokens.surface,
      dividerColor: tokens.border,
      // The field only opens the search view, which takes the typing: the top bar holds no
      // text field of its own.
      builder: (context, controller) => expanded
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
                            hint,
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
                  pages.contains(entry.$4) &&
                  (query.isEmpty ||
                      '${entry.$2} ${entry.$3}'.toLowerCase().contains(query)),
            )
            .take(24)
            .toList();
        if (matches.isEmpty) {
          return [
            ListTile(
              title: Text(
                query.isEmpty ? 'Nothing on the board yet.' : 'No matches.',
              ),
            ),
          ];
        }
        return [
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
                onOpen(page);
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
        border: Border.all(color: tokens.accent.withValues(alpha: 0.7)),
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
                            style: TextStyle(color: tokens.muted, fontSize: 12),
                          ),
                      ],
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
