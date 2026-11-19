import 'dart:convert';

import 'package:coqui_acp/coqui_acp.dart' show AcpSessionUpdate;

import 'content.dart' show attachedNames;
import 'limits.dart';

/// What a `session/update` becomes in a chat. [unshown] is the fallback: nothing in the conversation changes for it, and a host's
/// test can fail when a recorded kind reaches it (see [UpdateRecorder.unshown]).
enum Rendering {
  /// A user's or the agent's message.
  message,

  /// The agent's thinking.
  thought,

  /// A tool call: its title, status, content and places.
  toolCard,

  /// A command's output, shown with the tool call that ran it.
  terminal,

  /// The agent's plan.
  plan,

  /// The slash commands the agent offers.
  commands,

  /// What the session has used.
  usage,

  /// The agent's options and its current mode.
  choices,

  /// The conversation's name.
  title,

  /// Working or idle: when a turn starts and ends.
  status,

  /// Something the agent tells the person outside the conversation's history, as a notice in it.
  notice,

  /// The agent compacting its context: one entry per compaction.
  compaction,

  unshown,
}

/// How [update] is taken. An agent's chunk whose content carries no text, such as an image, is [Rendering.unshown]. A v2 message
/// or thought without content is a patch to one already shown, and keeps its rendering.
Rendering renderingOf(AcpSessionUpdate update, ChatLimits limits) =>
    switch (update.kind) {
      'user_message' || 'agent_message' => Rendering.message,
      'agent_thought' => Rendering.thought,
      'user_message_chunk' =>
        update.text == null &&
                attachedNames(update.raw['content'], limits).isEmpty
            ? Rendering.unshown
            : Rendering.message,
      'agent_message_chunk' =>
        update.text == null ? Rendering.unshown : Rendering.message,
      'agent_thought_chunk' =>
        update.text == null ? Rendering.unshown : Rendering.thought,
      'tool_call' ||
      'tool_call_update' ||
      'tool_call_content_chunk' => Rendering.toolCard,
      'terminal_update' || 'terminal_output_chunk' => Rendering.terminal,
      'plan' || 'plan_update' => Rendering.plan,
      'notice' => Rendering.notice,
      'compaction_update' || 'compaction_summary_chunk' => Rendering.compaction,
      'available_commands_update' => Rendering.commands,
      'usage_update' => Rendering.usage,
      'config_option_update' || 'current_mode_update' => Rendering.choices,
      'session_info_update' => Rendering.title,
      'state_update' => Rendering.status,
      _ => Rendering.unshown,
    };

/// Keys whose string values are the protocol's own words, such as a tool's `kind`, and so are kept in a shape.
const _protocolKeys = {
  'sessionUpdate',
  'type',
  'status',
  'kind',
  'state',
  'stopReason',
  'priority',
};
final _word = RegExp(r'^[a-z][a-z0-9_]{0,39}$');
final _key = RegExp(r'^[A-Za-z_][A-Za-z0-9_./-]{0,63}$');

/// [value] with every string replaced by `"string"`, every number and boolean by its type, and each list by its distinct element
/// shapes (at most [ChatLimits.listShapes]). Only a protocol word under one of a few protocol keys survives, so what an agent or a
/// person wrote never does.
Object? shapeOf(
  Object? value, {
  String? key,
  int depth = 0,
  ChatLimits limits = const ChatLimits(),
}) {
  if (depth > limits.shapeDepth) return '…';
  return switch (value) {
    final Map<dynamic, dynamic> map => {
      for (final entry in map.entries)
        _key.hasMatch('${entry.key}') ? '${entry.key}' : 'key': shapeOf(
          entry.value,
          key: '${entry.key}',
          depth: depth + 1,
          limits: limits,
        ),
    },
    final List<dynamic> list => () {
      final shapes = <String, Object?>{};
      for (final item in list) {
        if (shapes.length >= limits.listShapes) break;
        final shape = shapeOf(item, key: key, depth: depth + 1, limits: limits);
        shapes.putIfAbsent(jsonEncode(shape), () => shape);
      }
      return shapes.values.toList();
    }(),
    final String text
        when _protocolKeys.contains(key) && _word.hasMatch(text) =>
      text,
    String() => 'string',
    bool() => 'bool',
    int() => 'int',
    double() => 'double',
    null => 'null',
    _ => 'other',
  };
}

/// One distinct shape of update a session received, and how often.
final class SeenUpdate {
  SeenUpdate(this.kind, this.rendering, this.shape);
  final String kind;
  final Rendering rendering;
  final Object? shape;
  int count = 0;

  Map<String, Object?> toJson() => {
    'kind': kind,
    'rendering': rendering.name,
    'shape': shape,
  };
}

/// Records the kind and shape of every update a chat applies, never its text. Each distinct one goes to [write] once, as a JSON
/// line, the first time it arrives.
final class UpdateRecorder {
  UpdateRecorder({this.write});

  final void Function(String line)? write;
  final Map<String, SeenUpdate> _seen = {};

  List<SeenUpdate> get seen => List.unmodifiable(_seen.values);

  /// The kinds that reached the fallback, which nothing in the conversation shows.
  Set<String> get unshown => {
    for (final update in _seen.values)
      if (update.rendering == Rendering.unshown) update.kind,
  };

  void record(
    AcpSessionUpdate update,
    Rendering rendering, [
    ChatLimits limits = const ChatLimits(),
  ]) {
    final kind = _word.hasMatch(update.kind) ? update.kind : 'other';
    final seen = SeenUpdate(
      kind,
      rendering,
      shapeOf(update.raw, limits: limits),
    );
    final line = jsonEncode(seen.toJson());
    final known = _seen.putIfAbsent(line, () {
      write?.call(line);
      return seen;
    });
    known.count++;
  }
}
