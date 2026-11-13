import '../shell/operation_names.dart';

import 'package:material_ui/material_ui.dart';

import '../routes/oo_route.dart';
import '../shell/oo_shell.dart' show OoPage;
import '../theme/oo_tokens.dart';

/// Where the context sent with a message comes from.
enum ChatScopeMode {
  /// The page the person is on, as it changes.
  followPage,

  /// A page or Operation the person chose; navigation does not change it.
  pinned,

  /// None: she answers across everything the person may read.
  none,
}

/// What a route is about, in the words the scope chip shows: "Mission · m-7",
/// "Operation · guatiao", "Station · station-b", else the page.
String scopeLabel(OoRoute? route) {
  if (route == null) return 'All accessible Operations';
  for (final (field, word) in const [
    ('mission_id', 'Mission'),
    ('run_id', 'Run'),
    ('station_id', 'Station'),
    ('operation_id', 'Operation'),
  ]) {
    if (route[field] case final value?) return '$word · $value';
  }
  return 'Page · ${route.page.label}';
}

/// The scope above the composer: what the next message is about, a button to remove it, a
/// picker for an Operation, and, once the person has chosen a scope and moved to another
/// page, an offer to take that page as the scope instead. Nothing switches by itself.
class ChatScopeBar extends StatelessWidget {
  const ChatScopeBar({
    required this.scope,
    required this.operations,
    required this.onPick,
    required this.onClear,
    required this.onFollow,
    this.offered,
    this.following = true,
    super.key,
  });

  /// What messages are about now; null for everything.
  final OoRoute? scope;

  /// The page the person moved to, offered as the new scope; null when there is none.
  final OoRoute? offered;

  /// The Operations the person may pick.
  final List<String> operations;

  /// Whether the scope follows the page.
  final bool following;

  /// Pins an Operation or the offered page.
  final ValueChanged<OoRoute> onPick;

  /// Removes the scope.
  final VoidCallback onClear;

  /// Goes back to following the page.
  final VoidCallback onFollow;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final label = scopeLabel(scope);
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 4, 12, 0),
      child: Wrap(
        spacing: 8,
        runSpacing: 4,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Semantics(
            container: true,
            label: scope == null
                ? 'Scope: $label'
                : 'Scope: $label. Press the remove button to ask across everything.',
            child: InputChip(
              key: const ValueKey('chat-scope'),
              avatar: Icon(
                scope == null ? Icons.public : Icons.filter_alt_outlined,
                size: 16,
                color: tokens.accent,
              ),
              label: Text(label),
              onDeleted: scope == null ? null : onClear,
              deleteButtonTooltipMessage:
                  'Remove the scope: ask across everything',
            ),
          ),
          if (operations.isNotEmpty)
            PopupMenuButton<String>(
              key: const ValueKey('chat-scope-picker'),
              tooltip: 'Choose an Operation to ask about',
              icon: const Icon(Icons.arrow_drop_down_circle_outlined, size: 20),
              onSelected: (operation) => onPick(
                const OoRoute(OoPage.operations)
                    .withField('operation_id', operation),
              ),
              itemBuilder: (_) => [
                for (final operation in operations)
                  PopupMenuItem(
                    value: operation,
                    child: Text('Operation · ${OperationNames.of(operation)}'),
                  ),
              ],
            ),
          if (!following)
            TextButton(
              key: const ValueKey('chat-scope-follow'),
              onPressed: onFollow,
              child: const Text('Follow the page'),
            ),
          if (offered case final route?)
            ActionChip(
              key: const ValueKey('chat-scope-offer'),
              avatar: const Icon(Icons.add_location_alt_outlined, size: 16),
              label: Text('Ask about ${scopeLabel(route)} instead'),
              tooltip: 'Use this page as the scope',
              onPressed: () => onPick(route),
            ),
        ],
      ),
    );
  }
}
