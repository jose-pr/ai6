import 'package:material_ui/material_ui.dart';

import '../main.dart';

/// Renames a Mission: gives it another handle. Its UID, its route and everything kept by it
/// stay; the old handle still names it. Without `mission.control` the action shows disabled
/// with what it needs.
class RenameMissionButton extends StatelessWidget {
  const RenameMissionButton({
    required this.board,
    required this.operationId,
    required this.missionId,
    super.key,
  });

  final MissionBoard board;
  final String operationId;

  /// The Mission's UID.
  final String missionId;

  @override
  Widget build(BuildContext context) {
    final allowed = board.canControlMission(operationId) && !board.demo;
    final button = TextButton.icon(
      onPressed: allowed
          ? () => showDialog<String>(
              context: context,
              builder: (_) => RenameMissionDialog(
                board: board,
                operationId: operationId,
                missionId: missionId,
              ),
            )
          : null,
      icon: const Icon(Icons.drive_file_rename_outline, size: 18),
      label: const Text('Rename'),
    );
    return allowed
        ? button
        : Tooltip(
            message: board.demo
                ? 'Renaming is unavailable in preview'
                : 'Needs mission.control in Operation $operationId',
            child: button,
          );
  }
}

/// Asks for the new handle and sends it; a refusal shows in Universal Exports' own words.
class RenameMissionDialog extends StatefulWidget {
  const RenameMissionDialog({
    required this.board,
    required this.operationId,
    required this.missionId,
    super.key,
  });

  final MissionBoard board;
  final String operationId;
  final String missionId;

  @override
  State<RenameMissionDialog> createState() => _RenameMissionDialogState();
}

class _RenameMissionDialogState extends State<RenameMissionDialog> {
  late final TextEditingController _handle = TextEditingController(
    text: widget.board.handleOf(widget.operationId, widget.missionId),
  );
  String? _refused;
  bool _sending = false;

  @override
  void dispose() {
    _handle.dispose();
    super.dispose();
  }

  Future<void> _send() async {
    setState(() {
      _sending = true;
      _refused = null;
    });
    try {
      final handle = await widget.board.renameMission(
        operationId: widget.operationId,
        missionId: widget.missionId,
        handle: _handle.text.trim(),
      );
      if (mounted) Navigator.of(context).pop(handle);
    } on GatewayRefused catch (refusal) {
      if (mounted) setState(() => _refused = refusal.message);
    } on Object {
      if (mounted) {
        setState(() => _refused = 'Universal Exports could not be reached.');
      }
    } finally {
      if (mounted) setState(() => _sending = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(
      'Rename Mission ${widget.board.handleOf(widget.operationId, widget.missionId)}',
    ),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        TextField(
          controller: _handle,
          autofocus: true,
          enabled: !_sending,
          decoration: InputDecoration(
            labelText: 'Handle',
            helperText: 'The old handle keeps naming this Mission.',
            errorText: _refused,
            errorMaxLines: 4,
          ),
          onSubmitted: (_) => _send(),
        ),
      ],
    ),
    actions: [
      TextButton(
        onPressed: _sending ? null : () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: _sending ? null : _send,
        child: const Text('Rename'),
      ),
    ],
  );
}
