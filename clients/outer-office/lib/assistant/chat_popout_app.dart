import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../settings/oo_config.dart' show ChatSettings;
import '../shell/stay_signed_in.dart' show OoActivity;
import '../theme/oo_theme.dart';
import 'chat_popout.dart';
import 'chat_layout.dart';
import 'chat_region.dart' show ChatPanel;
import 'chat_window.dart';

/// What a browser window opened by Moneypenny's Pop out runs: a second copy of the app that
/// shows only her chat, on the sign-in the browser already holds, in the opener's
/// conversation, draft and theme.
class ChatPopoutApp extends StatelessWidget {
  const ChatPopoutApp({
    required this.board,
    required this.chatSettings,
    required this.popups,
    super.key,
  });

  final MissionBoard board;
  final ChatSettings chatSettings;
  final PopupPlatform popups;

  @override
  Widget build(BuildContext context) => boardThemedApp(
    board,
    title: chatWindowTitle,
    home: AnimatedBuilder(
      animation: board,
      builder: (context, _) {
        final ready =
            board.demo ||
            (board.accessToken != null &&
                board.moneypennyServed &&
                board.userRun != null);
        if (ready) {
          return _PopoutChat(
            board: board,
            chatSettings: chatSettings,
            popups: popups,
          );
        }
        if (board.resuming || board.loading || board.accessToken != null) {
          return const Scaffold(
            body: Center(child: CircularProgressIndicator()),
          );
        }
        return const _Note(
          'Sign in on the Outer Office page, then pop Moneypenny out again.',
        );
      },
    ),
  );
}

class _Note extends StatelessWidget {
  const _Note(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Scaffold(
    body: Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Text(
          text,
          textAlign: TextAlign.center,
          style: TextStyle(color: OoTokens.of(context).text),
        ),
      ),
    ),
  );
}

class _PopoutChat extends StatefulWidget {
  const _PopoutChat({
    required this.board,
    required this.chatSettings,
    required this.popups,
  });

  final MissionBoard board;
  final ChatSettings chatSettings;
  final PopupPlatform popups;

  @override
  State<_PopoutChat> createState() => _PopoutChatState();
}

class _PopoutChatState extends State<_PopoutChat> {
  late final PopoutRun _run = PopoutRun(
    widget.popups,
    person: widget.board.userRun?.personId,
  );
  MoneypennyChat? _chat;

  @override
  void initState() {
    super.initState();
    _run.onAppearance = _look;
    _run.state.then((state) {
      if (!mounted) return;
      _look(state.appearance);
      final chat = moneypennyChatFor(
        widget.board,
        widget.chatSettings,
        conversation: Future.value(state.conversation),
        remember: false,
      );
      _run.attach(chat, draft: state.draft);
      setState(() => _chat = chat);
    });
  }

  void _look(Map<String, Object?> look) {
    final theme = look['theme'];
    final backdrop = look['backdrop'];
    if (theme is String) widget.board.restoreThemeId(theme);
    if (backdrop is bool) widget.board.restoreBackdrop(backdrop);
  }

  @override
  void dispose() {
    _chat?.dispose();
    _run.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => OoActivity(
    onActivity: () {
      widget.board.markActive();
      _run.activity();
    },
    child: _body(),
  );

  Widget _body() => ValueListenableBuilder(
    valueListenable: _run.openerClosed,
    builder: (context, gone, _) {
      if (gone) {
        return const _Note(
          'The page this window belongs to was closed. Moneypenny is back '
          'there; you can close this window.',
        );
      }
      final chat = _chat;
      if (chat == null) {
        return const Scaffold(body: Center(child: CircularProgressIndicator()));
      }
      return Scaffold(
        body: SafeArea(
          child: ChatPanel(
            chat: chat,
            placement: ChatPlacement.away,
            inWindow: true,
            onBringBack: _run.bringBack,
            onHide: _run.hide,
          ),
        ),
      );
    },
  );
}
