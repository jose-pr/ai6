// The shared chat widgets draw with Flutter's own Material; the app's theme is material_ui's.
// ignore: deprecated_member_use
import 'package:flutter/material.dart' as flutter;
import 'package:coqui_acp_chat_ui/coqui_acp_chat_ui.dart';
import 'package:material_ui/material_ui.dart';

import '../forms/form_host.dart' show flutterThemeOf;
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

/// The shared chat widgets inside the app: Flutter's Material theme made from the app's tokens
/// (the app uses `material_ui`, whose `Theme` and `Material` are other widgets), the
/// [look] they draw with, and the app's text style. The app must carry
/// `flutterMaterialLocalizations` ([OoMaterialApp] does).
class OoChatTheme extends StatelessWidget {
  const OoChatTheme({required this.look, required this.child, super.key});

  final ChatLook look;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final text = Theme.of(context).textTheme;
    return flutter.Theme(
      data: flutterThemeOf(context).copyWith(
        textTheme: flutter.TextTheme(
          displayLarge: text.displayLarge,
          displayMedium: text.displayMedium,
          displaySmall: text.displaySmall,
          headlineLarge: text.headlineLarge,
          headlineMedium: text.headlineMedium,
          headlineSmall: text.headlineSmall,
          titleLarge: text.titleLarge,
          titleMedium: text.titleMedium,
          titleSmall: text.titleSmall,
          bodyLarge: text.bodyLarge,
          bodyMedium: text.bodyMedium,
          bodySmall: text.bodySmall,
          labelLarge: text.labelLarge,
          labelMedium: text.labelMedium,
          labelSmall: text.labelSmall,
        ),
        extensions: [look],
      ),
      child: flutter.Material(
        type: flutter.MaterialType.transparency,
        child: DefaultTextStyle(
          style: text.bodyMedium ?? const TextStyle(),
          child: child,
        ),
      ),
    );
  }
}
