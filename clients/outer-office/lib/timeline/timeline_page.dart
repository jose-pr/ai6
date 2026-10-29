import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../routes/oo_filters.dart';
import '../routes/oo_route.dart';

/// One entry of an Operation's activity timeline: metadata only.
@immutable
class TimelineItemSummary {
  const TimelineItemSummary({
    required this.id,
    required this.at,
    required this.kind,
    required this.action,
    required this.operationId,
    this.stationId,
    this.missionId,
    this.sessionId,
    this.actor,
    this.target,
    this.detail = const {},
  });

  final String id;

  /// Unix seconds.
  final double at;

  /// `lifecycle`, `control`, `cable`, `sleeper` or `audit`.
  final String kind;
  final String action;
  final String operationId;
  final String? stationId;
  final String? missionId;
  final String? sessionId;
  final String? actor;
  final String? target;
  final Map<String, String> detail;

  factory TimelineItemSummary.fromJson(Map<String, dynamic> json) =>
      TimelineItemSummary(
        id: json['id'] as String,
        at: (json['at'] as num).toDouble(),
        kind: json['kind'] as String,
        action: json['action'] as String,
        operationId: json['operation_id'] as String,
        stationId: json['station_id'] as String?,
        missionId: json['mission_id'] as String?,
        sessionId: json['session_id'] as String?,
        actor: json['actor'] as String?,
        target: json['target'] as String?,
        detail: {
          for (final entry
              in (json['detail'] as Map<String, dynamic>? ?? const {}).entries)
            entry.key: entry.value as String,
        },
      );

  /// Where it happened and who acted, as one line.
  String get context => [
    if (missionId != null) 'Mission $missionId',
    if (sessionId != null) 'Session $sessionId',
    if (stationId != null) 'Station $stationId',
    if (target != null && target != missionId) target!,
    if (actor != null) 'by $actor',
  ].join(' · ');

  /// The time in UTC, to the second.
  String get when => DateTime.fromMillisecondsSinceEpoch(
    (at * 1000).round(),
    isUtc: true,
  ).toIso8601String().replaceFirst(RegExp(r'\.\d+Z$'), 'Z');
}

/// One page of a timeline, newest first.
@immutable
class TimelinePageSummary {
  const TimelinePageSummary({required this.items, this.nextCursor});

  final List<TimelineItemSummary> items;

  /// Pass back to load the next, older page; `null` on the last one.
  final String? nextCursor;

  factory TimelinePageSummary.fromJson(Map<String, dynamic> json) =>
      TimelinePageSummary(
        items: [
          for (final item in json['items'] as List<dynamic>)
            TimelineItemSummary.fromJson(item as Map<String, dynamic>),
        ],
        nextCursor: json['next_cursor'] as String?,
      );
}

IconData _kindIcon(String kind) => switch (kind) {
  'lifecycle' => Icons.timeline,
  'control' => Icons.tune,
  'cable' => Icons.mail_outline,
  'sleeper' => Icons.alarm,
  _ => Icons.fact_check_outlined,
};

/// An Operation's activity, newest first, filterable by Mission. Refreshing adds what is
/// new above what is already loaded, so older pages and the scroll position stay.
///
/// The [route] (page `timeline`) names the Operation and the Mission filter (`mission_id`).
class OperationTimelinePage extends StatefulWidget {
  const OperationTimelinePage({
    required this.board,
    required this.route,
    required this.onRoute,
    this.refreshEvery = const Duration(seconds: 10),
    super.key,
  });

  final MissionBoard board;
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  /// How often new activity is fetched while the page is shown; `null` only on request.
  final Duration? refreshEvery;

  @override
  State<OperationTimelinePage> createState() => _OperationTimelinePageState();
}

class _OperationTimelinePageState extends State<OperationTimelinePage> {
  final _missionFilter = TextEditingController();
  List<TimelineItemSummary> _items = const [];
  String? _nextCursor;
  bool _loading = false;
  String? _error;
  Timer? _timer;

  /// The route's Operation when the person may read it, `null` when not, else the first.
  String? get _operation {
    final operations = widget.board.timelineOperations;
    final named = widget.route['operation_id'];
    if (named != null) return operations.contains(named) ? named : null;
    return operations.isEmpty ? null : operations.first;
  }

  String? get _mission => widget.route['mission_id'];

  @override
  void initState() {
    super.initState();
    _missionFilter.text = _mission ?? '';
    unawaited(_reload());
    if (widget.refreshEvery case final every?) {
      _timer = Timer.periodic(every, (_) => unawaited(_refresh()));
    }
  }

  @override
  void didUpdateWidget(OperationTimelinePage oldWidget) {
    super.didUpdateWidget(oldWidget);
    final mission = _mission ?? '';
    if (_missionFilter.text.trim() != mission) _missionFilter.text = mission;
    if (oldWidget.route['operation_id'] != widget.route['operation_id'] ||
        oldWidget.route['mission_id'] != _mission) {
      unawaited(_reload());
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    _missionFilter.dispose();
    super.dispose();
  }

  Future<TimelinePageSummary?> _fetch({String? cursor}) async {
    final operation = _operation;
    if (operation == null) return null;
    try {
      final page = await widget.board.loadTimeline(
        operationId: operation,
        missionId: _mission,
        cursor: cursor,
      );
      if (mounted) setState(() => _error = null);
      return page;
    } on StateError catch (error) {
      if (mounted) setState(() => _error = error.message);
      return null;
    }
  }

  /// Starts over: another Operation or Mission filter was chosen.
  Future<void> _reload() async {
    setState(() {
      _items = const [];
      _nextCursor = null;
      _loading = true;
    });
    final asked = (_operation, _mission);
    final page = await _fetch();
    // A later choice of Operation or Mission has its own reload.
    if (!mounted || asked != (_operation, _mission)) return;
    setState(() {
      _loading = false;
      if (page != null) {
        _items = page.items;
        _nextCursor = page.nextCursor;
      }
    });
  }

  /// Adds new activity above what is loaded, keeping older pages and their cursor.
  Future<void> _refresh() async {
    final page = await _fetch();
    if (!mounted || page == null) return;
    final known = {for (final item in _items) item.id};
    final fresh = page.items.where((item) => !known.contains(item.id));
    setState(() {
      _items = [...fresh, ..._items];
      if (_items.length == page.items.length) _nextCursor = page.nextCursor;
    });
  }

  Future<void> _loadOlder() async {
    final cursor = _nextCursor;
    if (cursor == null) return;
    setState(() => _loading = true);
    final page = await _fetch(cursor: cursor);
    if (!mounted) return;
    setState(() {
      _loading = false;
      if (page != null) {
        final known = {for (final item in _items) item.id};
        _items = [
          ..._items,
          ...page.items.where((item) => !known.contains(item.id)),
        ];
        _nextCursor = page.nextCursor;
      }
    });
  }

  void _applyMission() {
    final mission = _missionFilter.text.trim();
    final next = mission.isEmpty ? null : mission;
    if (next == _mission) return;
    try {
      widget.onRoute(widget.route.withField('mission_id', next));
    } on FormatException catch (error) {
      setState(() => _error = error.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = widget.board.timelineOperations;
    if (operations.isEmpty) {
      return const Center(
        child: Text('No Operation timeline is visible to this account.'),
      );
    }
    final operation = _operation;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 16, 24, 8),
          child: Wrap(
            spacing: 16,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              DropdownButton<String>(
                value: operation,
                hint: const Text('Operation'),
                items: [
                  for (final operation in operations)
                    DropdownMenuItem(value: operation, child: Text(operation)),
                ],
                onChanged: (chosen) {
                  if (chosen == null || chosen == operation) return;
                  widget.onRoute(
                    widget.route
                        .without('mission_id')
                        .withField('operation_id', chosen),
                  );
                },
              ),
              SizedBox(
                width: 260,
                child: TextField(
                  controller: _missionFilter,
                  decoration: InputDecoration(
                    labelText: 'Filter by Mission',
                    suffixIcon: IconButton(
                      tooltip: 'Apply Mission filter',
                      icon: const Icon(Icons.filter_alt_outlined),
                      onPressed: _applyMission,
                    ),
                  ),
                  onSubmitted: (_) => _applyMission(),
                ),
              ),
              IconButton(
                tooltip: 'Refresh timeline',
                onPressed: () => unawaited(_refresh()),
                icon: const Icon(Icons.refresh),
              ),
            ],
          ),
        ),
        OoFilterBar(route: widget.route, onRoute: widget.onRoute),
        if (_error case final error?)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 24),
            child: Text(error),
          ),
        Expanded(
          child: operation == null
              ? Center(
                  child: Text(
                    'Operation ${widget.route['operation_id']}\'s timeline is '
                    'not visible to this account.',
                  ),
                )
              : _items.isEmpty
              ? Center(
                  child: _loading
                      ? const CircularProgressIndicator()
                      : Text(
                          _mission == null
                              ? 'No activity recorded yet.'
                              : 'No activity of Mission $_mission in Operation '
                                    '$operation.',
                        ),
                )
              : ListView.builder(
                  key: const PageStorageKey('operation-timeline'),
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 24),
                  itemCount: _items.length + 1,
                  itemBuilder: (context, index) {
                    if (index == _items.length) {
                      return _nextCursor == null
                          ? const SizedBox(height: 24)
                          : Center(
                              child: TextButton(
                                onPressed: _loading ? null : _loadOlder,
                                child: const Text('Load older activity'),
                              ),
                            );
                    }
                    final item = _items[index];
                    return ListTile(
                      leading: Icon(_kindIcon(item.kind)),
                      title: Text('${item.kind} · ${item.action}'),
                      subtitle: Text(
                        [
                          if (item.context.isNotEmpty) item.context,
                          for (final entry in item.detail.entries)
                            '${entry.key}: ${entry.value}',
                        ].join('\n'),
                      ),
                      trailing: Text(
                        item.when,
                        style: Theme.of(context).textTheme.labelSmall,
                      ),
                      isThreeLine: item.detail.isNotEmpty,
                    );
                  },
                ),
        ),
      ],
    );
  }
}
