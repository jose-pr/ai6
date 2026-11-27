import 'package:coqui_acp/coqui_acp.dart' show AcpPermissionOption;

import 'limits.dart';
import 'text.dart';

/// The `_meta` key of an option's value that an agent lists only because it is in use: `false` says it cannot be chosen.
const String selectableMetaKey = 'dev.boriken.coqui.acp/selectable';

/// One value an option can take.
final class Choice {
  const Choice(
    this.value,
    this.label, {
    this.selectable = true,
    this.description,
  });
  final String value;
  final String label;

  /// What the agent says the value is, when it says: a model's price or what it suits, for a person choosing.
  final String? description;

  /// Whether the agent lets it be chosen: false for a value it lists only because it is in use.
  final bool selectable;
}

/// Something the agent lets a person choose, such as its model or its mode.
final class ConfigOption {
  ConfigOption({
    required this.id,
    required this.label,
    required this.current,
    required List<Choice> choices,
    this.isMode = false,
    this.category,
    this.defaultLabel,
    this.defaultName,
  }) : choices = List.unmodifiable(choices);
  final String id;
  final String label;
  final String current;
  final List<Choice> choices;

  /// What ACP says it is, such as `model`, `thought_level` or `mode`, when the agent says.
  final String? category;

  /// What taking no choice stands for, such as `Default (Opus 5.5)`, when a provider says.
  final String? defaultLabel;

  /// What taking no choice stands for, short, as a control reads it (`Opus 5.5`), when a provider says.
  final String? defaultName;

  /// Set with `session/set_mode` rather than `session/set_config_option`.
  final bool isMode;

  /// This option with [value] as its current one.
  ConfigOption withCurrent(String value) => ConfigOption(
    id: id,
    label: label,
    current: value,
    choices: choices,
    isMode: isMode,
    category: category,
    defaultLabel: defaultLabel,
    defaultName: defaultName,
  );

  /// This option as the update that carries it.
  Map<String, Object?> toJson() => {
    'id': id,
    'name': label,
    'category': ?(isMode ? 'mode' : category),
    'currentValue': current,
    'options': [
      for (final choice in choices)
        {
          'value': choice.value,
          'name': choice.label,
          'description': ?choice.description,
          if (!choice.selectable) '_meta': {selectableMetaKey: false},
        },
    ],
  };
}

Iterable<Object?> _flatChoices(List<dynamic> raw) sync* {
  for (final entry in raw) {
    // A group holds its own list of choices.
    if (entry is Map && entry['options'] is List) {
      yield* (entry['options'] as List);
    } else {
      yield entry;
    }
  }
}

/// The options of a `configOptions` list, skipping any that are not ACP's shape or whose current value is not among their
/// choices. Null when [raw] is not a list.
List<ConfigOption>? parseConfigOptions(Object? raw, ChatLimits limits) {
  if (raw is! List) return null;
  final parsed = <ConfigOption>[];
  for (final entry in raw.take(limits.configOptions)) {
    // ACP v2 calls it `configId`, and v1 `id`.
    final id = entry is Map
        ? idText(entry['configId'] ?? entry['id'], limits)
        : null;
    if (entry is! Map ||
        id == null ||
        idText(entry['currentValue'], limits) == null ||
        entry['options'] is! List) {
      continue;
    }
    final choices = _flatChoices(entry['options'] as List)
        .take(limits.choices)
        .whereType<Map>()
        .where((choice) => idText(choice['value'], limits) != null)
        .map(
          (choice) => Choice(
            choice['value'] as String,
            shortText(choice['name'], limits) ?? choice['value'] as String,
            selectable: !_unselectable(choice['_meta']),
            description: shortText(choice['description'], limits),
          ),
        )
        .toList();
    final current = entry['currentValue'] as String;
    if (!choices.any((choice) => choice.value == current)) continue;
    parsed.add(
      ConfigOption(
        id: id,
        label: shortText(entry['name'], limits) ?? id,
        current: current,
        choices: choices,
        category: shortText(entry['category'], limits),
      ),
    );
  }
  return parsed;
}

bool _unselectable(Object? meta) =>
    meta is Map && meta[selectableMetaKey] == false;

/// ACP v1's `modes` as an option; null when it is not a mode state or its current mode is not among those offered.
ConfigOption? parseModes(Object? raw, ChatLimits limits) {
  if (raw is! Map ||
      idText(raw['currentModeId'], limits) == null ||
      raw['availableModes'] is! List) {
    return null;
  }
  final choices = (raw['availableModes'] as List)
      .take(limits.choices)
      .whereType<Map>()
      .where((mode) => idText(mode['id'], limits) != null)
      .map(
        (mode) => Choice(
          mode['id'] as String,
          shortText(mode['name'], limits) ?? mode['id'] as String,
          description: shortText(mode['description'], limits),
        ),
      )
      .toList();
  final current = raw['currentModeId'] as String;
  if (!choices.any((choice) => choice.value == current)) return null;
  return ConfigOption(
    id: 'mode',
    label: 'Mode',
    current: current,
    choices: choices,
    isMode: true,
  );
}

/// Where an agent's choice belongs: its session mode, its model, its reasoning effort, or elsewhere.
enum OptionRole { mode, model, effort, other }

/// The role of [option]: by its ACP `category` where the agent gives one, else by its id.
OptionRole roleOf(ConfigOption option) {
  if (option.isMode) return OptionRole.mode;
  switch (option.category) {
    case 'mode':
      return OptionRole.mode;
    case 'model':
      return OptionRole.model;
    case 'thought_level':
      return OptionRole.effort;
    case final String _:
      return OptionRole.other;
  }
  return switch (option.id) {
    'mode' => OptionRole.mode,
    'model' => OptionRole.model,
    'effort' || 'reasoning_effort' || 'thought_level' => OptionRole.effort,
    _ => OptionRole.other,
  };
}

/// The first of [options] in [role].
ConfigOption? optionIn(List<ConfigOption> options, OptionRole role) =>
    options.where((option) => roleOf(option) == role).firstOrNull;

/// The mode option among [options].
ConfigOption? modeOptionIn(List<ConfigOption> options) =>
    optionIn(options, OptionRole.mode);

/// A choice made before the agent can take it: while a turn runs, or before it starts. It goes to the agent before the next
/// prompt, to the option of the same id, else to one in the same [role] that offers [value].
final class PendingChoice {
  const PendingChoice(this.id, this.role, this.value, {this.byPerson = false});
  final String id;
  final OptionRole role;
  final String value;

  /// Whether a person chose it, rather than its being a default a host put there.
  final bool byPerson;

  /// This choice as a host keeps it between runs, which [fromJson] reads back.
  Map<String, Object?> toJson() => {
    'id': id,
    'role': role.name,
    'value': value,
    if (byPerson) 'byPerson': true,
  };

  /// The choice [toJson] wrote; null for anything else.
  static PendingChoice? fromJson(Object? json) {
    if (json is! Map || json['id'] is! String || json['value'] is! String) {
      return null;
    }
    final role = OptionRole.values
        .where((role) => role.name == json['role'])
        .firstOrNull;
    return role == null
        ? null
        : PendingChoice(
            json['id'] as String,
            role,
            json['value'] as String,
            byPerson: json['byPerson'] == true,
          );
  }
}

/// The ids an agent may give a mode that asks for nothing: the Rust translators' own, and the names Claude Code and others use.
const Set<String> _bypassIds = {
  'bypass',
  'bypasspermissions',
  'full_access',
  'full-access',
  'fullaccess',
  'yolo',
};

/// Whether a mode of [value], or labelled [label], asks for nothing.
bool isBypassMode(String value, [String? label]) =>
    _bypassIds.contains(value.toLowerCase()) ||
    label?.toLowerCase() == 'bypass';

/// The id of the bypass a chat provides itself, by answering requests, for an agent that has no mode that asks for nothing.
const String ownBypass = 'bypass';

/// The option a request in bypass is answered with: the first that allows once, else the first that allows always unless
/// [onceOnly]. Null when it offers neither: that request is the person's.
AcpPermissionOption? bypassAnswer(
  List<AcpPermissionOption> options, {
  bool onceOnly = false,
}) =>
    options.where((option) => option.kind == 'allow_once').firstOrNull ??
    (onceOnly
        ? null
        : options.where((option) => option.kind == 'allow_always').firstOrNull);

/// The mode ids the Rust translators (`coqui_acp_harness::mode`) offer, in the order a client cycles them.
const List<String> modeOrder = [
  'manual',
  'accept_edits',
  'auto',
  'plan',
  'bypass',
];

/// One entry of a mode menu: an agent's own mode, or the Bypass the chat provides where the agent has no mode that asks for nothing.
final class ModeEntry {
  const ModeEntry(
    this.value,
    this.label, {
    this.own = false,
    this.selectable = true,
  });
  final String value;

  /// The agent's name for it ([ownBypass] is called `Bypass`); a UI words it for a person.
  final String label;

  /// Whether it can be chosen: the agent's own `custom`, settings no mode names, only says what it is in.
  final bool selectable;

  /// Whether the chat provides it, by answering the agent's requests, rather than the agent.
  final bool own;

  bool get bypass => isBypassMode(value, label);
}

/// The modes [option] offers, and Bypass beside them when none of them asks for nothing. [label] words a mode for a person where
/// the agent's own name for it is not what to show.
List<ModeEntry> modeEntries(
  ConfigOption? option, {
  String Function(Choice choice)? label,
}) {
  final entries = [
    for (final choice in option?.choices ?? const <Choice>[])
      ModeEntry(
        choice.value,
        label?.call(choice) ?? choice.label,
        selectable: choice.selectable && choice.value != 'custom',
      ),
  ];
  if (!entries.any((entry) => entry.bypass)) {
    entries.add(const ModeEntry(ownBypass, 'Bypass', own: true));
  }
  return entries;
}

/// The entry after [current] in [entries], in the order of [modeOrder] where the ids are known and the agent's own order
/// otherwise, the first entry when [current] is not among them; never one that cannot be chosen or is in [skip]. Null when there is
/// none.
ModeEntry? nextMode(
  List<ModeEntry> entries,
  String? current, {
  Set<String> skip = const {},
}) {
  final ordered = [...entries];
  int rank(ModeEntry entry) {
    final at = modeOrder.indexOf(entry.bypass ? ownBypass : entry.value);
    return at < 0 ? modeOrder.length : at;
  }

  if (ordered.every(
    (entry) => entry.value == 'custom' || rank(entry) < modeOrder.length,
  )) {
    ordered.sort((a, b) => rank(a).compareTo(rank(b)));
  }
  final from = ordered.indexWhere((entry) => entry.value == current);
  for (var step = 1; step <= ordered.length; step++) {
    final entry = ordered[(from + step) % ordered.length];
    if (entry.selectable &&
        !skip.contains(entry.value) &&
        entry.value != current) {
      return entry;
    }
  }
  return null;
}
