import 'package:material_ui/material_ui.dart';

import '../main.dart';
import 'attach_session.dart';
import 'attach_view.dart';

/// The Mission states in which a Run may be live to attach to.
const _liveStates = {'working', 'blocked', 'stalled'};

/// A Mission's live Run to attach to, for a person with `mission.control` on its Operation.
/// It attaches only once opened, and leaving or closing it detaches; the Run goes on.
class MissionAttachSection extends StatelessWidget {
  const MissionAttachSection({
    required this.board,
    required this.operationId,
    required this.missionId,
    required this.state,
    this.connect,
    super.key,
  });

  final MissionBoard board;
  final String operationId;
  final String missionId;
  final String state;

  /// Opens the attach socket; tests give a fake one. The gateway's by default.
  final AttachChannel Function()? connect;

  /// Whether the section is shown: the person holds `mission.control` and a Run may be live.
  bool get shown =>
      !board.demo &&
      board.accessToken != null &&
      _liveStates.contains(state) &&
      board.hasOperationAction(operationId, 'mission.control');

  @override
  Widget build(BuildContext context) {
    final token = board.accessToken;
    if (!shown || token == null) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: ExpansionTile(
        key: ValueKey(('attach', operationId, missionId)),
        title: const Text('Live Run'),
        subtitle: const Text('Attach to read it, type to it and answer it'),
        childrenPadding: const EdgeInsets.symmetric(horizontal: 8),
        children: [
          SizedBox(
            height: MediaQuery.sizeOf(context).height / 2,
            child: AttachView(
              token: token,
              target: MissionTarget(operationId, missionId),
              connect: connect ?? () => connectAttach(board.apiOrigin),
              canInterrupt: true,
            ),
          ),
        ],
      ),
    );
  }
}
