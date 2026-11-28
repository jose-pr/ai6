import 'dart:async';

import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:material_ui/material_ui.dart';

import 'entry.dart';
import 'fold.dart';
import 'look.dart';

/// What the agent is doing now, from the newest entry of the running turn: "Starting Claude…", "Thinking…", "Running cargo test…",
/// "Editing calc.js…", "Responding…". [agent] names who is starting when [starting] is true; warm turns show "Working…".
String workingHeadline(List<ChatEntryData> entries, {String? agent, bool starting = true}) {
  final newest = entries.isEmpty ? null : entries.last;
  if (newest == null || newest.role == MessageRole.user || newest.role == MessageRole.notice) {
    if (!starting) return 'Working…';
    return agent == null ? 'Starting…' : 'Starting $agent…';
  }
  switch (newest.role) {
    case MessageRole.thought:
      return 'Thinking…';
    case MessageRole.agent:
      return 'Responding…';
    case MessageRole.tool:
      final call = newest.tool;
      if (call == null || call.status == ToolStatus.done) return 'Working…';
      String named(List<String> paths) => paths.isEmpty ? '' : ' ${_file(paths.first)}';
      return switch (call.kind) {
        'execute' => 'Running ${call.title}…',
        'edit' || 'delete' || 'move' => call.changedPaths.isEmpty ? '${call.title}…' : 'Editing${named(call.changedPaths)}…',
        'read' => call.locations.isEmpty ? '${call.title}…' : 'Reading${named(call.locations)}…',
        'search' => 'Searching…',
        'fetch' => 'Fetching…',
        _ => '${call.title}…',
      };
    default:
      return 'Working…';
  }
}

String _file(String path) {
  final cut = path.split(RegExp(r'[\/]')).where((part) => part.isNotEmpty).lastOrNull ?? path;
  return cut.replaceFirst(RegExp(r':\d+$'), '');
}

/// The last row of a conversation while the agent works: a mark that shows it is alive, what it is doing and for how long. It
/// ticks once a second by itself, so that nothing else in the conversation rebuilds.
class WorkingRow extends StatefulWidget {
  const WorkingRow({super.key, required this.headline, this.startedAt, this.clock = DateTime.now});

  final String headline;

  /// When the turn started; the time is left out without it.
  final DateTime? startedAt;
  final DateTime Function() clock;

  @override
  State<WorkingRow> createState() => _WorkingRowState();
}

class _WorkingRowState extends State<WorkingRow> {
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final started = widget.startedAt;
    final elapsed = started == null ? null : widget.clock().difference(started);
    final style = TextStyle(color: look.muted, fontSize: look.bodySize - 1);
    return Row(key: const Key('working-row'), mainAxisSize: MainAxisSize.min, children: [
      ActivityMark(color: look.accent),
      const SizedBox(width: 10),
      Flexible(child: Text(widget.headline, style: style, maxLines: 1, overflow: TextOverflow.ellipsis)),
      if (elapsed != null) ...[
        Text('  ·  ', style: style),
        Text(spanLabel(elapsed.isNegative ? Duration.zero : elapsed), key: const Key('working-elapsed'), style: style),
      ],
    ]);
  }
}

/// A small dot that pulses while the agent works, and stands still when the system asks for no animation.
class ActivityMark extends StatefulWidget {
  const ActivityMark({super.key, required this.color});

  final Color color;

  @override
  State<ActivityMark> createState() => _ActivityMarkState();
}

class _ActivityMarkState extends State<ActivityMark> with SingleTickerProviderStateMixin {
  late final AnimationController _pulse = AnimationController(vsync: this, duration: const Duration(milliseconds: 1100));

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (MediaQuery.disableAnimationsOf(context)) {
      _pulse.stop();
      _pulse.value = 1;
    } else if (!_pulse.isAnimating) {
      _pulse.repeat(reverse: true);
    }
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => FadeTransition(
    opacity: Tween<double>(begin: 0.3, end: 1).animate(_pulse),
    child: Container(width: 8, height: 8, decoration: BoxDecoration(color: widget.color, shape: BoxShape.circle)),
  );
}
