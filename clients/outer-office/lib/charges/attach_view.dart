import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../assistant/assistant_controller.dart';
import '../assistant/chat_transcript.dart';
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
      return Column(
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
              entries: attachEntries(attachment, me: widget.me),
              busy: attachment.busy,
              textWidth: widget.textWidth,
              entryBuilder: (context, entry) =>
                  _entry(label, attachment, entry),
            ),
          ),
          if (attachment.busy) const LinearProgressIndicator(),
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
          ChatComposerBar(
            message: _message,
            focusNode: _composer,
            enabled: attachment.attached,
            busy: attachment.busy,
            hint: 'Say something to this Run',
            onSend: _send,
            onStop: widget.canInterrupt ? attachment.interrupt : null,
            stopTooltip: 'Interrupt',
            textWidth: widget.textWidth,
          ),
        ],
      );
    },
  );

  Widget _entry(String agent, Attachment attachment, AssistantEntry entry) =>
      switch (entry.role) {
        'person' => ChatPersonBubble(
          text: entry.text,
          sender: entry.data?['mine'] == true
              ? 'You'
              : '${entry.data?['from'] ?? ''}',
        ),
        'agent' => ChatAgentBubble(
          avatar: ChatAvatar(label: agent),
          text: entry.text,
        ),
        'permission' => ChatPermissionCard(
          request: '${entry.data?['request']}',
          title: 'Asks permission: ${entry.text}',
          detail: entry.data?['detail'] as String?,
          options: [
            for (final option
                in entry.data?['options'] as List<PermissionOption>? ??
                    const <PermissionOption>[])
              (id: option.id, label: option.label),
          ],
          answeredBy: entry.data?['answered'] as String?,
          onAnswer: attachment.attached
              ? (option) =>
                    attachment.answer('${entry.data?['request']}', option)
              : null,
        ),
        'notice' => ChatNotice(text: entry.text),
        _ => ChatErrorLine(entry.text),
      };

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
