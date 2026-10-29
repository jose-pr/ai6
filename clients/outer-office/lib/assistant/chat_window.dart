import 'package:material_ui/material_ui.dart';

import '../shell/app_window.dart';
import 'chat_layout.dart';
import 'chat_region.dart';

/// The title of Moneypenny's own window.
const chatWindowTitle = 'Moneypenny | Outer Office';

/// Opens Moneypenny's own native window for [chat] above [context], the same conversation
/// as the main window. [app] wraps the window's content in the app's themed
/// `MaterialApp`; [onClosed] runs when the window goes.
AppWindow openChatWindow(
  BuildContext context, {
  required MoneypennyChat chat,
  required VoidCallback onClosed,
  required Widget Function(Widget home) app,
}) => openAppWindow(
  context,
  size: Size(chat.settings.windowWidth, chat.settings.windowHeight),
  title: chatWindowTitle,
  onClosed: onClosed,
  builder: (_) => app(ChatWindowView(chat: chat)),
);

/// The content of her own window: the chat with Bring back and Hide. Her shortcuts work
/// here too: the main window's [ChatShell] reads every key the app receives.
class ChatWindowView extends StatelessWidget {
  const ChatWindowView({required this.chat, super.key});

  final MoneypennyChat chat;

  @override
  Widget build(BuildContext context) => Scaffold(
    body: SafeArea(
      child: ChatPanel(
        chat: chat,
        placement: ChatPlacement.away,
        inWindow: true,
      ),
    ),
  );
}
