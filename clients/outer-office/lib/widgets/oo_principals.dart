import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../theme/oo_tokens.dart';

/// A person the gateway has seen sign in, by the name their directory gave it.
@immutable
class OoPerson {
  const OoPerson({
    required this.id,
    required this.label,
    this.name = '',
    this.groups = const [],
  });

  /// The directory's person ID, as an access rule names it.
  final String id;

  /// The name to show: the display name, else the account name.
  final String label;

  /// The account name.
  final String name;
  final List<String> groups;

  factory OoPerson.fromJson(Map<String, dynamic> json) => OoPerson(
    id: json['id'] as String,
    label: json['label'] as String,
    name: json['name'] as String? ?? '',
    groups: (json['groups'] as List<dynamic>? ?? const []).cast<String>(),
  );
}

/// What a search for people and groups found.
@immutable
class OoPrincipalHits {
  const OoPrincipalHits({this.people = const [], this.groups = const []});

  final List<OoPerson> people;
  final List<String> groups;
}

/// What a principal as written names: a kind and the rest, such as `person:1000`. A bare ID
/// (an audit record's actor) is a person's; `ID via key K` is that person acting through a key.
@immutable
class OoPrincipalRef {
  const OoPrincipalRef(this.kind, this.id, {this.key});

  /// `person`, `group`, `account`, `station` or `cli`.
  final String kind;
  final String id;

  /// The API key a person acted through, when the text said so.
  final String? key;

  /// Whether the gateway can name it: only a person is in its directory of people.
  bool get isPerson => kind == 'person';

  static const _kinds = {'person', 'group', 'account', 'station', 'cli'};

  factory OoPrincipalRef.parse(String text) {
    var rest = text.trim();
    String? key;
    final via = rest.indexOf(' via key ');
    if (via >= 0) {
      key = rest.substring(via + ' via key '.length);
      rest = rest.substring(0, via);
    }
    final colon = rest.indexOf(':');
    if (colon > 0 && _kinds.contains(rest.substring(0, colon))) {
      return OoPrincipalRef(
        rest.substring(0, colon),
        rest.substring(colon + 1),
        key: key,
      );
    }
    return OoPrincipalRef('person', rest, key: key);
  }

  /// The principal as an access rule writes it.
  String get subject => '$kind:$id';
}

/// The names of people, learned from the gateway and kept for the run.
///
/// Asking for an ID the gateway has never seen is answered once, as unknown: the ID is shown,
/// muted. A name is for display only; nothing here decides what a person may do.
class OoPrincipals extends ChangeNotifier {
  OoPrincipals({required this.lookup, required this.search});

  /// Reads the people among the IDs the gateway has seen sign in.
  final Future<List<OoPerson>> Function(List<String> ids) lookup;

  /// Finds people and groups whose name, account name or ID holds every word of the text.
  final Future<OoPrincipalHits> Function(String text) search;

  final _known = <String, OoPerson>{};
  final _asked = <String>{};
  final _wanted = <String>{};
  bool _scheduled = false;
  bool _disposed = false;

  /// The name of person [id]; `null` until the gateway has told, or when it has never seen them.
  String? labelOf(String id) => _known[id]?.label;

  /// The person [id], when known.
  OoPerson? person(String id) => _known[id];

  /// Remembers names already in hand, such as those a response carries.
  void learn(Iterable<OoPerson> people) {
    var changed = false;
    for (final person in people) {
      if (_known[person.id]?.label != person.label) {
        _known[person.id] = person;
        changed = true;
      }
      _asked.add(person.id);
    }
    if (changed) notifyListeners();
  }

  /// Asks for the names of [ids] not asked for yet, in one request after the frame.
  void want(Iterable<String> ids) {
    for (final id in ids) {
      if (id.isNotEmpty && _asked.add(id)) _wanted.add(id);
    }
    if (_wanted.isEmpty || _scheduled) return;
    _scheduled = true;
    scheduleMicrotask(_flush);
  }

  Future<void> _flush() async {
    _scheduled = false;
    final ids = _wanted.toList()..sort();
    _wanted.clear();
    if (ids.isEmpty) return;
    try {
      final people = await lookup(ids);
      if (_disposed) return;
      for (final person in people) {
        _known[person.id] = person;
      }
      notifyListeners();
    } on Object {
      // A failed read leaves the IDs shown; a later reload asks again.
      _asked.removeAll(ids);
    }
  }

  /// Forgets what was asked, so the next view asks again; names kept stay until replaced.
  void refresh() => _asked
    ..clear()
    ..addAll(_known.keys);

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}

/// Gives the pages below it the run's [OoPrincipals].
class OoPrincipalsScope extends InheritedNotifier<OoPrincipals> {
  const OoPrincipalsScope({
    required OoPrincipals principals,
    required super.child,
    super.key,
  }) : super(notifier: principals);

  static OoPrincipals? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<OoPrincipalsScope>()?.notifier;
}

/// A principal as a person reads it: the person's name, and the ID in the tooltip. A person the
/// gateway has never seen shows as the ID, muted. Groups, accounts, Stations and command-line
/// users show as written.
class OoPrincipalBadge extends StatelessWidget {
  const OoPrincipalBadge(
    this.principal, {
    this.style,
    this.knownName,
    this.overflow = TextOverflow.ellipsis,
    super.key,
  });

  /// The principal as the gateway wrote it: `person:ID`, `group:NAME`, `ID`, `ID via key K`.
  final String principal;
  final TextStyle? style;

  /// The name the response carried, used before the directory is asked.
  final String? knownName;
  final TextOverflow overflow;

  @override
  Widget build(BuildContext context) {
    final ref = OoPrincipalRef.parse(principal);
    final tokens = OoTokens.of(context);
    final base = style ?? DefaultTextStyle.of(context).style;
    final muted = base.copyWith(color: tokens.muted);
    if (!ref.isPerson) {
      return Text(ref.subject, style: base, overflow: overflow);
    }
    final principals = OoPrincipalsScope.maybeOf(context);
    if (knownName == null) principals?.want([ref.id]);
    final label = knownName ?? principals?.labelOf(ref.id);
    final via = ref.key == null ? '' : ' via key ${ref.key}';
    if (label == null) {
      return Tooltip(
        message: 'Not seen signing in: ${ref.subject}',
        child: Text('${ref.id}$via', style: muted, overflow: overflow),
      );
    }
    return Tooltip(
      message: ref.subject,
      child: Text.rich(
        TextSpan(
          children: [
            TextSpan(text: label, style: base),
            if (via.isNotEmpty) TextSpan(text: via, style: muted),
          ],
        ),
        overflow: overflow,
      ),
    );
  }
}

/// The text a principal reads as in a place that cannot hold a widget (a sort key, a filter):
/// the person's name when known, else the ID.
String ooPrincipalText(OoPrincipals? principals, String principal) {
  final ref = OoPrincipalRef.parse(principal);
  if (ref.isPerson) return principals?.labelOf(ref.id) ?? ref.id;
  return ref.subject;
}

/// A field for a rule's subject that finds a person or a group by name as it is typed, and still
/// takes `person:ID`, `group:NAME` or `account:NAME` written out.
class OoPrincipalPicker extends StatefulWidget {
  const OoPrincipalPicker({
    required this.controller,
    required this.principals,
    this.label = 'Subject (a name, or account:NAME, person:ID, group:NAME)',
    this.accounts = const [],
    super.key,
  });

  /// Holds the subject as an access rule writes it.
  final TextEditingController controller;
  final OoPrincipals principals;
  final String label;

  /// Local account names the gateway keeps, offered beside the people and groups.
  final List<String> accounts;

  @override
  State<OoPrincipalPicker> createState() => _OoPrincipalPickerState();
}

class _OoPrincipalPickerState extends State<OoPrincipalPicker> {
  final _focus = FocusNode();
  Timer? _debounce;
  int _asked = 0;
  List<(String, String)> _options = const [];

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_typed);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_typed);
    _debounce?.cancel();
    _focus.dispose();
    super.dispose();
  }

  void _typed() {
    final text = widget.controller.text.trim();
    _debounce?.cancel();
    // A subject already written out is not searched for.
    if (text.isEmpty || RegExp(r'^(person|group|account):').hasMatch(text)) {
      if (_options.isNotEmpty) setState(() => _options = const []);
      return;
    }
    _debounce = Timer(const Duration(milliseconds: 200), () => _find(text));
  }

  Future<void> _find(String text) async {
    final asked = ++_asked;
    OoPrincipalHits hits;
    try {
      hits = await widget.principals.search(text);
    } on Object {
      hits = const OoPrincipalHits();
    }
    if (!mounted || asked != _asked) return;
    final lower = text.toLowerCase();
    setState(
      () => _options = [
        for (final person in hits.people) (person.label, 'person:${person.id}'),
        for (final group in hits.groups) (group, 'group:$group'),
        for (final account in widget.accounts)
          if (account.toLowerCase().contains(lower))
            (account, 'account:$account'),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        TextField(
          key: const ValueKey('principal-picker'),
          controller: widget.controller,
          focusNode: _focus,
          decoration: InputDecoration(labelText: widget.label),
        ),
        if (_options.isNotEmpty)
          Container(
            key: const ValueKey('principal-options'),
            constraints: const BoxConstraints(maxHeight: 220),
            child: ListView(
              shrinkWrap: true,
              children: [
                for (final (label, subject) in _options)
                  ListTile(
                    dense: true,
                    title: Text(label),
                    subtitle: Text(
                      subject,
                      style: TextStyle(color: tokens.muted),
                    ),
                    onTap: () {
                      widget.controller.text = subject;
                      setState(() => _options = const []);
                    },
                  ),
              ],
            ),
          ),
      ],
    );
  }
}
