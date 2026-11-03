import 'package:material_ui/material_ui.dart';

import '../shell/oo_link.dart';
import 'oo_route.dart';

/// The picker's value for every Operation the person may read: a route without
/// `operation_id`.
const allOperations = '\u0000all';

/// Which Operations a page reads, from its route: the one `operation_id` names when the person
/// may read it, every one they may when it names none, none when it names one they may not.
class OoScope {
  OoScope(this.route, this.readable);

  final OoRoute route;

  /// The Operations the person may read for this page.
  final List<String> readable;

  /// The one Operation named and readable; `null` for All or an unreadable one.
  String? get operation {
    final named = route['operation_id'];
    return named != null && readable.contains(named) ? named : null;
  }

  /// Whether the route names an Operation the person may not read.
  bool get unreadable => route['operation_id'] != null && operation == null;

  /// Whether every readable Operation is shown.
  bool get all => route['operation_id'] == null;

  /// The Operations to read.
  List<String> get shown => switch (operation) {
    final String operation => [operation],
    null when unreadable => const [],
    null => readable,
  };
}

/// Reads [load] for every Operation of [operations] at once. Rows come back with the Operation
/// they were read in; an Operation whose read fails keeps its rows from [previous] and is named
/// in `failed` with the reason.
Future<({List<(String, T)> rows, Map<String, String> failed})> readEach<T>(
  List<String> operations,
  Future<List<T>> Function(String operation) load,
  List<(String, T)> previous,
) async {
  final reads = await Future.wait([
    for (final operation in operations)
      load(operation).then<(String, List<T>?, String?)>(
        (rows) => (operation, rows, null),
        onError: (Object error) =>
            (operation, null, error is StateError ? error.message : '$error'),
      ),
  ]);
  return (
    rows: [
      for (final (operation, rows, _) in reads)
        ...(rows ??
                [
                  for (final (from, row) in previous)
                    if (from == operation) row,
                ])
            .map((row) => (operation, row)),
    ],
    failed: {for (final (operation, _, error) in reads) operation: ?error},
  );
}

/// The Operation picker of a page that reads Operations: "All Operations" first, the default,
/// then each Operation. A choice drops the route's [narrower] fields, which belong to one
/// Operation.
class OoOperationPicker extends StatelessWidget {
  const OoOperationPicker({
    required this.scope,
    required this.onRoute,
    this.narrower = const [],
    super.key,
  });

  final OoScope scope;
  final ValueChanged<OoRoute> onRoute;
  final List<String> narrower;

  @override
  Widget build(BuildContext context) {
    final operation = scope.operation;
    return DropdownButton<String>(
      value: operation ?? (scope.unreadable ? null : allOperations),
      hint: const Text('Operation'),
      items: [
        const DropdownMenuItem(
          value: allOperations,
          child: Text('All Operations'),
        ),
        for (final operation in scope.readable)
          DropdownMenuItem(value: operation, child: Text(operation)),
      ],
      onChanged: (chosen) {
        if (chosen == null) return;
        final all = chosen == allOperations;
        if (all ? scope.all : chosen == operation) return;
        var route = scope.route;
        for (final field in narrower) {
          route = route.without(field);
        }
        onRoute(
          all
              ? route.without('operation_id')
              : route.withField('operation_id', chosen),
        );
      },
    );
  }
}

/// The heading of one Operation's rows on a page showing All.
class OoOperationHeading extends StatelessWidget {
  const OoOperationHeading(this.operation, {super.key});

  final String operation;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(8, 16, 8, 4),
    child: OoLinkLine([
      ('Operation $operation', OoLinkTo.operation(operation)),
    ], style: Theme.of(context).textTheme.titleSmall),
  );
}

/// What failed to read, one line per Operation when several are shown.
class OoReadFailures extends StatelessWidget {
  const OoReadFailures({
    required this.failed,
    required this.several,
    super.key,
  });

  final Map<String, String> failed;
  final bool several;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 24),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final MapEntry(key: operation, value: why) in failed.entries)
          Text(several ? 'Operation $operation: $why' : why),
      ],
    ),
  );
}
