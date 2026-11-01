import 'package:material_ui/material_ui.dart';

import '../routes/oo_route.dart';
import '../theme/oo_tokens.dart';
import 'oo_shell.dart';

/// What a name in a page can open: the pages this person may open and how to go there. Set once
/// by the board page, so a link deep in a card needs nothing passed down.
class OoLinks extends InheritedWidget {
  const OoLinks({
    required this.pages,
    required this.onRoute,
    required super.child,
    super.key,
  });

  final List<OoPage> pages;
  final ValueChanged<OoRoute> onRoute;

  static OoLinks? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<OoLinks>();

  /// What opens [route] from here; null where the person may not, so a control is disabled
  /// rather than dead.
  static VoidCallback? opener(BuildContext context, OoRoute route) {
    final links = maybeOf(context);
    return links != null && links.pages.contains(route.page)
        ? () => links.onRoute(route)
        : null;
  }

  @override
  bool updateShouldNotify(OoLinks old) =>
      old.pages != pages || old.onRoute != onRoute;
}

/// The routes names open. An ID from outside AI6 may not be one a route takes: the page alone
/// then, so a name still goes where it can.
abstract final class OoLinkTo {
  static OoRoute _route(OoPage page, Map<String, String?> fields) {
    try {
      var route = OoRoute(page);
      for (final MapEntry(:key, :value) in fields.entries) {
        if (value != null) route = route.withField(key, value);
      }
      return route;
    } on FormatException {
      return OoRoute(page);
    }
  }

  static OoRoute operation(String operation) =>
      _route(OoPage.operations, {'operation_id': operation});

  static OoRoute mission(String operation, String mission) => _route(
    OoPage.missions,
    {'operation_id': operation, 'mission_id': mission},
  );

  /// The Missions of [operation] in [state], or every one.
  static OoRoute missions({String? operation, List<String> states = const []}) {
    try {
      var route = _route(OoPage.missions, {'operation_id': operation});
      if (states.isNotEmpty) route = route.withField('state', states);
      return route;
    } on FormatException {
      return const OoRoute(OoPage.missions);
    }
  }

  static OoRoute session(String operation, String session) => _route(
    OoPage.sessions,
    {'operation_id': operation, 'session_id': session},
  );

  /// The Sessions of a [state], such as those a tile counts.
  static OoRoute sessions({String? state}) =>
      _route(OoPage.sessions, {'state': state});

  static OoRoute station(String station) =>
      _route(OoPage.stations, {'station_id': station});

  static OoRoute theater(String station, String theater) => _route(
    OoPage.stations,
    {'tab': 'theaters', 'station_id': station, 'theater_id': theater},
  );

  static OoRoute agent(String agent, {String? operation}) => _route(
    OoPage.agents,
    {'tab': 'agents', 'operation_id': operation, 'agent': agent},
  );

  static OoRoute role(String role, {String? operation}) => _route(
    OoPage.agents,
    {'tab': 'roles', 'operation_id': operation, 'role': role},
  );

  static OoRoute pool(String pool, {String? operation}) =>
      _route(OoPage.pools, {'operation_id': operation, 'pool': pool});

  static OoRoute heldBrief(String operation, String key) => _route(
    OoPage.briefings,
    {'tab': 'held', 'operation_id': operation, 'item_id': key},
  );
}

/// [text], a name that opens [route]: the accent's colour, underlined under the pointer or
/// keyboard focus, the pointer's hand, reached by Tab and opened by Enter, announced as a link,
/// and at least [OoLink.phoneTarget] tall on a phone. Plain text where the person may not open
/// that page, so there is never a dead link.
class OoLink extends StatefulWidget {
  const OoLink(
    this.text, {
    required this.route,
    this.style,
    this.semanticLabel,
    this.maxLines,
    this.onTap,
    super.key,
  });

  /// The tap target a name keeps on a phone, in logical pixels.
  static const double phoneTarget = 48;

  final String text;
  final OoRoute route;
  final TextStyle? style;

  /// What a screen reader says instead of "Open [text]".
  final String? semanticLabel;
  final int? maxLines;

  /// Opens it some other way than going to [route], such as beside the list it sits in.
  final VoidCallback? onTap;

  @override
  State<OoLink> createState() => _OoLinkState();
}

class _OoLinkState extends State<OoLink> {
  bool _lit = false;

  @override
  Widget build(BuildContext context) {
    final style = widget.style ?? DefaultTextStyle.of(context).style;
    final links = OoLinks.maybeOf(context);
    final overflow = widget.maxLines == null ? null : TextOverflow.ellipsis;
    if (links == null || !links.pages.contains(widget.route.page)) {
      return Text(
        widget.text,
        style: style,
        maxLines: widget.maxLines,
        overflow: overflow,
      );
    }
    final tokens = OoTokens.of(context);
    final phone = MediaQuery.sizeOf(context).width < OoBreakpoints.phone;
    final open = widget.onTap ?? () => links.onRoute(widget.route);
    return Semantics(
      link: true,
      label: widget.semanticLabel ?? 'Open ${widget.text}',
      excludeSemantics: true,
      onTap: open,
      child: InkWell(
        onTap: open,
        onHover: (lit) => setState(() => _lit = lit),
        onFocusChange: (lit) => setState(() => _lit = lit),
        focusColor: tokens.accent.withValues(alpha: 0.2),
        hoverColor: Colors.transparent,
        borderRadius: BorderRadius.circular(4),
        child: ConstrainedBox(
          constraints: BoxConstraints(
            minHeight: phone ? OoLink.phoneTarget : 0,
          ),
          child: Align(
            alignment: Alignment.centerLeft,
            widthFactor: 1,
            heightFactor: 1,
            child: Text(
              widget.text,
              maxLines: widget.maxLines,
              overflow: overflow,
              style: style.copyWith(
                color: tokens.accent,
                decoration: _lit ? TextDecoration.underline : null,
                decorationColor: tokens.accent,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A line of names and words, such as `Operation G · Mission m-1 · by ana`: each item is a word
/// (`(text, null)`) or a name that opens its route, kept apart by [separator], wrapping as the
/// width needs.
class OoLinkLine extends StatelessWidget {
  const OoLinkLine(this.items, {this.style, this.separator = ' · ', super.key});

  final List<(String, OoRoute?)> items;
  final TextStyle? style;
  final String separator;

  @override
  Widget build(BuildContext context) {
    final text =
        style ??
        Theme.of(context).textTheme.bodySmall
            ?.copyWith(color: OoTokens.of(context).muted);
    return Wrap(
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        for (final (index, (word, route)) in items.indexed) ...[
          if (index > 0) Text(separator, style: text),
          if (route == null)
            Text(word, style: text)
          else
            OoLink(word, route: route, style: text),
        ],
      ],
    );
  }
}
