import 'package:material_ui/material_ui.dart';

import '../theme/oo_tokens.dart';
import 'assistant_controller.dart';

/// The kinds of thing in a conversation, told apart by an icon and a label that screen
/// readers read, from this one table: her answer, a proposal, a view, a card for an app page
/// and a page she published.
enum EntryKind {
  answer(Icons.chat_bubble_outline, 'Moneypenny'),
  proposal(Icons.edit_note, 'Proposal: nothing changes until you confirm'),
  view(Icons.insights, 'Made by Moneypenny'),
  navigation(Icons.open_in_new, 'Link to a page in Outer Office'),
  pageLink(Icons.launch, 'Page she published, opens outside Outer Office');

  const EntryKind(this.icon, this.label);

  final IconData icon;

  /// What a screen reader says of an entry of this kind.
  final String label;

  /// The kind of a conversation entry by its role; `null` for the person's own message and
  /// the notes, tool names and errors, which are not hers to label.
  static EntryKind? of(AssistantEntry entry) => switch (entry.role) {
    'moneypenny' => answer,
    'proposal' => proposal,
    'view' => view,
    'navigation' => navigation,
    'page' => pageLink,
    _ => null,
  };
}

/// The icon of [kind], read as its label.
class EntryKindIcon extends StatelessWidget {
  const EntryKindIcon(this.kind, {this.size = 20, this.color, super.key});

  final EntryKind kind;
  final double size;
  final Color? color;

  @override
  Widget build(BuildContext context) => Semantics(
    label: kind.label,
    container: true,
    excludeSemantics: true,
    child: Icon(
      kind.icon,
      size: size,
      color: color ?? OoTokens.of(context).accent,
    ),
  );
}
