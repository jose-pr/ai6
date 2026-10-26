import 'package:material_ui/material_ui.dart';

import '../green_ink/green_ink_page.dart' show utcTime;
import '../main.dart';

/// One thing vetting found in a brief.
@immutable
class VettingFinding {
  const VettingFinding({
    required this.kind,
    required this.detail,
    this.related,
  });

  /// `malformed`, `duplicate`, `rate`, `budget`, `screening` or `review`.
  final String kind;

  /// What was found, in AI6's words; never the brief's own text.
  final String detail;

  /// The Mission or brief it relates to, such as the work a duplicate repeats.
  final String? related;

  factory VettingFinding.fromJson(Map<String, dynamic> json) => VettingFinding(
    kind: json['kind'] as String,
    detail: json['detail'] as String,
    related: json['related'] as String?,
  );

  /// The finding as one line: `duplicate: the same URL as other work (mission-1)`.
  String get line => '$kind: $detail${related == null ? '' : ' ($related)'}';
}

/// The findings of a vetting, empty when it passed.
List<VettingFinding> vettingFindings(Map<String, dynamic>? vetting) => [
  for (final finding in vetting?['findings'] as List<dynamic>? ?? const [])
    VettingFinding.fromJson(finding as Map<String, dynamic>),
];

/// A brief vetting held for an operator.
@immutable
class HeldBriefSummary {
  const HeldBriefSummary({
    required this.key,
    required this.operationId,
    required this.title,
    required this.body,
    required this.source,
    required this.url,
    required this.findings,
    required this.heldAt,
  });

  /// The Registry key the release and refuse routes take.
  final String key;
  final String operationId;
  final String title;

  /// The brief's text: untrusted when it came from a Source or a proposal.
  final String body;
  final String source;
  final String? url;
  final List<VettingFinding> findings;
  final double heldAt;

  factory HeldBriefSummary.fromJson(Map<String, dynamic> json) {
    final brief = json['brief'] as Map<String, dynamic>;
    return HeldBriefSummary(
      key: json['key'] as String,
      operationId: json['operation'] as String,
      title: brief['title'] as String,
      body: brief['body'] as String? ?? '',
      source: brief['source'] as String? ?? 'manual',
      url: brief['url'] as String?,
      findings: vettingFindings(json['vetting'] as Map<String, dynamic>?),
      heldAt: (json['held_at'] as num).toDouble(),
    );
  }
}

/// Briefs vetting held: what it found in each, the brief's text only on request, and a
/// release or refusal. Needs `mission.create`.
class HeldBriefsPage extends StatefulWidget {
  const HeldBriefsPage({required this.board, super.key});

  final MissionBoard board;

  @override
  State<HeldBriefsPage> createState() => _HeldBriefsPageState();
}

class _HeldBriefsPageState extends State<HeldBriefsPage> {
  String? _operationId;
  Future<List<HeldBriefSummary>>? _held;
  bool _deciding = false;

  @override
  void initState() {
    super.initState();
    final operations = widget.board.proposalOperations;
    if (operations.isNotEmpty) _select(operations.first);
  }

  void _select(String? operationId) {
    if (operationId == null) return;
    setState(() {
      _operationId = operationId;
      _held = widget.board.loadHeldBriefs(operationId);
    });
  }

  Future<void> _decide(HeldBriefSummary held, {required bool release}) async {
    final reason = await showDialog<String?>(
      context: context,
      builder: (context) => _HeldDecisionDialog(held: held, release: release),
    );
    if (reason == null || !mounted) return;
    setState(() => _deciding = true);
    try {
      await widget.board.decideHeldBrief(
        operationId: held.operationId,
        key: held.key,
        release: release,
        reason: reason.isEmpty ? null : reason,
      );
      if (!mounted) return;
      _select(held.operationId);
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            release ? 'Brief released to the queue.' : 'Brief refused.',
          ),
        ),
      );
    } on Object catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.toString())));
      }
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = widget.board.proposalOperations;
    final operationId = _operationId;
    if (operations.isEmpty || operationId == null) {
      return const Center(
        child: Text('No Operations grant Mission creation access.'),
      );
    }
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: DropdownButtonFormField<String>(
              initialValue: operationId,
              decoration: const InputDecoration(labelText: 'Operation'),
              items: [
                for (final id in operations)
                  DropdownMenuItem(value: id, child: Text(id)),
              ],
              onChanged: _select,
            ),
          ),
          const SizedBox(height: 16),
          Row(
            children: [
              Expanded(
                child: Text(
                  'Vetting held these briefs; none runs until you release it.',
                  style: Theme.of(context).textTheme.titleMedium,
                ),
              ),
              IconButton(
                tooltip: 'Refresh held briefs',
                onPressed: _deciding ? null : () => _select(operationId),
                icon: const Icon(Icons.refresh),
              ),
            ],
          ),
          if (widget.board.demo)
            const Chip(label: Text('Preview data — decisions disabled')),
          const SizedBox(height: 8),
          Expanded(
            child: FutureBuilder<List<HeldBriefSummary>>(
              future: _held,
              builder: (context, snapshot) {
                if (snapshot.hasError) {
                  return Center(child: Text(snapshot.error.toString()));
                }
                if (!snapshot.hasData) {
                  return const Center(child: CircularProgressIndicator());
                }
                final held = snapshot.data!;
                if (held.isEmpty) {
                  return const Center(child: Text('No briefs are held.'));
                }
                return ListView.separated(
                  itemCount: held.length,
                  separatorBuilder: (context, index) =>
                      const SizedBox(height: 8),
                  itemBuilder: (context, index) =>
                      _heldCard(context, held[index]),
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _heldCard(BuildContext context, HeldBriefSummary held) => Card(
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(held.title, style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 4),
          Text('${held.source} · ${held.key} · held ${utcTime(held.heldAt)}'),
          if (held.url != null) Text(held.url!),
          const SizedBox(height: 8),
          for (final finding in held.findings) Text('• ${finding.line}'),
          // The text may carry the very thing vetting caught: shown on request, as text.
          ExpansionTile(
            tilePadding: EdgeInsets.zero,
            title: const Text('Show the brief text'),
            children: [
              Align(
                alignment: Alignment.centerLeft,
                child: SelectableText(held.body),
              ),
            ],
          ),
          Wrap(
            spacing: 8,
            children: [
              FilledButton(
                onPressed: _deciding || widget.board.demo
                    ? null
                    : () => _decide(held, release: true),
                child: const Text('Release…'),
              ),
              OutlinedButton(
                onPressed: _deciding || widget.board.demo
                    ? null
                    : () => _decide(held, release: false),
                child: const Text('Refuse…'),
              ),
            ],
          ),
        ],
      ),
    ),
  );
}

/// Asks the operator to confirm a release, or a refusal with an optional reason. Pops the
/// reason (empty for none), or nothing when cancelled.
class _HeldDecisionDialog extends StatefulWidget {
  const _HeldDecisionDialog({required this.held, required this.release});

  final HeldBriefSummary held;
  final bool release;

  @override
  State<_HeldDecisionDialog> createState() => _HeldDecisionDialogState();
}

class _HeldDecisionDialogState extends State<_HeldDecisionDialog> {
  final _reason = TextEditingController();

  @override
  void dispose() {
    _reason.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final release = widget.release;
    final kinds = {for (final finding in widget.held.findings) finding.kind};
    return AlertDialog(
      title: Text(release ? 'Release this brief?' : 'Refuse this brief?'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            release
                ? 'Queue “${widget.held.title}” again. Vetting will not hold it '
                      'again for ${kinds.join(', ')}.'
                : 'File “${widget.held.title}” as refused without running it.',
          ),
          if (!release) ...[
            const SizedBox(height: 12),
            TextField(
              controller: _reason,
              decoration: const InputDecoration(
                labelText: 'Reason, for the record (optional)',
              ),
            ),
          ],
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(_reason.text.trim()),
          child: Text(release ? 'Release' : 'Refuse'),
        ),
      ],
    );
  }
}
