import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';
import '../shell/oo_freshness.dart';

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

/// Whether [rune] renders as nothing: tag characters, direction controls and marks,
/// zero-width characters, variation selectors, fillers, invisible operators and the soft
/// hyphen, as AI6's vetting counts them.
bool isInvisible(int rune) =>
    (rune >= 0xE0000 && rune <= 0xE007F) ||
    (rune >= 0x202A && rune <= 0x202E) ||
    (rune >= 0x2066 && rune <= 0x2069) ||
    rune == 0x200E ||
    rune == 0x200F ||
    rune == 0x061C ||
    (rune >= 0x200B && rune <= 0x200D) ||
    (rune >= 0x2060 && rune <= 0x2064) ||
    rune == 0xFEFF ||
    (rune >= 0xFE00 && rune <= 0xFE0F) ||
    (rune >= 0xE0100 && rune <= 0xE01EF) ||
    rune == 0x180E ||
    rune == 0x115F ||
    rune == 0x1160 ||
    rune == 0x3164 ||
    rune == 0xFFA0 ||
    rune == 0x00AD;

/// [text] with the characters that render as nothing made visible, so an operator sees what
/// vetting found: each as `<U+XXXX>`, and a run of tag characters (U+E0000 to U+E007F) as the
/// text it spells, `<tags: ...>`.
String revealInvisible(String text) {
  final shown = StringBuffer();
  final tags = StringBuffer();
  void flushTags() {
    if (tags.isEmpty) return;
    shown.write('<tags: $tags>');
    tags.clear();
  }

  for (final rune in text.runes) {
    if (rune >= 0xE0000 && rune <= 0xE007F) {
      final code = rune - 0xE0000;
      tags.write(
        code >= 0x20 && code <= 0x7E ? String.fromCharCode(code) : '?',
      );
      continue;
    }
    flushTags();
    if (isInvisible(rune)) {
      shown.write(
        '<U+${rune.toRadixString(16).toUpperCase().padLeft(4, '0')}>',
      );
    } else {
      shown.writeCharCode(rune);
    }
  }
  flushTags();
  return shown.toString();
}

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
///
/// The [route] (page `briefings`) names the Operation and may narrow the list to the held
/// brief whose key is `item_id`.
class HeldBriefsPage extends StatefulWidget {
  const HeldBriefsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<HeldBriefsPage> createState() => _HeldBriefsPageState();
}

class _HeldBriefsPageState extends State<HeldBriefsPage> {
  Future<List<HeldBriefSummary>>? _held;
  bool _deciding = false;

  /// The route's Operation when the person may read it, `null` when not, else the first.
  String? get _operationId {
    final operations = widget.board.proposalOperations;
    final named = widget.route['operation_id'];
    if (named != null) return operations.contains(named) ? named : null;
    return operations.isEmpty ? null : operations.first;
  }

  @override
  void initState() {
    super.initState();
    _reload();
  }

  @override
  void didUpdateWidget(HeldBriefsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      _reload();
    }
  }

  void _reload() {
    final operationId = _operationId;
    setState(() {
      _held = operationId == null
          ? null
          : widget.board.loadHeldBriefs(operationId);
    });
  }

  void _select(String? operationId) {
    if (operationId == null || operationId == _operationId) return;
    widget.onRoute(
      widget.route.without('item_id').withField('operation_id', operationId),
    );
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
      _reload();
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
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operations grant Mission creation access.'),
      );
    }
    final itemId = widget.route['item_id'];
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: DropdownButtonFormField<String>(
              // The field keeps its first value; a new selection is a new field.
              key: ValueKey(operationId),
              initialValue: operationId,
              decoration: const InputDecoration(labelText: 'Operation'),
              items: [
                for (final id in operations)
                  DropdownMenuItem(value: id, child: Text(id)),
              ],
              onChanged: _select,
            ),
          ),
          OoFilterBar(
            route: widget.route,
            onRoute: widget.onRoute,
            padding: const EdgeInsets.only(top: 8),
          ),
          if (operationId == null)
            Expanded(
              child: Center(
                child: Text(
                  'Operation ${widget.route['operation_id']} does not grant '
                  'Mission creation access.',
                ),
              ),
            )
          else ...[
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
                  onPressed: _deciding ? null : _reload,
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
                  final held = [
                    for (final brief in snapshot.data!)
                      if (itemId == null || brief.key == itemId) brief,
                  ];
                  if (held.isEmpty) {
                    return Center(
                      child: Text(
                        itemId == null
                            ? 'No briefs are held.'
                            : 'No brief $itemId is held in Operation $operationId.',
                      ),
                    );
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
          Text(
            revealInvisible(held.title),
            style: Theme.of(context).textTheme.titleMedium,
          ),
          const SizedBox(height: 4),
          Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text('${revealInvisible(held.source)} · ${held.key} · '),
              OoTime(
                held.heldAt,
                prefix: 'held ',
                style: DefaultTextStyle.of(context).style,
              ),
            ],
          ),
          if (held.url != null) Text(revealInvisible(held.url!)),
          const SizedBox(height: 8),
          for (final finding in held.findings) Text('• ${finding.line}'),
          // The text may carry the very thing vetting caught: shown on request, as text.
          ExpansionTile(
            tilePadding: EdgeInsets.zero,
            title: const Text('Show the brief text'),
            children: [
              Align(
                alignment: Alignment.centerLeft,
                child: SelectableText(revealInvisible(held.body)),
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
                ? 'Queue “${revealInvisible(widget.held.title)}” again. Vetting will not hold it '
                      'again for ${kinds.join(', ')}.'
                : 'File “${revealInvisible(widget.held.title)}” as refused without running it.',
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
