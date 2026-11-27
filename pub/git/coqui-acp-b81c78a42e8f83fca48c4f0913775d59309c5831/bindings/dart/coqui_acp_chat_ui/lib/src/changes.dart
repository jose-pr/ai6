import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:flutter/material.dart';

import 'diff.dart';
import 'entry.dart';
import 'look.dart';


/// One file a session's tools changed, as the agent reported it: what it showed before the first change and after the last, and
/// any patches it wrote. Built from ACP updates alone, so it is what the agent sent, never the file on disk.
final class FileChange {
  FileChange(this.path);
  final String path;

  /// The text before the first change the agent showed as whole text; null for a new file, or when it sent only patches.
  String? before;

  /// The text after the last such change.
  String? after;

  /// Whether a change came as before-and-after text.
  bool whole = false;

  /// The patches the agent wrote for this file, in order.
  final List<String> patches = [];

  /// Whether any of it was cut at [ChatLimits.toolText].
  bool cut = false;

  /// Changes whenever what the file shows changes, so a mark of viewed holds only for what was viewed.
  int get version => Object.hash(before, after, Object.hashAll(patches));

  /// Its lines, every one numbered in the old and new text where it has a place there.
  late final List<ChangeLine> lines = [
    if (whole) ..._numbered(diffLines(before, after ?? '')),
    for (final patch in patches) ..._patchNumbered(patch),
  ];

  int get added => lines.where((line) => line.kind == DiffKind.added).length;
  int get removed => lines.where((line) => line.kind == DiffKind.removed).length;
}

/// A line of a change, with its numbers in the text before and after; null marks lines left out between hunks.
final class ChangeLine {
  const ChangeLine(this.kind, this.text, {this.old, this.now});
  final DiffKind kind;
  final String text;
  final int? old, now;
}

List<ChangeLine> _numbered(List<DiffLine> lines) {
  var old = 0, now = 0;
  return [
    for (final line in lines)
      switch (line.kind) {
        DiffKind.same => ChangeLine(line.kind, line.text, old: ++old, now: ++now),
        DiffKind.removed => ChangeLine(line.kind, line.text, old: ++old),
        DiffKind.added => ChangeLine(line.kind, line.text, now: ++now),
      },
  ];
}

final _hunk = RegExp(r'^@@ -(\d+)(?:,\d+)? \+(\d+)');

/// A unified patch's lines, numbered from each hunk's header; header lines are left out.
List<ChangeLine> _patchNumbered(String patch) {
  final out = <ChangeLine>[];
  var old = 0, now = 0;
  for (final line in patch.split('\n')) {
    final hunk = _hunk.firstMatch(line);
    if (hunk != null) {
      old = int.parse(hunk[1]!) - 1;
      now = int.parse(hunk[2]!) - 1;
    } else if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff ') || line.startsWith('index ')) {
      continue;
    } else if (line.startsWith('+')) {
      out.add(ChangeLine(DiffKind.added, line.substring(1), now: ++now));
    } else if (line.startsWith('-')) {
      out.add(ChangeLine(DiffKind.removed, line.substring(1), old: ++old));
    } else if (line.isNotEmpty && line != r'\ No newline at end of file') {
      out.add(ChangeLine(DiffKind.same, line.startsWith(' ') ? line.substring(1) : line, old: ++old, now: ++now));
    }
  }
  return out;
}

bool _wasCut(String? text) => text != null && text.endsWith('\n…');

/// Every file the tools of [entries] changed, in the order each was first changed. Only tools that completed count: a refused or
/// failed edit changed nothing.
List<FileChange> changesOf(Iterable<ChatEntryData> entries) {
  final files = <String, FileChange>{};
  for (final message in entries) {
    final tool = message.tool;
    if (tool == null || tool.status != ToolStatus.done) continue;
    for (final block in tool.blocks) {
      switch (block) {
        case ToolDiff(:final path, :final oldText, :final newText):
          final file = files.putIfAbsent(path, () => FileChange(path));
          if (!file.whole) file.before = oldText;
          file
            ..whole = true
            ..after = newText
            ..cut |= _wasCut(oldText) || _wasCut(newText);
        case ToolPatch(:final text, :final paths):
          for (final path in paths.isEmpty ? const ['a file'] : paths) {
            files.putIfAbsent(path, () => FileChange(path))
              ..patches.add(text)
              ..cut |= _wasCut(text);
          }
        case ToolText() || ToolTerminal():
          break;
      }
    }
  }
  return files.values.toList();
}

/// The changes of a session, by file, each with its additions and removals, a mark of viewed, and its lines; choosing a line
/// writes a comment on it, with the file and line, into the message being written.
class ChangesPane extends StatelessWidget {
  const ChangesPane({
    super.key,
    required this.files,
    required this.isViewed,
    required this.onViewed,
    required this.onComment,
    this.onClose,
    this.color,
    this.dialog,
  });

  final List<FileChange> files;

  /// Whether the person marked [file] viewed as it is now.
  final bool Function(FileChange file) isViewed;

  /// Marks [file] viewed as it is now, or takes the mark back.
  final void Function(FileChange file, bool viewed) onViewed;

  /// Adds text to the message being written.
  final ValueChanged<String> onComment;
  final VoidCallback? onClose;

  /// The pane's background; the look's `surface` when null.
  final Color? color;

  /// Shows a dialog; [showDialog] when null.
  final Future<String?> Function(BuildContext context, WidgetBuilder builder)? dialog;

  @override
  Widget build(BuildContext context) {
    final viewed = files.where(isViewed).length;
    final muted = TextStyle(color: context.chatLook.muted, fontSize: 12);
    return Material(
      color: color ?? context.chatLook.surface,
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 8, 0),
          child: Row(children: [
            Expanded(child: Text('Changes', style: Theme.of(context).textTheme.titleMedium)),
            Text('$viewed of ${files.length} viewed', style: muted),
            if (onClose != null) IconButton(tooltip: 'Close changes', onPressed: onClose, icon: const Icon(Icons.close)),
          ]),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
          child: Text('As the agent sent them, not as the files are on disk now. Choose a line to comment on it.', style: muted),
        ),
        const Divider(height: 1),
        Expanded(
          child: files.isEmpty
              ? const Center(child: Text('No file has changed yet.'))
              : ListView(children: [
                  for (final file in files)
                    _FileView(
                      key: ValueKey('file ${file.path} ${file.version}'),
                      file: file,
                      viewed: isViewed(file),
                      onViewed: (on) => onViewed(file, on),
                      onComment: (line) => _comment(context, file, line),
                    ),
                ]),
        ),
      ]),
    );
  }

  Future<void> _comment(BuildContext context, FileChange file, ChangeLine line) async {
    final at = line.now != null ? '${file.path}:${line.now}' : '${file.path}, removed line ${line.old}';
    final show = dialog ?? (context, builder) => showDialog<String>(context: context, builder: builder);
    final text = await show(context, (context) => _CommentDialog(at: at));
    if (text == null || text.trim().isEmpty) return;
    onComment('$at: `${line.text}`\n${text.trim()}\n');
  }
}

/// Asks for a comment on a line, which it answers.
class _CommentDialog extends StatefulWidget {
  const _CommentDialog({required this.at});

  final String at;

  @override
  State<_CommentDialog> createState() => _CommentDialogState();
}

class _CommentDialogState extends State<_CommentDialog> {
  final _text = TextEditingController();

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text('Comment on ${widget.at}'),
    content: SizedBox(
      width: 420,
      child: TextField(
        key: const Key('change-comment'),
        controller: _text,
        autofocus: true,
        minLines: 2,
        maxLines: 6,
        decoration: const InputDecoration(hintText: 'What should change here?'),
      ),
    ),
    actions: [
      TextButton(onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
      FilledButton(onPressed: () => Navigator.pop(context, _text.text), child: const Text('Add to message')),
    ],
  );
}

class _FileView extends StatefulWidget {
  const _FileView({super.key, required this.file, required this.viewed, required this.onViewed, required this.onComment});

  final FileChange file;
  final bool viewed;
  final ValueChanged<bool> onViewed;
  final ValueChanged<ChangeLine> onComment;

  @override
  State<_FileView> createState() => _FileViewState();
}

class _FileViewState extends State<_FileView> {
  /// A file marked viewed starts folded.
  late bool _open = !widget.viewed;

  @override
  Widget build(BuildContext context) {
    final file = widget.file;
    final look = context.chatLook;
    final mono = TextStyle(fontFamily: look.codeFont, fontSize: 12, height: 1.35);
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      InkWell(
        onTap: () => setState(() => _open = !_open),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
          child: Row(children: [
            Checkbox(
              key: ValueKey('viewed ${file.path}'),
              value: widget.viewed,
              onChanged: (on) {
                widget.onViewed(on ?? false);
                setState(() => _open = !(on ?? false));
              },
            ),
            Expanded(child: Text(file.path, style: mono, overflow: TextOverflow.ellipsis)),
            Text('+${file.added}', style: mono.copyWith(color: look.success)),
            const SizedBox(width: 6),
            Text('−${file.removed}', style: mono.copyWith(color: look.danger)),
            Icon(_open ? Icons.expand_less : Icons.expand_more, size: 18),
          ]),
        ),
      ),
      if (_open) ...[
        if (file.cut)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 4),
            child: Text('The agent\'s text was cut short here, so this is not the whole change.',
                style: TextStyle(color: look.muted, fontSize: 12, fontWeight: FontWeight.w600)),
          ),
        Container(
          margin: const EdgeInsets.fromLTRB(12, 0, 12, 8),
          constraints: const BoxConstraints(maxHeight: 480),
          decoration: BoxDecoration(color: look.code, borderRadius: BorderRadius.circular(look.radius / 2)),
          child: SingleChildScrollView(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                for (final line in _shown(file.lines))
                  if (line == null)
                    Padding(padding: const EdgeInsets.symmetric(horizontal: 8), child: Text('⋯', style: mono.copyWith(color: look.muted)))
                  else
                    InkWell(
                      key: ValueKey('line ${file.path} ${line.now != null ? 'new ${line.now}' : 'old ${line.old}'}'),
                      onTap: () => widget.onComment(line),
                      child: Container(
                        color: switch (line.kind) {
                          DiffKind.added => look.added,
                          DiffKind.removed => look.removed,
                          DiffKind.same => null,
                        },
                        padding: const EdgeInsets.only(right: 8),
                        child: Text(
                          '${'${line.now ?? ''}'.padLeft(4)} ${switch (line.kind) { DiffKind.added => '+', DiffKind.removed => '-', DiffKind.same => ' ' }} ${line.text}',
                          style: mono,
                          softWrap: false,
                        ),
                      ),
                    ),
              ]),
            ),
          ),
        ),
      ],
    ]);
  }

  /// The changed lines with three around each, a null where lines are left out.
  static List<ChangeLine?> _shown(List<ChangeLine> lines) {
    final keep = List<bool>.filled(lines.length, false);
    for (var i = 0; i < lines.length; i++) {
      if (lines[i].kind == DiffKind.same) continue;
      for (var at = i - 3; at <= i + 3; at++) {
        if (at >= 0 && at < lines.length) keep[at] = true;
      }
    }
    final out = <ChangeLine?>[];
    for (var i = 0; i < lines.length; i++) {
      if (keep[i]) {
        out.add(lines[i]);
      } else if (out.isEmpty || out.last != null) {
        out.add(null);
      }
    }
    return out;
  }
}
