import 'package:material_ui/material_ui.dart';

import '../theme/oo_tokens.dart';

/// [child] kept to [width], centred, where a width is set.
Widget chatColumn(double? width, Widget child) => switch (width) {
  null => child,
  final width => Center(
    child: ConstrainedBox(
      constraints: BoxConstraints(maxWidth: width),
      child: child,
    ),
  ),
};

/// The avatar of a speaker that has no portrait: its [label] (an Agent's codename) in the
/// avatar's rounded square.
class ChatAvatar extends StatelessWidget {
  const ChatAvatar({required this.label, this.size = 32, super.key});

  final String label;
  final double size;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Semantics(
      label: label,
      excludeSemantics: true,
      child: Container(
        width: size,
        height: size,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: tokens.selectedFill,
          border: Border.all(color: tokens.accent.withValues(alpha: 0.5)),
          borderRadius: BorderRadius.circular(size / 4),
        ),
        child: Padding(
          padding: const EdgeInsets.all(3),
          child: FittedBox(
            child: Text(
              label,
              maxLines: 1,
              style: Theme.of(context).textTheme.labelMedium
                  ?.copyWith(color: tokens.accent, fontWeight: FontWeight.w700),
            ),
          ),
        ),
      ),
    );
  }
}

/// The line under a conversation that says what is wrong, with [actions] to put it right.
class ChatProblemLine extends StatelessWidget {
  const ChatProblemLine({
    required this.text,
    this.actions = const [],
    super.key,
  });

  final String text;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
    child: Row(
      children: [
        Expanded(
          child: Semantics(
            liveRegion: true,
            child: Text(
              text,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          ),
        ),
        ...actions,
      ],
    ),
  );
}
