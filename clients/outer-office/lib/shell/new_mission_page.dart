import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../assistant/mission_draft.dart' show DraftPlacement, PlacementLine;
import '../main.dart';
import '../routes/oo_route.dart';
import '../settings/oo_config.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'oo_empty.dart';
import 'oo_shell.dart';
import 'operation_names.dart';

/// A page to start a Mission without a conversation: pick the Operation, write the title and
/// the instructions, see where it would run, and queue it. Queueing is the call the app has
/// always made for a Mission, and opens the Mission on the board.
///
/// The route's `operation_id` is picked first when the person may start a Mission there.
/// Without `mission.create` anywhere the page says so and offers nothing.
class NewMissionPage extends StatefulWidget {
  const NewMissionPage({
    required this.board,
    required this.route,
    required this.onRoute,
    this.onDraft,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  /// Asks Moneypenny to draft the Mission instead; none where she is not served.
  final ValueChanged<String>? onDraft;

  @override
  State<NewMissionPage> createState() => _NewMissionPageState();
}

class _NewMissionPageState extends State<NewMissionPage> {
  final _title = TextEditingController();
  final _prompt = TextEditingController();
  String? _picked;
  Future<DraftPlacement?>? _placement;
  String? _placementFor;
  bool _sending = false;
  String? _problem;

  /// The Operations the person may start a Mission in.
  List<String> get _operations => widget.board.demo
      ? const []
      : [
          for (final operation
              in widget.board.userSession?.operations ??
                  const <UserOperationAccess>[])
            if (widget.board.canCreateMission(operation.operationId))
              operation.operationId,
        ];

  String? get _operation {
    final operations = _operations;
    final picked = _picked;
    if (picked != null && operations.contains(picked)) return picked;
    final asked = widget.route['operation_id'];
    if (asked != null && operations.contains(asked)) return asked;
    return operations.firstOrNull;
  }

  @override
  void dispose() {
    _title.dispose();
    _prompt.dispose();
    super.dispose();
  }

  Future<DraftPlacement?>? _placementOf(String operation) {
    if (_placementFor != operation) {
      _placementFor = operation;
      _placement = widget.board.placement(operation);
    }
    return _placement;
  }

  bool get _ready =>
      !_sending &&
      _title.text.trim().isNotEmpty &&
      _prompt.text.trim().isNotEmpty;

  Future<void> _queue(String operation) async {
    if (!_ready) return;
    setState(() {
      _sending = true;
      _problem = null;
    });
    try {
      final id = await widget.board.createMission(
        operationId: operation,
        title: _title.text.trim(),
        prompt: _prompt.text.trim(),
      );
      if (!mounted) return;
      widget.onRoute(
        const OoRoute(OoPage.missions)
            .withField('operation_id', operation)
            .withField('mission_id', id),
      );
    } on Object {
      if (!mounted) return;
      setState(() {
        _sending = false;
        _problem = 'Could not queue the Mission. Check access and try again.';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final operation = _operation;
    if (operation == null) {
      return const OoEmptyState(
        icon: OoIconName.newMission,
        title: 'You cannot start a Mission',
        body:
            'Your account cannot start one; a controller grants mission.create on an '
            'Operation.',
      );
    }
    final inputs = widget.board.limits.inputs;
    final theme = Theme.of(context);
    final onDraft = widget.onDraft;
    return ListenableBuilder(
      listenable: widget.board,
      builder: (context, _) => SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(24, 8, 24, 36),
        child: Align(
          alignment: Alignment.topLeft,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 640),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              spacing: 12,
              children: [
                DropdownMenu<String>(
                  key: const ValueKey('new-mission-operation'),
                  label: const Text('Operation'),
                  initialSelection: operation,
                  requestFocusOnTap: false,
                  expandedInsets: EdgeInsets.zero,
                  dropdownMenuEntries: [
                    for (final id in _operations)
                      DropdownMenuEntry(
                        value: id,
                        label: OperationNames.of(id),
                      ),
                  ],
                  onSelected: _sending
                      ? null
                      : (id) => setState(() => _picked = id),
                ),
                TextField(
                  key: const ValueKey('new-mission-title'),
                  controller: _title,
                  enabled: !_sending,
                  maxLength: InputSettings.field(inputs.missionTitleChars),
                  onChanged: (_) => setState(() {}),
                  decoration: const InputDecoration(labelText: 'Mission title'),
                ),
                TextField(
                  key: const ValueKey('new-mission-prompt'),
                  controller: _prompt,
                  enabled: !_sending,
                  minLines: 5,
                  maxLines: 10,
                  maxLength: InputSettings.field(inputs.missionPromptChars),
                  onChanged: (_) => setState(() {}),
                  decoration: const InputDecoration(
                    labelText: 'Instructions for the agent',
                    alignLabelWithHint: true,
                  ),
                ),
                PlacementLine(_placementOf(operation)),
                if (_problem case final problem?)
                  Text(
                    problem,
                    key: const ValueKey('new-mission-problem'),
                    style: TextStyle(color: OoTokens.of(context).danger),
                  ),
                Text(
                  'Nothing runs until you queue it.',
                  style: theme.textTheme.bodySmall,
                ),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    FilledButton.icon(
                      key: const ValueKey('new-mission-queue'),
                      onPressed: _ready
                          ? () => unawaited(_queue(operation))
                          : null,
                      icon: _sending
                          ? const SizedBox.square(
                              dimension: 16,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : const Icon(Icons.send),
                      label: const Text('Queue Mission'),
                    ),
                    if (onDraft != null)
                      OutlinedButton(
                        key: const ValueKey('new-mission-draft'),
                        onPressed: _sending ? null : () => onDraft(operation),
                        child: const Text('Draft with Moneypenny'),
                      ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
