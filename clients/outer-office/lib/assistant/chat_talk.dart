import 'package:material_ui/material_ui.dart';

import '../charges/attachment.dart';
import '../theme/oo_tokens.dart';

/// A Run the chat may talk to instead of Moneypenny: a charge's, or a Mission's live Run.
@immutable
class TalkOption {
  const TalkOption({
    required this.id,
    required this.label,
    required this.group,
    required this.target,
    this.agent,
    this.canInterrupt = false,
  });

  /// Tells it from the others in a list: `charge:operation:G`, `mission:G/m1`.
  final String id;

  /// What the person calls it, as the Charges page and the Missions board do.
  final String label;

  /// The heading it is listed under.
  final String group;

  final AttachTarget target;

  /// The codename of the Agent on the Run, which labels its messages.
  final String? agent;

  /// Whether the person holds the action that interrupts a turn on it.
  final bool canInterrupt;

  @override
  bool operator ==(Object other) => other is TalkOption && other.id == id;

  @override
  int get hashCode => id.hashCode;
}

/// What the chat reads to offer Runs and to attach to one. An option is offered only for a
/// Run the person holds the right to attach to.
abstract interface class TalkSource {
  /// The Runs the person may talk to, grouped by [TalkOption.group] in order.
  Future<List<TalkOption>> options();

  /// The person's access token; null while there is none, when nothing can be attached.
  String? get token;

  /// Opens the attach socket.
  AttachChannel Function() get connect;

  /// The names the Run knows this person by.
  Set<String> get me;
}

/// Lets a page open the chat on a Run: the Charges page's Live tab and a Mission's Live Run.
/// Without a scope above it (no chat) [maybeOf] is null and a page offers nothing.
class RunTalkScope extends InheritedWidget {
  const RunTalkScope({required this.open, required super.child, super.key});

  /// Shows the chat talking to [option].
  final void Function(TalkOption option) open;

  static void Function(TalkOption option)? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<RunTalkScope>()?.open;

  @override
  bool updateShouldNotify(RunTalkScope old) => old.open != open;
}

const _moneypenny = 'moneypenny';

/// "Talking to": who the chat's messages go to. Moneypenny first, then each Run [source]
/// offers under its group. It shows nothing while the person has no Run to choose and is
/// talking to Moneypenny. [onPick] gets the Run chosen, or null for Moneypenny.
class TalkingToPicker extends StatefulWidget {
  const TalkingToPicker({
    required this.source,
    required this.talking,
    required this.onPick,
    super.key,
  });

  final TalkSource source;
  final TalkOption? talking;
  final ValueChanged<TalkOption?> onPick;

  @override
  State<TalkingToPicker> createState() => _TalkingToPickerState();
}

class _TalkingToPickerState extends State<TalkingToPicker> {
  List<TalkOption> _options = const [];
  bool _disposed = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final options = await widget.source.options();
      if (!_disposed) setState(() => _options = options);
    } on Object {
      // No list: the person can still talk to Moneypenny.
    }
  }

  @override
  Widget build(BuildContext context) {
    final talking = widget.talking;
    if (_options.isEmpty && talking == null) return const SizedBox.shrink();
    final tokens = OoTokens.of(context);
    final text = Theme.of(context).textTheme;
    final groups = <String>[];
    for (final option in _options) {
      if (!groups.contains(option.group)) groups.add(option.group);
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 6, 12, 0),
      child: Row(
        spacing: 8,
        children: [
          Text(
            'Talking to',
            style: text.labelMedium?.copyWith(color: tokens.muted),
          ),
          Flexible(
            child: PopupMenuButton<String>(
              key: const ValueKey('talking-to'),
              tooltip: 'Choose who to talk to',
              onOpened: _load,
              onSelected: (id) => widget.onPick(
                id == _moneypenny
                    ? null
                    : _options.firstWhere((option) => option.id == id),
              ),
              itemBuilder: (_) => [
                PopupMenuItem(
                  key: const ValueKey('talking-to-moneypenny'),
                  value: _moneypenny,
                  child: const Text('Moneypenny'),
                ),
                for (final group in groups) ...[
                  PopupMenuItem<String>(
                    enabled: false,
                    child: Text(group, style: text.labelSmall),
                  ),
                  for (final option in _options.where(
                    (option) => option.group == group,
                  ))
                    PopupMenuItem(
                      key: ValueKey('talking-to-${option.id}'),
                      value: option.id,
                      child: Text(option.label),
                    ),
                ],
              ],
              child: Chip(
                avatar: Icon(
                  Icons.forum_outlined,
                  size: 16,
                  color: tokens.accent,
                ),
                label: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Flexible(
                      child: Text(
                        talking?.label ?? 'Moneypenny',
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    const Icon(Icons.arrow_drop_down, size: 20),
                  ],
                ),
                visualDensity: VisualDensity.compact,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
