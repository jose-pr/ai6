import 'package:material_ui/material_ui.dart';

import '../assistant/chat_talk.dart';
import '../main.dart';
import 'board_talk.dart';

/// The Mission states in which a Run may be live to attach to.
const liveMissionStates = {'working', 'blocked', 'stalled'};

/// The names the Run's frames know [session]'s person by.
Set<String> attachNames(UserSessionSummary? session) => {
  if (session != null) ...[
    session.personId,
    session.name,
    ?session.displayName,
  ],
};

/// A Mission's live Run, for a person with `mission.control` on its Operation: a row that
/// opens the chat panel talking to it. The panel attaches only once asked, and talking to
/// Moneypenny again detaches; the Run goes on.
class MissionAttachSection extends StatelessWidget {
  const MissionAttachSection({
    required this.board,
    required this.operationId,
    required this.missionId,
    required this.state,
    this.agent,
    super.key,
  });

  final MissionBoard board;
  final String operationId;
  final String missionId;
  final String state;

  /// The codename of the Agent on the Mission's Run, which labels its messages.
  final String? agent;

  /// Whether the section is shown: the person holds `mission.control` and a Run may be live.
  bool get shown =>
      !board.demo &&
      board.accessToken != null &&
      liveMissionStates.contains(state) &&
      board.hasOperationAction(operationId, 'mission.control');

  @override
  Widget build(BuildContext context) {
    if (!shown) return const SizedBox.shrink();
    final open = RunTalkScope.maybeOf(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: ListTile(
        key: ValueKey(('attach', operationId, missionId)),
        contentPadding: EdgeInsets.zero,
        title: const Text('Live Run'),
        subtitle: const Text(
          'Talk to it in the chat: read it, type to it and answer it',
        ),
        trailing: FilledButton.icon(
          key: const ValueKey('attach-open-chat'),
          onPressed: open == null
              ? null
              : () => open(
                  BoardTalk.mission(
                    board,
                    operationId,
                    missionId,
                    agent: agent,
                  ),
                ),
          icon: const Icon(Icons.forum_outlined, size: 18),
          label: const Text('Open in chat'),
        ),
      ),
    );
  }
}
