import 'package:material_ui/material_ui.dart';

import '../main.dart';

/// Mission states after which a Mission has ended.
const _ended = {'done', 'failed', 'blown', 'abandoned'};

/// One operator control as offered for a Mission in its current state.
@immutable
class MissionControlOption {
  const MissionControlOption({
    required this.action,
    required this.label,
    required this.icon,
    this.unavailable,
  });

  /// The wire spelling: `cancel`, `retry`, `resume` or `interrupt`.
  final String action;
  final String label;
  final IconData icon;

  /// Why the control cannot be asked for now; `null` when it can.
  final String? unavailable;

  bool get enabled => unavailable == null;
}

/// The latest outcome of an operator control, as Universal Exports reports it.
@immutable
class ControlStatusSummary {
  const ControlStatusSummary({
    required this.controlId,
    required this.operationId,
    required this.missionId,
    required this.action,
    required this.stationId,
    required this.status,
    this.state,
    this.reason,
  });

  final String controlId;
  final String operationId;
  final String missionId;
  final String action;
  final String stationId;

  /// `queued`, `delivered`, `applied`, `rejected` or `expired`.
  final String status;

  /// The Mission state the Station reported afterwards.
  final String? state;
  final String? reason;

  bool get isOpen => status == 'queued' || status == 'delivered';

  factory ControlStatusSummary.fromJson(Map<String, dynamic> json) =>
      ControlStatusSummary(
        controlId: json['control_id'] as String,
        operationId: json['operation_id'] as String,
        missionId: json['mission_id'] as String,
        action: json['action'] as String,
        stationId: json['station_id'] as String,
        status: json['status'] as String,
        state: json['state'] as String?,
        reason: json['reason'] as String?,
      );

  /// One line for the operator: what happened to the control.
  String describe() {
    final label = _labels[action] ?? action;
    return switch (status) {
      'applied' => '$label applied · Mission is ${state ?? 'updated'}',
      'rejected' =>
        '$label rejected by $stationId: ${reason ?? 'no reason given'}',
      'expired' => '$label expired before a Station acted on it',
      _ => '$label sent to $stationId · waiting for its outcome',
    };
  }
}

const _labels = {
  'cancel': 'Cancel Mission',
  'retry': 'Retry Mission',
  'resume': 'Resume Mission',
  'interrupt': 'Interrupt Session',
};

/// The Session carrying a Mission's current state: the one whose state changed last.
SessionSummary? currentSession(List<SessionSummary> sessions) {
  SessionSummary? current;
  for (final session in sessions) {
    if (current == null ||
        (session.updatedAt ?? 0) > (current.updatedAt ?? 0)) {
      current = session;
    }
  }
  return current;
}

/// Every control, enabled only where the Mission's state allows it; the Station that holds
/// the Mission makes the same checks and has the final say.
List<MissionControlOption> missionControls({
  required String? state,
  required bool permitted,
}) {
  const denied = 'Your account lacks mission.control in this Operation.';
  final current = state ?? 'working';
  String? when(bool allowed, String reason) =>
      !permitted ? denied : (allowed ? null : reason);
  return [
    MissionControlOption(
      action: 'cancel',
      label: _labels['cancel']!,
      icon: Icons.cancel_outlined,
      unavailable: when(
        !_ended.contains(current),
        'The Mission already ended ($current).',
      ),
    ),
    MissionControlOption(
      action: 'retry',
      label: _labels['retry']!,
      icon: Icons.replay,
      unavailable: when(
        current == 'stalled' || _ended.contains(current),
        switch (current) {
          'queued' => 'The Mission is already queued.',
          'quarantined' =>
            'A quarantined Mission waits for an operator to release its Agent.',
          _ => 'Only a stalled or finished Mission can be retried; cancel it first.',
        },
      ),
    ),
    MissionControlOption(
      action: 'resume',
      label: _labels['resume']!,
      icon: Icons.play_arrow_outlined,
      unavailable: when(
        const {'parked', 'blocked', 'stalled'}.contains(current),
        'Only a parked, blocked or stalled Mission can resume.',
      ),
    ),
    MissionControlOption(
      action: 'interrupt',
      label: _labels['interrupt']!,
      icon: Icons.pause_circle_outline,
      unavailable: when(
        current == 'working',
        'Only a working Session can be interrupted.',
      ),
    ),
  ];
}

/// The Mission card's control menu. Closing a Session view never sends a control; only a
/// choice made here does.
class MissionActionsMenu extends StatelessWidget {
  const MissionActionsMenu({
    required this.board,
    required this.operationId,
    required this.missionId,
    required this.sessions,
    super.key,
  });

  final MissionBoard board;
  final String operationId;
  final String missionId;
  final List<SessionSummary> sessions;

  @override
  Widget build(BuildContext context) {
    final current = currentSession(sessions);
    final options = missionControls(
      state: current?.state,
      permitted: board.canControlMission(operationId),
    );
    return PopupMenuButton<String>(
      tooltip: 'Mission actions',
      icon: const Icon(Icons.more_vert),
      itemBuilder: (context) => [
        for (final option in options)
          PopupMenuItem<String>(
            value: option.action,
            enabled: option.enabled,
            child: ListTile(
              dense: true,
              enabled: option.enabled,
              leading: Icon(option.icon),
              title: Text(option.label),
              subtitle: option.unavailable == null
                  ? null
                  : Text(option.unavailable!),
            ),
          ),
      ],
      onSelected: (action) => _run(context, action, current),
    );
  }

  Future<void> _run(
    BuildContext context,
    String action,
    SessionSummary? current,
  ) async {
    final request = await showDialog<_ControlRequest>(
      context: context,
      builder: (_) => _ControlDialog(action: action, missionId: missionId),
    );
    if (request == null || !context.mounted) return;
    final messenger = ScaffoldMessenger.maybeOf(context);
    try {
      final status = await board.requestControl(
        operationId: operationId,
        missionId: missionId,
        action: action,
        agent: request.agent,
        message: request.message,
        sessionId: action == 'interrupt' ? current?.sessionId : null,
      );
      messenger?.showSnackBar(SnackBar(content: Text(status.describe())));
    } on StateError catch (error) {
      messenger?.showSnackBar(SnackBar(content: Text(error.message)));
    }
  }
}

@immutable
class _ControlRequest {
  const _ControlRequest({this.agent, this.message});

  final String? agent;
  final String? message;
}

class _ControlDialog extends StatefulWidget {
  const _ControlDialog({required this.action, required this.missionId});

  final String action;
  final String missionId;

  @override
  State<_ControlDialog> createState() => _ControlDialogState();
}

class _ControlDialogState extends State<_ControlDialog> {
  final _field = TextEditingController();

  @override
  void dispose() {
    _field.dispose();
    super.dispose();
  }

  String? get _value {
    final value = _field.text.trim();
    return value.isEmpty ? null : value;
  }

  @override
  Widget build(BuildContext context) {
    final (title, body, confirm) = switch (widget.action) {
      'cancel' => (
        'Cancel Mission ${widget.missionId}?',
        'Its live Session stops and the Mission is abandoned. Its safe house keeps the work.',
        'Cancel Mission',
      ),
      'retry' => (
        'Retry Mission ${widget.missionId}?',
        'The Mission is queued again as a new attempt that continues in the same safe house.',
        'Retry',
      ),
      'resume' => (
        'Resume Mission ${widget.missionId}?',
        'The Mission resumes now on its provider Session.',
        'Resume',
      ),
      _ => (
        'Interrupt the Session?',
        'The current turn stops. The Mission waits, blocked, until it is resumed.',
        'Interrupt',
      ),
    };
    final field = switch (widget.action) {
      'retry' => 'Agent codename (optional)',
      'resume' => 'Message to the Agent (optional)',
      _ => null,
    };
    return AlertDialog(
      title: Text(title),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(body),
          if (field != null) ...[
            const SizedBox(height: 16),
            TextField(
              controller: _field,
              decoration: InputDecoration(labelText: field),
              maxLines: widget.action == 'resume' ? 4 : 1,
            ),
          ],
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Back'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(
            _ControlRequest(
              agent: widget.action == 'retry' ? _value : null,
              message: widget.action == 'resume' ? _value : null,
            ),
          ),
          child: Text(confirm),
        ),
      ],
    );
  }
}
