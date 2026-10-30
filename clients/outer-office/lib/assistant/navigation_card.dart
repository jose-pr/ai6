import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import 'assistant_panel.dart' show MoneypennyAvatar;
import 'assistant_view.dart' show OutsideTextStrip, RefusedView;
import 'entry_kind.dart';

/// The route a navigation entry offers (its `data`), or why it is not one.
OoRoute navigationRoute(Map<String, Object?>? data) =>
    OoRoute.fromJson(data?['route']);

/// A page Moneypenny offers to open: the page and every field of its route as chips, built
/// from the route itself, her label beneath as her words, and Open. Opening only reads; an
/// entry she offered after reading outside text says so above it.
class NavigationCard extends StatelessWidget {
  const NavigationCard({required this.data, this.onOpen, super.key});

  /// The entry's `data`: `{id, route, label, exposed?}`.
  final Map<String, Object?>? data;

  /// Opens the route, as a link (`app:/<page>?...`); without it there is no Open.
  final void Function(String link)? onOpen;

  @override
  Widget build(BuildContext context) {
    final OoRoute route;
    try {
      route = navigationRoute(data);
    } on FormatException catch (error) {
      return RefusedView(reason: error.message);
    }
    final theme = Theme.of(context);
    final label = data?['label'];
    final open = onOpen;
    return Card(
      key: ValueKey('navigation-${data?['id']}'),
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 8,
          children: [
            if (data?['exposed'] == true) const OutsideTextStrip(),
            Row(
              spacing: 8,
              children: [
                const EntryKindIcon(EntryKind.navigation),
                Expanded(
                  child: Text(
                    route.page.label,
                    style: theme.textTheme.titleMedium,
                  ),
                ),
                if (open != null)
                  FilledButton(
                    onPressed: () => open(route.toUri()),
                    child: const Text('Open'),
                  ),
              ],
            ),
            if (route.keys.isNotEmpty)
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final key in route.keys)
                    Chip(
                      key: ValueKey('navigation-field-$key'),
                      visualDensity: VisualDensity.compact,
                      label: Text(revealInvisible(routeFieldLabel(route, key))),
                    ),
                ],
              ),
            if (label is String && label.isNotEmpty)
              Text(
                revealInvisible(label),
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.bodySmall,
              ),
          ],
        ),
      ),
    );
  }
}

/// Says the page in view was opened from one of Moneypenny's links or cards.
class OpenedByMoneypenny extends StatelessWidget {
  const OpenedByMoneypenny({super.key});

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
    child: Align(
      alignment: AlignmentDirectional.centerStart,
      child: Chip(
        key: const ValueKey('opened-by-moneypenny'),
        avatar: const ExcludeSemantics(child: MoneypennyAvatar(size: 20)),
        label: const Text('Opened by Moneypenny'),
      ),
    ),
  );
}
