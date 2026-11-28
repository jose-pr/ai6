import 'package:coqui_acp_chat_ui/coqui_acp_chat_ui.dart';
import 'package:material_ui/material_ui.dart';

import '../theme/oo_tokens.dart';

/// Outer Office's chat look: the theme's tokens as the shared chat's, so each of the six themes
/// draws its conversation in its own colours.
///
/// [width] is the widest a message grows (null: as wide as the chat is); [centred] puts that
/// column in the middle of a wider space, as the chat does when it takes the page.
ChatLook ooChatLook(OoTokens t, {double? width, bool centred = false}) =>
    ChatLook(
      bg: t.bg,
      surface: t.surface,
      raised: t.raised,
      text: t.text,
      muted: t.muted,
      accent: t.accent,
      border: t.border,
      success: t.success,
      warning: t.warning,
      danger: t.danger,
      user: t.selectedFill,
      agent: t.raised,
      notice: t.surface,
      composer: t.raised,
      code: t.bg,
      messageWidth: width ?? double.infinity,
      gap: 10,
      padding: 10,
      radius: 14,
      bodySize: 14,
      agentInBubble: true,
      centred: centred,
      bubbleBorder: t.border,
      density: ChatDensity.thinking,
    );

/// The shared chat widgets inside the app: the app's `material_ui` theme with the [look] they
/// draw with added to its extensions, and the app's text style.
class OoChatTheme extends StatelessWidget {
  const OoChatTheme({required this.look, required this.child, super.key});

  final ChatLook look;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Theme(
      data: theme.copyWith(
        extensions: [
          for (final extension in theme.extensions.values)
            if (extension is! ChatLook) extension,
          look,
        ],
      ),
      child: Material(
        type: MaterialType.transparency,
        child: DefaultTextStyle(
          style: theme.textTheme.bodyMedium ?? const TextStyle(),
          child: child,
        ),
      ),
    );
  }
}
