import 'package:coqui_acp/coqui_acp.dart' show AcpPermissionOption;
import 'package:coqui_acp_chat_ui/coqui_acp_chat_ui.dart'
    show
        ChatEntryData,
        ChatLookOf,
        ChatRenderers,
        ChatTranscript,
        Composer,
        MessageBox,
        PermissionCard;
import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../assistant/assistant_controller.dart' show AssistantEntry;
import '../assistant/chat_entries.dart';
import '../assistant/chat_look.dart';
import '../assistant/chat_parts.dart';
import '../held/held_briefs_page.dart' show revealInvisible;
import '../assistant/turns.dart' show SourcesChecked;
import '../shell/oo_markdown.dart';
import '../theme/oo_tokens.dart';
import 'attach_entries.dart';
import 'attachment.dart';

/// A live view of a Run a person is attached to, drawn as Moneypenny's chat is: its history,
/// then what it shows as it shows it, with a box to type to it, the answers to what it
/// asks and, when [canInterrupt], Interrupt where the chat has Stop. Leaving the view
/// detaches; the Run goes on. Enter sends; Shift+Enter starts a new line.
///
/// Whoever shows this holds the right to attach (`charge.converse` on a charge's scope,
/// `mission.control` on a Mission's Operation); a person without it is never given the view.
class AttachView extends StatefulWidget {
  const AttachView({
    required this.token,
    required this.target,
    required this.connect,
    required this.canInterrupt,
    this.agent,
    this.me = const {},
    this.textWidth,
    super.key,
  });

  final String token;
  final AttachTarget target;

  /// Opens the socket; tests give a fake one.
  final AttachChannel Function() connect;

  /// Whether the person holds the action that interrupts a turn here.
  final bool canInterrupt;

  /// The codename of the Agent on the Run, which labels its messages; "Agent" without one.
  final String? agent;

  /// The names the Run knows this person by, so their own prompts read as theirs.
  final Set<String> me;

  /// Where set, messages and the box keep to a centred column this wide.
  final double? textWidth;

  @override
  State<AttachView> createState() => _AttachViewState();
}

class _AttachViewState extends State<AttachView> {
  late final Attachment _attachment = Attachment(
    token: widget.token,
    target: widget.target,
    connect: widget.connect,
  )..connect();
  final _message = TextEditingController();
  final _composer = FocusNode();

  @override
  void initState() {
    super.initState();
    _composer.onKeyEvent = _key;
  }

  @override
  void dispose() {
    _composer.onKeyEvent = null;
    _composer.dispose();
    _attachment.dispose();
    _message.dispose();
    super.dispose();
  }

  void _send() {
    final text = _message.text;
    if (text.trim().isEmpty) return;
    _attachment.prompt(text);
    _message.clear();
  }

  /// Enter sends; with Shift it is left to the field, which starts a new line.
  KeyEventResult _key(FocusNode _, KeyEvent event) {
    if (event is! KeyDownEvent ||
        (event.logicalKey != LogicalKeyboardKey.enter &&
            event.logicalKey != LogicalKeyboardKey.numpadEnter) ||
        HardwareKeyboard.instance.isShiftPressed) {
      return KeyEventResult.ignored;
    }
    _send();
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _attachment,
    builder: (context, _) {
      final attachment = _attachment;
      final tokens = OoTokens.of(context);
      final label = widget.agent ?? 'Agent';
      final dropped = attachment.phase == AttachPhase.dropped;
      final problem =
          attachment.refusal ??
          (dropped ? 'The connection to this Run was lost.' : null);
      return OoChatTheme(
        look: ooChatLook(
          tokens,
          width: widget.textWidth,
          centred: widget.textWidth != null,
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
              child: Text(
                _status(attachment),
                key: const ValueKey('attach-status'),
                style: Theme.of(context).textTheme.bodySmall
                    ?.copyWith(color: tokens.muted),
              ),
            ),
            const Divider(height: 1),
            Expanded(
              child: ChatTranscript(
                entries: ooChatEntries(
                  attachEntries(attachment, me: widget.me),
                  busy: attachment.busy,
                ),
                working: attachment.busy,
                agentName: label,
                narrow: true,
                renderers: ChatRenderers(
                  entry: [
                    (context, entry) =>
                        _entry(context, label, attachment, entry),
                  ],
                ),
              ),
            ),
            if (problem != null)
              ChatProblemLine(
                text: problem,
                actions: [
                  if (dropped)
                    TextButton(
                      key: const ValueKey('attach-reconnect'),
                      onPressed: attachment.reconnect,
                      child: const Text('Reconnect'),
                    ),
                ],
              ),
            chatColumn(
              widget.textWidth,
              Padding(
                padding: const EdgeInsets.all(12),
                child: Composer(
                  controller: _message,
                  focusNode: _composer,
                  enabled: attachment.attached,
                  // Interrupt is the only thing to do while a turn runs, where the person holds
                  // it; without it the box stays a box.
                  working: attachment.busy && widget.canInterrupt,
                  canQueue: false,
                  onStop: widget.canInterrupt ? attachment.interrupt : null,
                  onSend: _send,
                  hint: 'Say something to this Run',
                ),
              ),
            ),
          ],
        ),
      );
    },
  );

  /// The Run's own kinds of entry; null leaves the person's own words and a plain notice to the
  /// package.
  Widget? _entry(
    BuildContext context,
    String agent,
    Attachment attachment,
    ChatEntryData data,
  ) {
    if (data is OoSources) return SourcesChecked(item: data.item);
    if (data is! OoEntry) return null;
    final entry = data.entry;
    final look = context.chatLook;
    return switch (entry.role) {
      'person' => MessageBox(
        color: look.user,
        copy: data.text,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.end,
          mainAxisSize: MainAxisSize.min,
          spacing: 8,
          children: [
            Text(
              entry.data?['mine'] == true
                  ? 'You'
                  : revealInvisible('${entry.data?['from'] ?? ''}'),
              style: TextStyle(color: look.muted, fontSize: look.bodySize - 3),
            ),
            Text(data.text),
          ],
        ),
      ),
      'agent' => Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: 8,
        children: [
          ChatAvatar(label: agent),
          Expanded(
            child: MessageBox(
              color: look.agent,
              copy: entry.text,
              child: OoMarkdown(entry.text, webLinks: false, selectable: false),
            ),
          ),
        ],
      ),
      'permission' => _permission(context, agent, attachment, entry, data),
      'notice' => null,
      _ => Text(data.text, style: TextStyle(color: look.danger)),
    };
  }

  /// A request to be answered: its card while it waits and the Run can be answered, the answer
  /// once there is one.
  Widget _permission(
    BuildContext context,
    String agent,
    Attachment attachment,
    AssistantEntry entry,
    OoEntry data,
  ) {
    final look = context.chatLook;
    final request = '${entry.data?['request']}';
    final options =
        entry.data?['options'] as List<PermissionOption>? ??
        const <PermissionOption>[];
    final answered = entry.data?['answered'] as String?;
    final detail = entry.data?['detail'] as String?;
    if (answered == null && attachment.attached) {
      return KeyedSubtree(
        key: ValueKey('permission-$request'),
        child: PermissionCard(
          agent: agent,
          title: data.text,
          detail: detail == null ? null : revealInvisible(detail),
          options: [
            for (final option in options)
              AcpPermissionOption(
                optionId: option.id,
                name: option.label,
                kind: _kindOf(option),
              ),
          ],
          onChoose: (option) => attachment.answer(request, option.optionId),
          host: 'Outer Office',
        ),
      );
    }
    return MessageBox(
      color: look.notice,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: 4,
        children: [
          Text('Asks permission: ${data.text}'),
          if (detail != null && detail.isNotEmpty)
            Text(
              revealInvisible(detail),
              style: TextStyle(color: look.muted, fontSize: look.bodySize - 2),
            ),
          if (answered != null)
            Text(
              answered,
              key: ValueKey('permission-$request-answered'),
              style: TextStyle(color: look.muted, fontSize: look.bodySize - 2),
            ),
        ],
      ),
    );
  }

  /// The kind ACP gives an answer, from what the Run called it.
  static String _kindOf(PermissionOption option) {
    final words = '${option.id} ${option.label}'.toLowerCase();
    if (words.contains('reject') ||
        words.contains('deny') ||
        words.contains('refuse')) {
      return words.contains('always') ? 'reject_always' : 'reject_once';
    }
    return words.contains('always') ? 'allow_always' : 'allow_once';
  }

  String _status(Attachment attachment) => switch (attachment.phase) {
    AttachPhase.connecting => 'Connecting to ${widget.target.label}',
    AttachPhase.replaying => 'Replaying what ${widget.target.label} showed',
    AttachPhase.live => attachment.busy ? 'Answering…' : 'Idle',
    AttachPhase.ended => 'This Run has closed',
    AttachPhase.dropped => 'Disconnected',
    AttachPhase.refused => 'Not attached',
    AttachPhase.detached => 'Detached',
  };
}
