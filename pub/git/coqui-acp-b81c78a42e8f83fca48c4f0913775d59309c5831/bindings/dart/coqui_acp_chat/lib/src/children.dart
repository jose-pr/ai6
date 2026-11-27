import 'package:coqui_acp/coqui_acp.dart';

import 'limits.dart';
import 'text.dart';

/// The `_meta` key under which an agent marks an update as a child agent's (a subagent's), or reports the child itself:
/// `{id, parentToolCallId}`, and on a report also `title`, `task`, `state`, `result`, `model` and `usage`, each only when known.
const String childMetaKey = 'dev.boriken.coqui.acp/child';

/// Where a child agent stands, as its agent reports it.
enum ChildState {
  starting,
  working,
  completed,
  failed,
  cancelled;

  /// The state a report names; null for a name this client does not know.
  static ChildState? named(Object? name) => switch (name) {
    'starting' => starting,
    'working' => working,
    'completed' => completed,
    'failed' => failed,
    'cancelled' => cancelled,
    _ => null,
  };

  /// Whether nothing changes the child after this.
  bool get ended => this == completed || this == failed || this == cancelled;
}

/// What a child has spent, as far as its agent says; null where it did not.
final class ChildUsage {
  const ChildUsage({this.totalTokens, this.toolUses, this.durationMs});

  final int? totalTokens;
  final int? toolUses;
  final int? durationMs;
}

/// A subagent an agent started, built from the updates that name it. Two children in one turn are two of these, each with its own
/// state; a child is never inferred from its parent's tool call alone.
final class ChatChild {
  ChatChild(this.id, this.order);

  /// The id the agent gave it, unique in the conversation.
  final String id;

  /// Where it came in the conversation's list: the order it was first named in.
  final int order;

  /// The tool call of the parent that started it.
  String? parentToolCallId;
  String? title;

  /// What it was asked to do.
  String? task;

  /// Where it stands; null while the agent has said nothing of it.
  ChildState? state;

  /// What it handed back.
  String? result;
  String? model;
  ChildUsage? usage;

  final List<AcpSessionUpdate> _updates = [];

  /// The child's own messages and tool calls, in the order they came, as the updates the agent sent. Empty when the agent does
  /// not report them.
  List<AcpSessionUpdate> get updates => List.unmodifiable(_updates);

  /// Whether the agent reports this child's progress as it happens. False means the child shows its task and result only.
  bool get reportsProgress => _updates.isNotEmpty;
}

/// The marker of [raw], when it is a child's.
Map<String, dynamic>? childMarkOf(Map<String, dynamic> raw) {
  final meta = raw['_meta'];
  if (meta is! Map) return null;
  final mark = meta[childMetaKey];
  if (mark is! Map || mark['id'] is! String) return null;
  return mark.cast<String, dynamic>();
}

/// Whether [raw] reports the child itself: the change to the parent's tool call that carries only the marker. A child's own tool
/// call has a call id of its own.
bool isChildReport(Map<String, dynamic> raw, Map<String, dynamic> mark) =>
    raw['sessionUpdate'] == 'tool_call_update' &&
    raw['toolCallId'] == (mark['parentToolCallId'] ?? mark['id']);

/// The children of a conversation, in the order they were named.
final class ChildRegistry {
  ChildRegistry(this.limits);

  final ChatLimits limits;
  final Map<String, ChatChild> _byId = {};
  int _named = 0;

  List<ChatChild> get all => _byId.values.toList(growable: false);

  ChatChild? operator [](String id) => _byId[id];

  ChatChild? _child(Map<String, dynamic> mark) {
    final id = idText(mark['id'], limits);
    if (id == null) return null;
    var child = _byId[id];
    if (child == null) {
      if (_byId.length >= limits.children) {
        // The oldest ended child goes first; with none ended, the oldest.
        final gone = _byId.values.firstWhere(
          (child) => child.state?.ended ?? false,
          orElse: () => _byId.values.first,
        );
        _byId.remove(gone.id);
      }
      child = ChatChild(id, _named++);
      _byId[id] = child;
    }
    child.parentToolCallId ??= idText(mark['parentToolCallId'], limits);
    return child;
  }

  /// Applies the marked [update]: a report changes the child, anything else is the child's own and joins its updates.
  void apply(AcpSessionUpdate update, Map<String, dynamic> mark) {
    final child = _child(mark);
    if (child == null) return;
    if (!isChildReport(update.raw, mark)) {
      if (child._updates.length >= limits.childUpdates) {
        child._updates.removeAt(0);
      }
      child._updates.add(update);
      return;
    }
    final title = shortText(mark['title'], limits);
    if (title != null) child.title = title;
    final task = mark['task'];
    if (task is String) child.task = _cut(task, limits.toolText);
    final state = ChildState.named(mark['state']);
    if (state != null) child.state = state;
    final result = mark['result'];
    if (result is String) child.result = _cut(result, limits.toolText);
    final model = shortText(mark['model'], limits);
    if (model != null) child.model = model;
    final usage = mark['usage'];
    if (usage is Map) {
      int? count(String key) => usage[key] is int ? usage[key] as int : null;
      child.usage = ChildUsage(
        totalTokens: count('totalTokens'),
        toolUses: count('toolUses'),
        durationMs: count('durationMs'),
      );
    }
  }
}

String _cut(String text, int max) =>
    text.length <= max ? text : '${text.substring(0, max)}…';
