import 'package:material_ui/material_ui.dart';

import '../main.dart';

/// One Operation on the board with the Operations under it, as the person sees the tree.
@immutable
class OperationNode {
  const OperationNode({required this.access, required this.children});

  final UserOperationAccess access;
  final List<OperationNode> children;

  String get operationId => access.operationId;

  /// This Operation's ID and every one under it.
  Iterable<String> get subtree sync* {
    yield operationId;
    for (final child in children) {
      yield* child.subtree;
    }
  }
}

/// The Operations open to the person as a tree: each under its `parent`, roots and siblings in
/// ID order. An Operation whose parent is not open to the person (the gateway names only the
/// nearest readable ancestor) stands at the top.
List<OperationNode> operationTree(List<UserOperationAccess> operations) {
  final ids = {for (final access in operations) access.operationId};
  final byParent = <String?, List<UserOperationAccess>>{};
  for (final access in operations) {
    final parent = ids.contains(access.parent) ? access.parent : null;
    byParent.putIfAbsent(parent, () => []).add(access);
  }
  List<OperationNode> build(String? parent, Set<String> seen) {
    final children = [...?byParent[parent]]
      ..sort((left, right) => left.operationId.compareTo(right.operationId));
    return [
      for (final access in children)
        if (seen.add(access.operationId))
          OperationNode(
            access: access,
            children: build(access.operationId, seen),
          ),
    ];
  }

  return build(null, <String>{});
}

/// `root` and the IDs of every Operation under it, from each Operation's parent.
Set<String> subtreeIds(Map<String, String?> parents, String root) {
  final subtree = {root};
  var grew = true;
  while (grew) {
    grew = false;
    for (final MapEntry(key: id, value: parent) in parents.entries) {
      if (parent != null && subtree.contains(parent) && subtree.add(id)) {
        grew = true;
      }
    }
  }
  return subtree;
}

/// The Operations `operation` may be moved under: every other one but those in its own
/// subtree. The gateway's roster source still refuses a cycle.
List<String> parentChoices(Map<String, String?> parents, String operation) {
  final excluded = subtreeIds(parents, operation);
  return [
    for (final id in parents.keys)
      if (!excluded.contains(id)) id,
  ]..sort();
}

/// One Operation as its Stations resolve it, from
/// `GET /v1/user/roster/operation/{id}/effective`.
@immutable
class EffectiveOperationSummary {
  const EffectiveOperationSummary({
    required this.operationId,
    required this.lineage,
    required this.inherit,
    required this.definition,
    required this.origins,
  });

  final String operationId;

  /// Its ancestors, nearest first.
  final List<String> lineage;
  final bool inherit;
  final Map<String, dynamic> definition;

  /// Each written key, dotted, and the Operation that wrote it.
  final Map<String, String> origins;

  factory EffectiveOperationSummary.fromJson(Map<String, dynamic> json) =>
      EffectiveOperationSummary(
        operationId: json['operation_id'] as String,
        lineage: (json['lineage'] as List<dynamic>).cast<String>(),
        inherit: json['inherit'] as bool,
        definition: json['definition'] as Map<String, dynamic>,
        origins: (json['origins'] as Map<String, dynamic>)
            .cast<String, String>(),
      );

  /// The value of the dotted `key` in [definition], or `null`.
  Object? valueOf(String key) {
    Object? value = definition;
    for (final part in key.split('.')) {
      if (value is! Map<String, dynamic>) return null;
      value = value[part];
    }
    return value;
  }
}

/// Each key an Operation was resolved with and where it comes from: its own, or "from P" for
/// one inherited from an ancestor.
class EffectivePolicyView extends StatelessWidget {
  const EffectivePolicyView({required this.effective, super.key});

  final EffectiveOperationSummary effective;

  @override
  Widget build(BuildContext context) {
    final text = Theme.of(context).textTheme;
    final keys = effective.origins.keys.toList()..sort();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      spacing: 4,
      children: [
        Text(
          effective.lineage.isEmpty
              ? 'A root Operation'
              : 'Under ${effective.lineage.join(' › ')}',
          style: text.bodySmall,
        ),
        if (!effective.inherit)
          Text(
            'Takes no policy from above (inherit = false)',
            style: text.bodySmall,
          ),
        for (final key in keys)
          Row(
            spacing: 8,
            children: [
              Expanded(
                child: Text(
                  '$key = ${_shown(effective.valueOf(key))}',
                  style: text.bodyMedium,
                ),
              ),
              if (effective.origins[key] case final writer?
                  when writer != effective.operationId)
                Chip(
                  label: Text('from $writer'),
                  visualDensity: VisualDensity.compact,
                ),
            ],
          ),
      ],
    );
  }

  static String _shown(Object? value) => switch (value) {
    final String text => '"$text"',
    null => '—',
    _ => value.toString(),
  };
}
