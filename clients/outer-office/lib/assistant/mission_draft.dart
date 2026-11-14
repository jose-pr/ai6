import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../shell/operation_names.dart';
import '../theme/oo_tokens.dart';
import 'assistant_panel.dart' show proposalStatus;
import 'entry_kind.dart';

/// Where a Mission of an Operation would run now, in words: the best place, or why none.
class DraftPlacement {
  const DraftPlacement({this.best, this.reasons = const []});

  /// `Station · Theater · Agent`, the first choice; `null` when nowhere can take it.
  final String? best;

  /// Why places were ruled out, one line each.
  final List<String> reasons;
}

/// How a Mission stands on the board.
class MissionStanding {
  const MissionStanding(this.state, {this.reason});

  /// `queued`, `working`, `verifying`, `held`, `done`, `failed`, ...
  final String state;
  final String? reason;

  /// Whether its Runs have nothing more to do without a person.
  bool get settled =>
      const {'done', 'failed', 'held', 'verifying'}.contains(state);
}

/// What a Mission draft card reads of the board: where it would run, and how it stands once
/// queued. It notifies when the board hears of a change.
abstract interface class MissionDraftReads implements Listenable {
  /// Where a Mission of [operationId] would run; `null` when the person may not preview it.
  Future<DraftPlacement?> placement(String operationId);

  /// The Mission's state on the board; `null` before the board has heard of it.
  MissionStanding? standing(String operationId, String missionId);

  /// The Mission's final message, once it has one.
  Future<String?> finalReport(String operationId, String missionId);
}

/// A Mission Moneypenny drafted: the brief to edit, where it would run, and the same Confirm
/// and Cancel as any proposal; once queued, its state and result and a link that reopens it.
///
/// Editing asks her to draft again, so what is confirmed is always a proposal she recorded.
/// Confirm answers once: it is disabled after the first press, and a settled card offers none.
class MissionDraftCard extends StatefulWidget {
  const MissionDraftCard({
    required this.proposal,
    required this.reads,
    required this.onConfirm,
    required this.onCancel,
    required this.onRevise,
    this.onOpen,
    super.key,
  });

  final Map<String, Object?> proposal;
  final MissionDraftReads reads;
  final void Function(String id) onConfirm;
  final void Function(String id) onCancel;

  /// Withdraws the proposal and asks for another with this text.
  final void Function(String id, String text) onRevise;
  final void Function(String link)? onOpen;

  @override
  State<MissionDraftCard> createState() => _MissionDraftCardState();
}

class _MissionDraftCardState extends State<MissionDraftCard> {
  late final TextEditingController _title;
  late final TextEditingController _brief;
  bool _pressed = false;
  Future<DraftPlacement?>? _placement;
  Future<String?>? _report;
  String? _reportFor;

  Map<String, Object?> get _body {
    final request = widget.proposal['request'];
    final body = request is Map ? request['body'] : null;
    return body is Map ? body.cast<String, Object?>() : const {};
  }

  String get _operation {
    final needs = widget.proposal['needs'];
    final on = needs is Map ? needs['on'] as String? : null;
    return on?.replaceFirst('operation:', '') ?? '';
  }

  String get _status => widget.proposal['status'] as String? ?? 'pending';

  @override
  void initState() {
    super.initState();
    _title = TextEditingController(text: _body['title'] as String? ?? '');
    _brief = TextEditingController(text: _body['prompt'] as String? ?? '');
    if (_status == 'pending' && _operation.isNotEmpty) {
      _placement = widget.reads.placement(_operation);
    }
  }

  @override
  void dispose() {
    _title.dispose();
    _brief.dispose();
    super.dispose();
  }

  bool get _edited =>
      _title.text.trim() != (_body['title'] as String? ?? '') ||
      _brief.text.trim() != (_body['prompt'] as String? ?? '');

  void _revise() {
    final id = widget.proposal['id'] as String? ?? '';
    widget.onRevise(
      id,
      'Draft the Mission again with this title and brief.\n'
      'Title: ${_title.text.trim()}\nBrief: ${_brief.text.trim()}',
    );
  }

  Widget _placementLine(BuildContext context) => FutureBuilder<DraftPlacement?>(
    future: _placement,
    builder: (context, snapshot) {
      final muted = TextStyle(color: OoTokens.of(context).muted);
      final placement = snapshot.data;
      if (snapshot.connectionState != ConnectionState.done) {
        return Text('Checking where it would run…', style: muted);
      }
      if (placement == null) return const SizedBox.shrink();
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            placement.best == null
                ? 'Nowhere can take it now.'
                : 'Would run on ${placement.best}.',
            key: const ValueKey('draft-placement'),
          ),
          for (final reason in placement.reasons)
            Text(
              reason,
              style: muted.merge(Theme.of(context).textTheme.bodySmall),
            ),
        ],
      );
    },
  );

  /// The receipt: the Mission the confirmed request queued, how it stands, and its result.
  Widget _receipt(BuildContext context, Map<String, Object?> receipt) {
    final operation = receipt['operation_id'] as String? ?? _operation;
    final mission = receipt['mission_id'] as String? ?? '';
    final standing = widget.reads.standing(operation, mission);
    final word = standing?.state ?? 'queued';
    if (standing != null && standing.settled && _reportFor != word) {
      _reportFor = word;
      // A result not written yet is read again when the board next hears of a change.
      _report = widget.reads.finalReport(operation, mission).then((text) {
        if (text == null || text.isEmpty) _reportFor = null;
        return text;
      });
    }
    final open = widget.onOpen;
    final link = widget.proposal['opens'] as String?;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      spacing: 6,
      children: [
        Row(
          spacing: 8,
          children: [
            Chip(
              key: const ValueKey('draft-state'),
              label: Text('Mission $word'),
              visualDensity: VisualDensity.compact,
            ),
            if (standing?.reason case final reason?)
              Flexible(child: Text(reason)),
          ],
        ),
        if (_report != null)
          FutureBuilder<String?>(
            future: _report,
            builder: (context, snapshot) => switch (snapshot.data) {
              final text? when text.isNotEmpty => Text(
                text,
                key: const ValueKey('draft-result'),
                maxLines: 6,
                overflow: TextOverflow.ellipsis,
              ),
              _ => const SizedBox.shrink(),
            },
          ),
        if (open != null && link != null)
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: FilledButton.tonalIcon(
              key: const ValueKey('draft-open'),
              icon: const Icon(Icons.open_in_new, size: 16),
              label: const Text('Open Mission'),
              onPressed: () => open(link),
            ),
          ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.reads,
    builder: (context, _) {
      final theme = Theme.of(context);
      final proposal = widget.proposal;
      final id = proposal['id'] as String? ?? '';
      final status = _status;
      final shown = proposalStatus(status);
      final result = proposal['result'] as String?;
      final receipt = proposal['receipt'];
      return Card(
        margin: EdgeInsets.zero,
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            spacing: 8,
            children: [
              Row(
                spacing: 8,
                children: [
                  const EntryKindIcon(EntryKind.proposal),
                  Expanded(
                    child: Text(
                      'Mission draft · ${OperationNames.of(_operation)}',
                      style: theme.textTheme.labelLarge,
                    ),
                  ),
                  Chip(
                    key: const ValueKey('proposal-status'),
                    avatar: Icon(shown.icon, size: 16),
                    label: Text(shown.word),
                    visualDensity: VisualDensity.compact,
                  ),
                ],
              ),
              if (status == 'pending') ...[
                TextField(
                  key: const ValueKey('draft-title'),
                  controller: _title,
                  onChanged: (_) => setState(() {}),
                  decoration: const InputDecoration(
                    labelText: 'Title',
                    isDense: true,
                  ),
                ),
                TextField(
                  key: const ValueKey('draft-brief'),
                  controller: _brief,
                  minLines: 3,
                  maxLines: 8,
                  onChanged: (_) => setState(() {}),
                  decoration: const InputDecoration(
                    labelText: 'Brief',
                    alignLabelWithHint: true,
                    isDense: true,
                  ),
                ),
                _placementLine(context),
                Text(
                  _edited
                      ? 'Your edits are not part of the proposal: ask Moneypenny to draft it again with them.'
                      : 'Nothing changes until you confirm.',
                  style: theme.textTheme.bodySmall,
                ),
                Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  spacing: 8,
                  children: [
                    TextButton(
                      onPressed: () => widget.onCancel(id),
                      child: const Text('Cancel'),
                    ),
                    if (_edited)
                      OutlinedButton(
                        key: const ValueKey('draft-revise'),
                        onPressed:
                            _title.text.trim().isEmpty ||
                                _brief.text.trim().isEmpty
                            ? null
                            : _revise,
                        child: const Text('Draft again with my edits'),
                      )
                    else
                      FilledButton(
                        key: const ValueKey('draft-confirm'),
                        onPressed: _pressed
                            ? null
                            : () {
                                // Set at once: a second press in the same frame finds it taken.
                                if (_pressed) return;
                                _pressed = true;
                                setState(() {});
                                widget.onConfirm(id);
                              },
                        child: const Text('Confirm'),
                      ),
                  ],
                ),
              ] else ...[
                Text(_title.text, style: theme.textTheme.titleSmall),
                if (status == 'done' && receipt is Map)
                  _receipt(context, receipt.cast<String, Object?>())
                else if (result != null)
                  Text(result),
              ],
            ],
          ),
        ),
      );
    },
  );
}
