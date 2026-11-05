import 'package:material_ui/material_ui.dart';

import '../theme/oo_tokens.dart';

/// What a person may do in one area, in plain words, and the actions that say it.
@immutable
class AccessGroup {
  const AccessGroup(this.name, this.words, this.actions);

  final String name;

  /// What the actions allow, such as `see, queue new`.
  final List<String> words;
  final List<String> actions;
}

/// The areas, in the order they are shown, with the plain words for each action.
const _areas = <(String, Map<String, String>)>[
  (
    'Missions',
    {
      'mission.read': 'see',
      'mission.create': 'queue new',
      'mission.control': 'cancel, retry, resume, interrupt',
      'mission.verify': 'give a verdict on',
      'mission.review': 'decide over a review of',
    },
  ),
  (
    'Sessions',
    {
      'session.read': 'watch',
      'session.input': 'type into',
      'session.context': 'read the context of',
    },
  ),
  (
    'Linked issues',
    {
      'issue.link': 'link',
      'issue.comment': 'comment on',
      'issue.update': 'change',
      'issue.create': 'create',
    },
  ),
  (
    'Green ink',
    {
      'approval.decide': 'decide',
      'approval.class': 'manage class authorisations',
    },
  ),
  ('Agents', {'agent.read': 'see', 'agent.configure': 'change'}),
  ('Pools', {'pool.read': 'see', 'pool.manage': 'manage'}),
  ('Cables', {'cable.read': 'read', 'cable.send': 'send'}),
  ('Audit', {'audit.read': 'read'}),
  ('Access', {'access.read': 'see', 'access.manage': 'change'}),
];

/// [actions] by area in plain words; an action this build has no words for goes under
/// **Other** by its own name, so a new action on the gateway is never hidden.
List<AccessGroup> accessGroups(Iterable<String> actions) {
  final held = actions.toSet();
  final groups = <AccessGroup>[];
  for (final (name, words) in _areas) {
    final own = [
      for (final action in words.keys)
        if (held.remove(action)) action,
    ];
    if (own.isNotEmpty) {
      groups.add(
        AccessGroup(name, [for (final action in own) words[action]!], own),
      );
    }
  }
  if (held.isNotEmpty) {
    final other = held.toList()..sort();
    groups.add(AccessGroup('Other', other, other));
  }
  return groups;
}

/// What the person may do with [actions]: a line per area in plain words, the action names on
/// hover (a long press on a phone).
class OoAccessSummary extends StatelessWidget {
  const OoAccessSummary(this.actions, {super.key});

  final List<String> actions;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final style = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: tokens.muted);
    final groups = accessGroups(actions);
    if (groups.isEmpty) return Text('No actions granted', style: style);
    return Wrap(
      spacing: 16,
      runSpacing: 2,
      children: [
        for (final group in groups)
          Tooltip(
            message: group.actions.join(', '),
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: '${group.name}: ',
                    style: TextStyle(color: tokens.text),
                  ),
                  TextSpan(text: group.words.join(', ')),
                ],
              ),
              style: style,
            ),
          ),
      ],
    );
  }
}
