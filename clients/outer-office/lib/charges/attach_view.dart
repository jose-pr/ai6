import 'package:material_ui/material_ui.dart';

import '../theme/oo_tokens.dart';
import 'attachment.dart';

/// A live view of a Run a person is attached to: its history, then what it shows as it shows
/// it, with a box to type to it, the answers to what it asks, and, when [canInterrupt], a way
/// to interrupt it. Leaving the view detaches; the Run goes on.
///
/// Whoever shows this holds the right to attach (`charge.converse` on a charge's scope,
/// `mission.control` on a Mission's Operation); a person without it is never given the view.
class AttachView extends StatefulWidget {
  const AttachView({
    required this.token,
    required this.target,
    required this.connect,
    required this.canInterrupt,
    super.key,
  });

  final String token;
  final AttachTarget target;

  /// Opens the socket; tests give a fake one.
  final AttachChannel Function() connect;

  /// Whether the person holds the action that interrupts a turn here.
  final bool canInterrupt;

  @override
  State<AttachView> createState() => _AttachViewState();
}

class _AttachViewState extends State<AttachView> {
  late final Attachment _attachment = Attachment(
    token: widget.token,
    target: widget.target,
    connect: widget.connect,
  )..connect();
  final _prompt = TextEditingController();
  final _scroll = ScrollController();

  @override
  void dispose() {
    _attachment.dispose();
    _prompt.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _send() {
    final text = _prompt.text;
    if (text.trim().isEmpty) return;
    _attachment.prompt(text);
    _prompt.clear();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _attachment,
    builder: (context, _) {
      final attachment = _attachment;
      final tokens = OoTokens.of(context);
      final text = Theme.of(context).textTheme;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    _status(attachment),
                    key: const ValueKey('attach-status'),
                    style: text.labelLarge?.copyWith(color: tokens.muted),
                  ),
                ),
                if (attachment.phase == AttachPhase.dropped)
                  TextButton.icon(
                    key: const ValueKey('attach-reconnect'),
                    onPressed: attachment.reconnect,
                    icon: const Icon(Icons.refresh, size: 18),
                    label: const Text('Reconnect'),
                  ),
                if (widget.canInterrupt)
                  TextButton.icon(
                    key: const ValueKey('attach-interrupt'),
                    onPressed: attachment.attached
                        ? attachment.interrupt
                        : null,
                    icon: const Icon(Icons.stop_circle_outlined, size: 18),
                    label: const Text('Interrupt'),
                  ),
              ],
            ),
          ),
          if (attachment.refusal case final refusal?)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Text(
                refusal,
                key: const ValueKey('attach-refusal'),
                style: text.bodyMedium?.copyWith(color: tokens.danger),
              ),
            ),
          Expanded(
            child: ListView.builder(
              controller: _scroll,
              itemCount: attachment.frames.length,
              itemBuilder: (context, index) =>
                  _FrameRow(attachment.frames[index], attachment),
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: TextField(
                  key: const ValueKey('attach-prompt'),
                  controller: _prompt,
                  enabled: attachment.attached,
                  minLines: 1,
                  maxLines: 4,
                  decoration: const InputDecoration(
                    labelText: 'Say something to this Run',
                    border: OutlineInputBorder(),
                  ),
                  onSubmitted: (_) => _send(),
                ),
              ),
              const SizedBox(width: 8),
              FilledButton(
                key: const ValueKey('attach-send'),
                onPressed: attachment.attached ? _send : null,
                child: const Text('Send'),
              ),
            ],
          ),
        ],
      );
    },
  );

  String _status(Attachment attachment) => switch (attachment.phase) {
    AttachPhase.connecting => 'Connecting to ${widget.target.label}',
    AttachPhase.replaying => 'Replaying what ${widget.target.label} showed',
    AttachPhase.live => attachment.busy ? 'Taking a turn' : 'Idle',
    AttachPhase.ended => 'This Run has closed',
    AttachPhase.dropped => 'Disconnected',
    AttachPhase.refused => 'Not attached',
    AttachPhase.detached => 'Detached',
  };
}

class _FrameRow extends StatelessWidget {
  const _FrameRow(this.frame, this.attachment);

  final AttachFrame frame;
  final Attachment attachment;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final muted = text.bodySmall?.copyWith(color: tokens.muted);
    Widget line(String words, {TextStyle? style}) => Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: SelectableText(words, style: style ?? muted),
    );
    switch (frame.kind) {
      case 'prompt':
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: 4),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('From ${frame.text('from')}', style: muted),
              SelectableText(frame.text('text'), style: text.bodyMedium),
            ],
          ),
        );
      case 'event':
        final tools = [
          for (final tool in frame.data['tools'] as List<Object?>? ?? const [])
            '$tool',
        ];
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: 4),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(_eventKind(frame.text('kind')), style: muted),
              SelectableText(frame.text('text'), style: text.bodyMedium),
              if (tools.isNotEmpty)
                Text('Tools: ${tools.join(', ')}', style: muted),
            ],
          ),
        );
      case 'turn':
        return line(
          frame.data['state'] == 'begun'
              ? 'Turn begun by ${frame.text('by')}'
              : 'Turn ended by ${frame.text('by')}',
        );
      case 'permission':
        return _Permission(frame, attachment);
      case 'answered':
        final reason = frame.text('reason');
        return line(
          '${frame.text('by')} answered request ${frame.text('request')}: '
          '${frame.text('option')}${reason.isEmpty ? '' : ' ($reason)'}',
        );
      case 'attached':
        return line('${frame.text('person')} attached');
      case 'detached':
        return line('${frame.text('person')} detached');
      case 'interrupted':
        return line('Interrupted by ${frame.text('by')}');
      case 'ready':
        return line('Replay complete');
      case 'closed':
        final reason = frame.text('reason');
        return line('This Run closed${reason.isEmpty ? '' : ': $reason'}');
      case 'error':
        return line(
          'Refused: ${frame.text('reason')}',
          style: text.bodySmall?.copyWith(color: tokens.danger),
        );
    }
    return const SizedBox.shrink();
  }

  static String _eventKind(String kind) => switch (kind) {
    'text' => 'Said',
    'tool' => 'Tool',
    'plan' => 'Plan',
    'completed' => 'Completed',
    '' => 'Event',
    _ => kind,
  };
}

/// An agent's request for permission: its options, until one is answered; then who answered.
class _Permission extends StatelessWidget {
  const _Permission(this.frame, this.attachment);

  final AttachFrame frame;
  final Attachment attachment;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final request = frame.text('request');
    final answer = attachment.answers[request];
    final detail = frame.text('detail');
    final enabled = answer == null && attachment.attached;
    return Card(
      margin: const EdgeInsets.symmetric(vertical: 4),
      child: Padding(
        padding: const EdgeInsets.all(10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Asks permission: ${frame.text('tool')}',
              style: text.titleSmall,
            ),
            if (detail.isNotEmpty)
              SelectableText(
                detail,
                style: text.bodySmall?.copyWith(color: tokens.muted),
              ),
            const SizedBox(height: 6),
            Wrap(
              spacing: 8,
              runSpacing: 4,
              children: [
                for (final option in Attachment.optionsOf(frame))
                  OutlinedButton(
                    key: ValueKey('permission-$request-${option.id}'),
                    onPressed: enabled
                        ? () => attachment.answer(request, option.id)
                        : null,
                    child: Text(option.label),
                  ),
              ],
            ),
            if (answer != null)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  'Answered by ${answer.text('by')}: ${answer.text('option')}',
                  key: ValueKey('permission-$request-answered'),
                  style: text.bodySmall?.copyWith(color: tokens.muted),
                ),
              ),
          ],
        ),
      ),
    );
  }
}
