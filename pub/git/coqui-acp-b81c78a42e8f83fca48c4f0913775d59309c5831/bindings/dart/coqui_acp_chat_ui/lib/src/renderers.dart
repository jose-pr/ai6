import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:flutter/widgets.dart';

import 'entry.dart';
import 'fold.dart';

/// Draws an entry the host knows how to draw, or returns null to leave it to the next renderer and then to the package's own.
typedef EntryRenderer = Widget? Function(BuildContext context, ChatEntryData entry);

/// Draws a tool call whose `kind` the renderer was registered under, in place of the package's card.
typedef ToolRenderer = Widget Function(BuildContext context, ToolCall tool);

/// Draws one piece of a tool call's content, or returns null to leave it to the next renderer and then to the package's own.
typedef BlockRenderer = Widget? Function(BuildContext context, ToolCall tool, ToolBlock block);

/// Draws the line a burst of finished tool calls folds into. [cards] is what opens beneath it: each call as the package, or a
/// [ChatRenderers.tool] renderer, draws it.
typedef BurstRenderer = Widget Function(BuildContext context, BurstSummary summary, List<ToolCall> tools, Widget cards);

/// Draws the line the work of a finished turn folds into. [items] is what opens beneath it.
typedef WorkRenderer = Widget Function(BuildContext context, Duration duration, BurstSummary summary, Widget items);

/// Draws the agent's plan.
typedef PlanRenderer = Widget Function(BuildContext context, List<PlanEntry> plan);

/// What a host adds to how a chat draws, without forking it: its own content kinds (a chart, a diff, a page, a delegation card), a
/// card for a kind of tool, a piece of a tool's content, and its own line for a folded burst, turn or plan. Every field has a
/// default in the package, and [merge] puts one registry over another.
@immutable
final class ChatRenderers {
  const ChatRenderers({this.entry = const [], this.tool = const {}, this.block = const [], this.burst, this.work, this.plan, this.wide});

  /// Tried in order for every entry before the package draws it.
  final List<EntryRenderer> entry;

  /// By a tool call's `kind` (`edit`, `execute`, ...), or the kind an agent invented.
  final Map<String, ToolRenderer> tool;

  /// Tried in order for each piece of a tool call's content.
  final List<BlockRenderer> block;
  final BurstRenderer? burst;
  final WorkRenderer? work;
  final PlanRenderer? plan;

  /// Whether an entry takes the whole width rather than the column of [ChatLook.messageWidth]: a table or a chart.
  final bool Function(ChatEntryData entry)? wide;

  static const ChatRenderers none = ChatRenderers();

  /// This registry, with [other] taking precedence: its entry and block renderers are tried first, its tool renderers replace
  /// the same kinds, and its burst, work and plan renderers replace these.
  ChatRenderers merge(ChatRenderers other) => ChatRenderers(
    entry: [...other.entry, ...entry],
    tool: {...tool, ...other.tool},
    block: [...other.block, ...block],
    burst: other.burst ?? burst,
    work: other.work ?? work,
    plan: other.plan ?? plan,
    wide: other.wide ?? wide,
  );

  Widget? drawEntry(BuildContext context, ChatEntryData entry) {
    for (final renderer in this.entry) {
      if (renderer(context, entry) case final drawn?) return drawn;
    }
    return null;
  }

  Widget? drawBlock(BuildContext context, ToolCall tool, ToolBlock block) {
    for (final renderer in this.block) {
      if (renderer(context, tool, block) case final drawn?) return drawn;
    }
    return null;
  }
}

/// What the widgets of a conversation read from above them: the host's renderers and the commands' output, which a tool's card
/// shows by its terminal id.
class ChatScope extends InheritedWidget {
  const ChatScope({super.key, this.renderers = ChatRenderers.none, this.terminals = const {}, required super.child});

  final ChatRenderers renderers;
  final Map<String, TerminalView> terminals;

  static ChatScope? maybeOf(BuildContext context) => context.dependOnInheritedWidgetOfExactType<ChatScope>();

  static ChatRenderers renderersOf(BuildContext context) => maybeOf(context)?.renderers ?? ChatRenderers.none;

  @override
  bool updateShouldNotify(ChatScope oldWidget) => true;
}
