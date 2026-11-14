import '../assistant/chat_talk.dart';
import '../main.dart';
import '../shell/operation_names.dart';
import 'attachment.dart';
import 'charges_model.dart';
import 'mission_attach.dart' show attachNames, liveMissionStates;

/// The chat's Runs to talk to, read from [board]: each charge the person may converse with
/// (`charge.converse`), then each Mission with a live Run under `mission.control`. Charges
/// are named as the Charges page names them, Missions as the Missions board does.
class BoardTalk implements TalkSource {
  BoardTalk(this.board);

  final MissionBoard board;

  static const chargesGroup = 'Charges';
  static const missionsGroup = 'Mission Runs';

  /// [charge] as a Run to talk to.
  static TalkOption charge(ChargeSummary charge) => TalkOption(
    id: 'charge:${charge.scope}',
    label: charge.title,
    group: chargesGroup,
    target: ChargeTarget(charge.scope),
    agent: charge.agent,
    canInterrupt: charge.canInterrupt,
  );

  /// The live Run of Mission [missionId] of [operationId] as a Run to talk to.
  static TalkOption mission(
    MissionBoard board,
    String operationId,
    String missionId, {
    String? agent,
  }) => TalkOption(
    id: 'mission:$operationId/$missionId',
    label:
        '${OperationNames.of(operationId)} › ${board.handleOf(operationId, missionId)}',
    group: missionsGroup,
    target: MissionTarget(operationId, missionId),
    agent: agent,
    // The right that lets a person attach to a Mission's Run is the one that interrupts it.
    canInterrupt: true,
  );

  @override
  String? get token => board.demo ? null : board.accessToken;

  @override
  AttachChannel Function() get connect =>
      board.attachChannel ?? () => connectAttach(board.apiOrigin);

  @override
  Set<String> get me => attachNames(board.userSession);

  @override
  Future<List<TalkOption>> options() async {
    if (token == null) return const [];
    final options = <TalkOption>[];
    try {
      for (final charge in await board.loadCharges()) {
        if (charge.canConverse) options.add(BoardTalk.charge(charge));
      }
    } on Object {
      // Charges unreadable: Missions and Moneypenny remain.
    }
    for (final group in board.missionGroups()) {
      if (liveMissionStates.contains(group.state) &&
          board.hasOperationAction(group.operationId, 'mission.control')) {
        options.add(
          BoardTalk.mission(
            board,
            group.operationId,
            group.missionId,
            agent: group.latest?.agent,
          ),
        );
      }
    }
    return options;
  }
}
