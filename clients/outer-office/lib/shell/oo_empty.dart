import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_route.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'oo_freshness.dart';
import 'oo_link.dart';

/// Why [source] has nothing to show when that is not because it is empty: the person may not
/// read it, or the read failed. Null while it can be taken at its word.
Widget? boardProblem(MissionBoard board, BoardSource source) =>
    switch (board.freshness(source)) {
      OoFreshness.noAccess => OoEmptyState(
        icon: OoIconName.settings,
        title: 'Your account may not read ${source.label}.',
        body: 'A controller grants access.',
      ),
      OoFreshness.unavailable => OoEmptyState(
        icon: OoIconName.notifications,
        title: '${source.label} could not be read.',
        body: 'Universal Exports did not answer. The app keeps trying.',
        action: ('Try again', () => unawaited(board.refresh())),
      ),
      _ => null,
    };

/// A page with nothing to list: what it is for and what to do first, not only "No X".
/// [link] opens another page, [action] does something here; [actions] are for several.
class OoEmptyState extends StatelessWidget {
  const OoEmptyState({
    required this.icon,
    required this.title,
    required this.body,
    this.link,
    this.action,
    this.actions = const [],
    this.backdrop = false,
    super.key,
  });

  final OoIconName icon;
  final String title;
  final String body;

  /// A label and the page it opens; shown only when the person may open that page.
  final (String, OoRoute)? link;

  /// A label and what it does.
  final (String, VoidCallback)? action;
  final List<(String, VoidCallback)> actions;

  /// Whether the wallpaper shows behind it; not on pages that must stay plain, such as green ink.
  final bool backdrop;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final open = switch (link) {
      (final label, final route) => switch (OoLinks.opener(context, route)) {
        final open? => (label, open),
        null => null,
      },
      null => null,
    };
    final content = ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 520),
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            IconTheme.merge(
              data: IconThemeData(size: 42, color: tokens.muted),
              child: OoIcon(icon),
            ),
            const SizedBox(height: 12),
            Semantics(
              header: true,
              child: Text(
                title,
                textAlign: TextAlign.center,
                style: text.titleMedium,
              ),
            ),
            const SizedBox(height: 6),
            Text(
              body,
              textAlign: TextAlign.center,
              style: text.bodyMedium?.copyWith(color: tokens.muted),
            ),
            if (open != null || action != null || actions.isNotEmpty) ...[
              const SizedBox(height: 16),
              Wrap(
                alignment: WrapAlignment.center,
                spacing: 8,
                runSpacing: 8,
                children: [
                  for (final (label, onPressed) in [...actions, ?action])
                    FilledButton(onPressed: onPressed, child: Text(label)),
                  if (open case (final label, final onPressed))
                    OutlinedButton(onPressed: onPressed, child: Text(label)),
                ],
              ),
            ],
          ],
        ),
      ),
    );
    final scrolled = SingleChildScrollView(child: content);
    // The wallpaper fills the page: around only the text it would be a band with hard edges.
    return backdrop
        ? SizedBox.expand(
            child: OoBackdrop(alignment: Alignment.center, child: scrolled),
          )
        : Center(child: scrolled);
  }
}
