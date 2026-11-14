import '../assistant/assistant_controller.dart';
import 'attachment.dart';

/// What the attachment's frames are as the chat reads them, oldest first:
///
/// - a `prompt` is a `person` entry, its `from` in the data (`mine` for the reader's own);
/// - an `event` that names tools is a `tools` entry, so a turn's calls fold into one row;
///   its words are an `agent` entry, or a `notice` when the event only reports an outcome;
/// - a `permission` is a `permission` entry that carries its answer once one came;
/// - `attached`, `detached`, `interrupted` and `closed` are `notice` entries and `error` an
///   `error` one.
///
/// `turn`, `answered` and `ready` draw nothing of their own: the turn shows as the chat's
/// typing state, and an answer shows in its request's card.
List<AssistantEntry> attachEntries(
  Attachment attachment, {
  Set<String> me = const {},
}) {
  final entries = <AssistantEntry>[];
  for (final frame in attachment.frames) {
    AssistantEntry entry(
      String role,
      String text, [
      Map<String, Object?>? data,
    ]) => AssistantEntry(role: role, text: text, at: frame.at, data: data);
    switch (frame.kind) {
      case 'prompt':
        final from = frame.text('from');
        entries.add(
          entry('person', frame.text('text'), {
            'from': from,
            'mine': me.contains(from),
          }),
        );
      case 'event':
        final text = frame.text('text');
        final tools = [
          for (final tool in frame.data['tools'] as List<Object?>? ?? const [])
            '$tool',
        ];
        final kind = frame.text('kind');
        if (tools.isNotEmpty) {
          entries.add(entry('tools', tools.join(', ')));
        } else if (kind == 'tool') {
          if (text.isNotEmpty) entries.add(entry('tools', text));
        } else if (text.isNotEmpty) {
          final said = kind == 'text' || kind == 'plan' || kind.isEmpty;
          entries.add(entry(said ? 'agent' : 'notice', text));
        }
      case 'permission':
        final request = frame.text('request');
        final answer = attachment.answers[request];
        entries.add(
          entry('permission', frame.text('tool'), {
            'request': request,
            'detail': frame.text('detail'),
            'options': Attachment.optionsOf(frame),
            if (answer != null)
              'answered':
                  'Answered by ${answer.text('by')}: ${answer.text('option')}',
          }),
        );
      case 'attached':
        entries.add(entry('notice', '${frame.text('person')} attached'));
      case 'detached':
        entries.add(entry('notice', '${frame.text('person')} detached'));
      case 'interrupted':
        entries.add(entry('notice', 'Interrupted by ${frame.text('by')}'));
      case 'closed':
        final reason = frame.text('reason');
        entries.add(
          entry(
            'notice',
            'This Run closed${reason.isEmpty ? '' : ': $reason'}',
          ),
        );
      case 'error':
        entries.add(entry('error', 'Refused: ${frame.text('reason')}'));
    }
  }
  return entries;
}
