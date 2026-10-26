import 'dart:convert';

import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import 'assistant_controller.dart';
import 'assistant_view.dart';

/// The chat with Moneypenny: her answers, the tools she used, her views and the changes she
/// proposes, with Confirm and Cancel. Each message carries [page], the app page the person
/// is on, so "this Mission" means something to her.
class AssistantPanel extends StatefulWidget {
  const AssistantPanel({
    required this.controller,
    this.page = const {},
    this.onOpen,
    super.key,
  });

  final AssistantController controller;
  final Map<String, String> page;

  /// Opens an app page a view links to (`app:/missions/G/m-1`).
  final void Function(String link)? onOpen;

  @override
  State<AssistantPanel> createState() => _AssistantPanelState();
}

class _AssistantPanelState extends State<AssistantPanel> {
  final _message = TextEditingController();

  @override
  void initState() {
    super.initState();
    widget.controller.open();
  }

  @override
  void dispose() {
    _message.dispose();
    super.dispose();
  }

  void _send() {
    final text = _message.text;
    if (text.trim().isEmpty || widget.controller.busy) return;
    widget.controller.say(text, page: widget.page);
    _message.clear();
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.controller,
    builder: (context, _) {
      final controller = widget.controller;
      final theme = Theme.of(context);
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ListTile(
            leading: const Icon(Icons.support_agent),
            title: const Text('Moneypenny'),
            subtitle: Text(
              !controller.connected
                  ? 'Connecting…'
                  : controller.busy
                  ? 'Answering…'
                  : 'Ask about AI6, or ask her to propose a change.',
            ),
            trailing: IconButton(
              tooltip: 'New conversation',
              onPressed: controller.connected ? controller.startOver : null,
              icon: const Icon(Icons.add_comment_outlined),
            ),
          ),
          const Divider(height: 1),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.all(12),
              reverse: true,
              children: [
                for (final entry in controller.entries.reversed)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 10),
                    child: _EntryView(
                      entry: entry,
                      controller: controller,
                      onOpen: widget.onOpen,
                    ),
                  ),
              ],
            ),
          ),
          if (controller.busy) const LinearProgressIndicator(),
          if (controller.problem != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
              child: Text(
                controller.problem!,
                style: TextStyle(color: theme.colorScheme.error),
              ),
            ),
          Padding(
            padding: const EdgeInsets.all(12),
            child: Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _message,
                    minLines: 1,
                    maxLines: 4,
                    enabled: controller.connected,
                    decoration: const InputDecoration(
                      hintText: 'Message Moneypenny',
                    ),
                    onSubmitted: (_) => _send(),
                  ),
                ),
                const SizedBox(width: 8),
                IconButton.filled(
                  tooltip: 'Send',
                  onPressed: controller.connected && !controller.busy
                      ? _send
                      : null,
                  icon: const Icon(Icons.send),
                ),
              ],
            ),
          ),
        ],
      );
    },
  );
}

class _EntryView extends StatelessWidget {
  const _EntryView({
    required this.entry,
    required this.controller,
    this.onOpen,
  });

  final AssistantEntry entry;
  final AssistantController controller;
  final void Function(String link)? onOpen;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return switch (entry.role) {
      'person' => Align(
        alignment: Alignment.centerRight,
        child: Container(
          padding: const EdgeInsets.all(10),
          decoration: BoxDecoration(
            color: colors.primaryContainer,
            borderRadius: BorderRadius.circular(12),
          ),
          child: Text(revealInvisible(entry.text)),
        ),
      ),
      'moneypenny' => MarkdownText(entry.text, onOpen: onOpen),
      'tools' => Wrap(
        spacing: 6,
        children: [
          for (final tool in entry.text.split(', '))
            Chip(
              avatar: const Icon(Icons.build_outlined, size: 16),
              label: Text(_toolName(tool)),
            ),
        ],
      ),
      'proposal' => ProposalCard(
        proposal: entry.data ?? const {},
        onConfirm: controller.confirm,
        onCancel: controller.cancel,
      ),
      'view' => AssistantViewCard(
        json: entry.data?['view'],
        onOpen: onOpen,
        onSubmit: entry.id == null
            ? null
            : (values) => controller.submit(entry.id!, values),
      ),
      _ => Text(
        revealInvisible(entry.text),
        style: TextStyle(color: colors.error),
      ),
    };
  }
}

/// `mcp__moneypenny__board` as `board`.
String _toolName(String tool) => tool.split('__').last;

/// A change Moneypenny proposed: what it is, the exact request, the permission it needs,
/// and, while it waits, Confirm and Cancel.
class ProposalCard extends StatelessWidget {
  const ProposalCard({
    required this.proposal,
    required this.onConfirm,
    required this.onCancel,
    super.key,
  });

  final Map<String, Object?> proposal;
  final void Function(String id) onConfirm;
  final void Function(String id) onCancel;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final id = proposal['id'] as String? ?? '';
    final status = proposal['status'] as String? ?? 'pending';
    final request =
        (proposal['request'] as Map?)?.cast<String, Object?>() ?? {};
    final needs = (proposal['needs'] as Map?)?.cast<String, Object?>() ?? {};
    final body = request['body'];
    final result = proposal['result'] as String?;
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          spacing: 6,
          children: [
            Row(
              children: [
                const Icon(Icons.edit_note),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    revealInvisible(proposal['summary'] as String? ?? ''),
                    style: theme.textTheme.titleSmall,
                  ),
                ),
                Chip(label: Text(status)),
              ],
            ),
            SelectableText(
              '${request['method']} ${request['path']}'
              '${body == null ? '' : '\n${const JsonEncoder.withIndent('  ').convert(body)}'}',
              style: const TextStyle(fontFamily: 'monospace'),
            ),
            Text(
              'Needs ${needs['action']} on ${needs['on']}. Nothing changes until you confirm.',
              style: theme.textTheme.bodySmall,
            ),
            if (result != null) Text(revealInvisible(result)),
            if (status == 'pending')
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                spacing: 8,
                children: [
                  TextButton(
                    onPressed: () => onCancel(id),
                    child: const Text('Cancel'),
                  ),
                  FilledButton(
                    onPressed: () => onConfirm(id),
                    child: const Text('Confirm'),
                  ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}
