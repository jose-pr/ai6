import 'dart:async';

import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:flutter/foundation.dart' show defaultTargetPlatform, TargetPlatform;
import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';

import 'look.dart';
import 'usage_text.dart';

/// What was copied: an image, files by their paths, or text.
abstract interface class ClipboardReader {
  Future<Uint8List?> image();
  Future<List<String>> files();
  Future<String?> text();
}

/// Text only, which is all Flutter's own clipboard reads.
final class TextClipboard implements ClipboardReader {
  const TextClipboard();
  @override
  Future<Uint8List?> image() async => null;
  @override
  Future<List<String>> files() async => const [];
  @override
  Future<String?> text() async => (await Clipboard.getData(Clipboard.kTextPlain))?.text;
}

/// A file the person picked, as the composer takes it from the host's picker.
typedef PickedFile = ({String name, String path, String? mimeType, Future<Uint8List> Function() read});

/// What goes with the next message, as a chip shows it.
final class ComposerChip {
  const ComposerChip({
    required this.key,
    required this.name,
    required this.detail,
    this.kind = ChipKind.file,
    this.thumbnail,
    this.note,
    this.refused = false,
  });

  /// Names the chip to [ComposerAttachments.remove].
  final Object key;
  final String name;

  /// Its size, or what it is.
  final String detail;
  final ChipKind kind;

  /// An image's bytes, for a thumbnail.
  final List<int>? thumbnail;

  /// How it will reach the agent when that is not as it is, or why it cannot.
  final String? note;
  final bool refused;
}

enum ChipKind { image, text, file, folder }

/// What `@` can name: a file or folder the host knows, as the list shows it.
final class MentionCandidate {
  const MentionCandidate({required this.label, this.detail, this.folder = false, this.key, this.group, this.icon});

  /// What is written after the `@` when it is chosen, and what the list shows.
  final String label;

  /// A second line, such as the folder it is in.
  final String? detail;
  final bool folder;

  /// What the host knows it by; [label] when null.
  final Object? key;

  /// The heading the list shows over it, when the host offers more than files: candidates of one group should be together. None
  /// puts no heading.
  final String? group;

  /// Its icon; a folder's or a file's when null.
  final IconData? icon;
}

/// What the composer asks of a host that lets the person name things with `@`.
abstract interface class ComposerMentions {
  /// What [query], the text typed after the `@`, names, best first. Nothing the person typed is a path to read: it only
  /// narrows what the host chooses to offer.
  Future<List<MentionCandidate>> search(String query);

  /// Takes the choice of [candidate]. Answers why it cannot be taken, or null once it is; the text is then left as it was.
  String? pick(MentionCandidate candidate);
}

/// An action of the host's own that `/` lists beside the agent's commands. Choosing it runs [run] at once; nothing is sent.
final class ComposerAction {
  const ComposerAction({required this.name, required this.description, required this.run, this.group});
  final String name, description;
  final VoidCallback run;

  /// The heading the entry is listed under; the composer's `actionsTitle` when null. Entries of one group should be together.
  final String? group;
}

/// The `@` being typed at [caret] in [text]: where it starts and the query after it, or null when the caret is not in one.
/// An `@` counts at the start or after whitespace, and the query ends at the first whitespace.
({int start, String query})? mentionAt(String text, int caret) {
  if (caret < 0 || caret > text.length) return null;
  final space = RegExp(r'\s');
  for (var i = caret - 1; i >= 0; i--) {
    if (text.codeUnitAt(i) == 0x40) {
      if (i > 0 && !space.hasMatch(text[i - 1])) return null;
      return (start: i, query: text.substring(i + 1, caret));
    }
    if (space.hasMatch(text[i])) return null;
  }
  return null;
}

/// What the composer asks of a host that takes attachments. Each `add` answers a problem to tell the person, or nothing.
abstract interface class ComposerAttachments {
  /// Whether anything can be added now.
  bool get canAttach;

  List<ComposerChip> get chips;

  /// Adds what the person picked, by path where the host can read paths and by bytes otherwise.
  Future<List<String>> addFiles(List<PickedFile> files);

  /// Adds files by their paths, such as copied files or dropped ones.
  Future<List<String>> addPaths(List<String> paths);

  /// Adds an image pasted from the clipboard.
  String? addImage(Uint8List bytes);

  /// Adds a long text as an attachment of its own.
  String? addText(String text);

  /// Adds a folder, to be linked by its path.
  String? addFolder(String path);

  void remove(Object key);
}

/// Makes the [ComposerPill]s below it plain text: no icon and no chevron, as in [Composer.below].
class ComposerQuiet extends InheritedWidget {
  const ComposerQuiet({super.key, required super.child});

  static bool of(BuildContext context) => context.dependOnInheritedWidgetOfExactType<ComposerQuiet>() != null;

  @override
  bool updateShouldNotify(ComposerQuiet old) => false;
}

/// A compact control: in the line under the composer's box, plain text; elsewhere, with its icon and a chevron when it opens a menu.
class ComposerPill extends StatelessWidget {
  const ComposerPill({super.key, required this.label, this.icon, this.leading, this.danger = false, this.status = false, this.enabled = true, this.maxWidth = 180, this.quiet});

  final String label;
  final IconData? icon;

  /// A widget drawn where [icon] would be, in the same size and colour: an app's own mark for a source or an agent.
  final Widget? leading;

  /// [danger] is for what is wrong; [status] for what is on, never the colour of an error.
  final bool danger, status, enabled;

  /// The widest the label gets before it is cut short.
  final double maxWidth;

  /// Whether it draws only its label, in the muted colour; null follows [ComposerQuiet], which [Composer.below] puts its children
  /// in. The app's own [leading] mark stays, since it says what the control is about.
  final bool? quiet;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final plain = quiet ?? ComposerQuiet.of(context);
    final color = danger ? look.danger : status ? look.warning : (enabled ? null : look.muted);
    return Padding(
      padding: EdgeInsets.symmetric(horizontal: plain ? 6 : 8, vertical: plain ? 4 : 6),
      child: Row(mainAxisSize: MainAxisSize.min, children: [
        if (icon != null && !plain) ...[Icon(icon, size: 16, color: color ?? look.muted), const SizedBox(width: 4)],
        if (leading != null) ...[
          IconTheme(data: IconThemeData(size: plain ? 14 : 16, color: color ?? look.muted), child: leading!),
          const SizedBox(width: 4),
        ],
        Flexible(
          child: ConstrainedBox(
            constraints: BoxConstraints(maxWidth: maxWidth),
            child: Text(label, maxLines: 1, overflow: TextOverflow.ellipsis, style: TextStyle(fontSize: plain ? 12 : 12.5, color: color ?? (plain ? look.muted : null))),
          ),
        ),
        if (enabled && !plain) Icon(Icons.expand_more, size: 16, color: color ?? look.muted),
      ]),
    );
  }
}

/// The message being written, in one rounded box: the attachments as cards above the text, `+` at its left, and at its right the
/// host's [primary] control while the message is empty, then Send once it holds something (Queue, Steer and Stop while a turn runs).
/// A line of the host's own choices sits under the box.
///
/// What is the host's goes in slots: [header] at the top of the box, [primary] at the right, [plusItems] in the `+` menu and
/// [below] under the box. The package supplies the field, the `/` suggestions, paste, the attachment cards and the files and
/// folder entries of `+`, and Send, Queue, Steer and Stop.
class Composer extends StatefulWidget {
  const Composer({
    super.key,
    required this.controller,
    required this.onSend,
    this.focusNode,
    this.enabled = true,
    this.working = false,
    this.starting = false,
    this.onStop,
    this.onSteer,
    this.enterSteers = false,
    this.canQueue = true,
    this.onSuggestions,
    this.suggestionLimit = CommandSuggestions.shown,
    this.commands = const [],
    this.actions = const [],
    this.commandsTitle = 'Agent commands',
    this.actionsTitle = 'Host actions',
    this.mentions,
    this.centred = false,
    this.hint = 'Message',
    this.header,
    this.attachments,
    this.pickFiles,
    this.pickFolder,
    this.clipboard = const TextClipboard(),
    this.pasteAsFile = defaultPasteAsFile,
    this.primary,
    this.holdPrimary = false,
    this.below = const [],
    this.plusItems = const [],
    this.onShiftTab,
    this.recall,
  });

  /// The most characters a paste puts in the text; a longer one goes as an attachment of its own.
  static const defaultPasteAsFile = 8000;

  /// From this width the line under the box is right-aligned; narrower, it starts at the left and wraps.
  static const double wideFrom = 600;

  final TextEditingController controller;
  final FocusNode? focusNode;
  final VoidCallback onSend;

  /// Whether the field takes text and Send works.
  final bool enabled;

  /// Whether a turn is running: Send becomes Queue (and Steer), and Stop appears.
  final bool working;

  /// Stops the turn and sends the message first. With it, a turn running shows Steer beside Queue and Stop; [onSend] is Queue.
  /// Enter does one of the two and Alt+Enter the other, as [enterSteers] says.
  final VoidCallback? onSteer;

  /// Whether Enter steers while a turn runs and Alt+Enter queues, rather than the reverse.
  final bool enterSteers;

  /// Whether the conversation is starting: a spinner stands in for Send.
  final bool starting;
  final VoidCallback? onStop;

  /// Whether a message can wait for the turn to end. Without it, a turn running shows Stop alone and Enter sends nothing.
  final bool canQueue;

  /// Told whether the `/` list is showing, so a host's own keys leave Enter, Tab and Escape to it while it is.
  final ValueChanged<bool>? onSuggestions;

  /// The most each group of the `/` list holds; the list scrolls past a screenful when it holds more than the default.
  final int suggestionLimit;

  /// The agent's `/` commands.
  final List<AgentCommandInfo> commands;

  /// The host's own `/` actions. With any, the list has two groups, titled [commandsTitle] and [actionsTitle]; with none it is
  /// the agent's commands alone, untitled.
  final List<ComposerAction> actions;
  final String commandsTitle, actionsTitle;

  /// What an `@` in the message names; null leaves `@` as text.
  final ComposerMentions? mentions;

  /// Whether it is the middle of a new session's empty view rather than docked under a conversation.
  final bool centred;

  /// The placeholder, kept to one line.
  final String hint;

  /// Drawn at the top of the box, above the attachments.
  final Widget? header;
  final ComposerAttachments? attachments;
  final Future<List<PickedFile>> Function()? pickFiles;
  final Future<String?> Function()? pickFolder;
  final ClipboardReader clipboard;
  final int pasteAsFile;

  /// The host's one control at the right of the box, such as a microphone: shown while the message is empty (and has no
  /// attachment), and put away for Send once it holds something, with its state kept. Without it Send is always shown.
  final Widget? primary;

  /// Keeps [primary] in view although the message holds text, beside Send: for a control that is mid-action, such as a dictation
  /// that is listening, which would end if its button left.
  final bool holdPrimary;

  /// The host's choices (a model, a mode, an account), in a line under the box: [ComposerPill]s there draw as plain text. It is
  /// right-aligned from [wideFrom] and wraps, from the left, below it. Empty leaves no line.
  final List<Widget> below;

  /// Entries at the top of the `+` menu.
  final List<Widget> plusItems;

  /// What Shift+Tab does in the field, usually putting the conversation in its next mode; null leaves the key alone.
  final VoidCallback? onShiftTab;

  /// The last message the person sent here, or null: Up in an empty message brings it back, and anywhere else moves the cursor as
  /// usual.
  final String? Function()? recall;

  @override
  State<Composer> createState() => ComposerState();
}

class ComposerState extends State<Composer> {
  int _highlight = 0;
  String? _closedFor;
  String? _lastTyped;
  bool _shown = false;

  /// The `/` list for the text now in the field, none after Escape closed it for this text.
  Suggested get _suggested {
    final typed = widget.controller.text;
    if (typed == _closedFor) return (commands: const [], actions: const []);
    return suggestionsFor(typed, widget.commands, widget.actions, limit: widget.suggestionLimit);
  }

  /// Takes the row [index] of the list: a command's name goes into the field, an action runs.
  void _pickRow(Suggested found, int index) {
    if (index < 0 || index >= found.commands.length + found.actions.length) return;
    if (index < found.commands.length) {
      final text = '/${found.commands[index].name} ';
      widget.controller.value = TextEditingValue(text: text, selection: TextSelection.collapsed(offset: text.length));
    } else {
      widget.controller.value = const TextEditingValue();
      found.actions[index - found.commands.length].run();
    }
  }

  void _reportShown(bool shown) {
    if (shown == _shown) return;
    _shown = shown;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) widget.onSuggestions?.call(shown);
    });
  }

  @override
  void dispose() {
    if (_shown) widget.onSuggestions?.call(false);
    super.dispose();
  }

  void _say(String text) => ScaffoldMessenger.maybeOf(context)?.showSnackBar(SnackBar(content: Text(text)));

  void _sayAll(Iterable<String> problems) {
    if (problems.isNotEmpty) _say(problems.join('\n'));
  }

  Future<void> _addFiles() async {
    final host = widget.attachments;
    final picked = await widget.pickFiles?.call() ?? const <PickedFile>[];
    if (!mounted || host == null) return;
    final problems = await host.addFiles(picked);
    if (mounted) _sayAll(problems);
  }

  Future<void> _addFolder() async {
    final folder = await widget.pickFolder?.call();
    if (folder == null || !mounted) return;
    final problem = widget.attachments?.addFolder(folder);
    if (problem != null) _say(problem);
  }

  /// Chooses [candidate] for the `@` that starts at [start] and runs to [end]: the host takes it, and the text becomes `@label `.
  void _pickMention(MentionCandidate candidate, int start, int end) {
    final problem = widget.mentions?.pick(candidate);
    if (problem != null) {
      _say(problem);
      return;
    }
    final text = widget.controller.text;
    if (start < 0 || end > text.length || start > end) return;
    final inserted = '@${candidate.label} ';
    widget.controller.value = TextEditingValue(
      text: text.replaceRange(start, end, inserted),
      selection: TextSelection.collapsed(offset: start + inserted.length),
    );
  }

  /// What the clipboard holds: an image becomes an image attachment, copied files file attachments, and text stays text unless it
  /// is longer than [Composer.pasteAsFile] characters.
  Future<void> paste() async {
    final clipboard = widget.clipboard;
    final host = widget.attachments;
    final image = await clipboard.image();
    if (!mounted) return;
    if (image != null && image.isNotEmpty && host != null) {
      final problem = host.addImage(image);
      if (problem != null) _say(problem);
      return;
    }
    final files = await clipboard.files();
    if (!mounted) return;
    if (files.isNotEmpty && host != null) {
      _sayAll(await host.addPaths(files));
      return;
    }
    final text = await clipboard.text();
    if (text == null || text.isEmpty || !mounted) return;
    if (text.length > widget.pasteAsFile && host != null) {
      final problem = host.addText(text);
      if (problem != null) _say(problem);
      return;
    }
    final value = widget.controller.value;
    final selection = value.selection.isValid ? value.selection : TextSelection.collapsed(offset: value.text.length);
    final joined = value.text.replaceRange(selection.start, selection.end, text);
    widget.controller.value = TextEditingValue(
      text: joined,
      selection: TextSelection.collapsed(offset: selection.start + text.length),
    );
  }

  bool get _steering => widget.working && widget.onSteer != null;

  /// What Enter does: send, or while a turn runs queue or steer as [Composer.enterSteers] says.
  void _enter() {
    if (widget.working && !widget.canQueue && !_steering) return;
    (_steering && widget.enterSteers ? widget.onSteer! : widget.onSend)();
  }

  /// Shift+Tab goes to [Composer.onShiftTab], Up in an empty message brings back [Composer.recall], Alt+Enter while a turn runs
  /// does what Enter does not (queue or steer), and Ctrl+V (⌘V on a Mac) pastes through [paste]; every other key goes on as usual.
  KeyEventResult onKey(FocusNode node, KeyEvent event) {
    final found = _suggested;
    final rows = found.commands.length + found.actions.length;
    if (rows > 0 && (event is KeyDownEvent || event is KeyRepeatEvent)) {
      final key = event.logicalKey;
      final keys = HardwareKeyboard.instance;
      final plain = !keys.isShiftPressed && !keys.isAltPressed && !keys.isControlPressed && !keys.isMetaPressed;
      if (key == LogicalKeyboardKey.arrowDown) {
        setState(() => _highlight = (_highlight + 1) % rows);
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.arrowUp) {
        setState(() => _highlight = (_highlight - 1 + rows) % rows);
        return KeyEventResult.handled;
      }
      if (event is KeyDownEvent && plain && (key == LogicalKeyboardKey.enter || key == LogicalKeyboardKey.numpadEnter || key == LogicalKeyboardKey.tab)) {
        _pickRow(found, _highlight.clamp(0, rows - 1));
        return KeyEventResult.handled;
      }
      if (event is KeyDownEvent && key == LogicalKeyboardKey.escape) {
        setState(() => _closedFor = widget.controller.text);
        return KeyEventResult.handled;
      }
    }
    if (event is KeyDownEvent &&
        (event.logicalKey == LogicalKeyboardKey.enter || event.logicalKey == LogicalKeyboardKey.numpadEnter) &&
        HardwareKeyboard.instance.isAltPressed &&
        !HardwareKeyboard.instance.isControlPressed &&
        !HardwareKeyboard.instance.isShiftPressed &&
        _steering &&
        widget.enabled) {
      (widget.enterSteers ? widget.onSend : widget.onSteer!)();
      return KeyEventResult.handled;
    }
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.tab &&
        HardwareKeyboard.instance.isShiftPressed &&
        !HardwareKeyboard.instance.isControlPressed &&
        !HardwareKeyboard.instance.isAltPressed &&
        widget.onShiftTab != null) {
      widget.onShiftTab!();
      return KeyEventResult.handled;
    }
    if ((event is KeyDownEvent || event is KeyRepeatEvent) && event.logicalKey == LogicalKeyboardKey.arrowUp && widget.controller.text.isEmpty) {
      final last = widget.recall?.call();
      if (last != null && last.isNotEmpty) {
        widget.controller.value = TextEditingValue(text: last, selection: TextSelection.collapsed(offset: last.length));
        return KeyEventResult.handled;
      }
    }
    if (event is! KeyDownEvent || event.logicalKey != LogicalKeyboardKey.keyV || !widget.enabled) return KeyEventResult.ignored;
    final keys = HardwareKeyboard.instance;
    final mac = defaultTargetPlatform == TargetPlatform.macOS || defaultTargetPlatform == TargetPlatform.iOS;
    if ((mac ? keys.isMetaPressed : keys.isControlPressed) && !keys.isAltPressed && !keys.isShiftPressed) {
      unawaited(paste());
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final chips = widget.attachments?.chips ?? const <ComposerChip>[];
    final field = TextField(
      controller: widget.controller,
      focusNode: widget.focusNode,
      enabled: widget.enabled,
      minLines: 1,
      maxLines: widget.centred ? 10 : 5,
      textInputAction: TextInputAction.send,
      onSubmitted: (_) => _enter(),
      decoration: InputDecoration(
        hintText: widget.hint,
        hintMaxLines: 1,
        border: InputBorder.none,
        enabledBorder: InputBorder.none,
        focusedBorder: InputBorder.none,
        disabledBorder: InputBorder.none,
        filled: false,
        isDense: true,
        contentPadding: const EdgeInsets.fromLTRB(4, 8, 4, 8),
      ),
    );
    final box = Container(
      key: const Key('composer'),
      padding: const EdgeInsets.fromLTRB(6, 6, 6, 6),
      // A centred box is taller, and what is written grows it upward: the row of +, the text and the button stays at the bottom.
      constraints: widget.centred ? const BoxConstraints(minHeight: 112) : null,
      alignment: widget.centred ? Alignment.bottomCenter : null,
      decoration: BoxDecoration(color: look.composer, borderRadius: BorderRadius.circular(look.radius + 8)),
      child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        ?widget.header,
        if (chips.isNotEmpty) AttachmentChips(chips: chips, onRemove: widget.attachments!.remove),
        Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
          _PlusButton(composer: this),
          Expanded(child: Focus(canRequestFocus: false, skipTraversal: true, onKeyEvent: onKey, child: field)),
          _Trailing(composer: this, attached: chips.isNotEmpty),
        ]),
      ]),
    );
    return Column(mainAxisSize: MainAxisSize.min, children: [
      ValueListenableBuilder<TextEditingValue>(
        valueListenable: widget.controller,
        builder: (context, value, _) {
          final caret = value.selection.isValid ? value.selection.baseOffset : value.text.length;
          final mention = widget.mentions == null ? null : mentionAt(value.text, caret);
          return Column(mainAxisSize: MainAxisSize.min, children: [
            Builder(builder: (context) {
              // Typing on opens the list again, from its first row.
              if (value.text != _lastTyped) {
                _lastTyped = value.text;
                _highlight = 0;
                if (value.text != _closedFor) _closedFor = null;
              }
              final found = _suggested;
              _reportShown(found.commands.isNotEmpty || found.actions.isNotEmpty);
              return CommandSuggestions(
                commands: widget.commands,
                actions: widget.actions,
                commandsTitle: widget.commandsTitle,
                actionsTitle: widget.actionsTitle,
                typed: value.text == _closedFor ? '' : value.text,
                limit: widget.suggestionLimit,
                highlighted: _highlight,
                onPick: (command) => _pickRow(found, found.commands.indexWhere((c) => c.name == command.name)),
                onRun: (action) => _pickRow(found, found.commands.length + found.actions.indexOf(action)),
              );
            }),
            if (mention != null)
              MentionSuggestions(
                key: const Key('mention-suggestions'),
                source: widget.mentions!,
                query: mention.query,
                onPick: (candidate) => _pickMention(candidate, mention.start, caret),
              ),
          ]);
        },
      ),
      const SizedBox(height: 4),
      box,
      if (widget.below.isNotEmpty) _Below(children: widget.below),
    ]);
  }
}

/// What goes with the next message, as cards in a row above the text: an image's own picture, a file's icon and name. A note says
/// how it will reach the agent when that is not as it is (in the card, or for a picture in a line under the row), a refused one is
/// outlined in the look's danger colour, and each has a way to take it back.
class AttachmentChips extends StatelessWidget {
  const AttachmentChips({super.key, required this.chips, required this.onRemove});

  /// The height of a card, and the side of an image's.
  static const double cardSize = 72;

  final List<ComposerChip> chips;
  final ValueChanged<Object> onRemove;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    // A picture has no room for its note, so it is said under the row.
    final noted = [for (final chip in chips) if (chip.kind == ChipKind.image && chip.thumbnail != null && chip.note != null) chip];
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 4, 4, 6),
      child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
        Wrap(spacing: 8, runSpacing: 8, children: [for (final chip in chips) _AttachmentCard(chip: chip, onRemove: onRemove)]),
        for (final chip in noted)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: Text('${chip.name}: ${chip.note}', style: TextStyle(fontSize: 11, color: chip.refused ? look.danger : look.muted)),
          ),
      ]),
    );
  }
}

class _AttachmentCard extends StatelessWidget {
  const _AttachmentCard({required this.chip, required this.onRemove});

  final ComposerChip chip;
  final ValueChanged<Object> onRemove;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final error = look.danger;
    final side = chip.refused ? error : look.muted.withValues(alpha: 0.35);
    final picture = chip.kind == ChipKind.image && chip.thumbnail != null;
    final Widget body = picture
        ? Image.memory(
            Uint8List.fromList(chip.thumbnail!),
            width: AttachmentChips.cardSize,
            height: AttachmentChips.cardSize,
            fit: BoxFit.cover,
            gaplessPlayback: true,
            errorBuilder: (_, _, _) => SizedBox.square(dimension: AttachmentChips.cardSize, child: Icon(Icons.image_outlined, size: 28, color: look.muted)),
          )
        : Container(
            width: 148,
            height: AttachmentChips.cardSize,
            padding: const EdgeInsets.fromLTRB(10, 8, 22, 8),
            child: Column(mainAxisAlignment: MainAxisAlignment.spaceBetween, crossAxisAlignment: CrossAxisAlignment.start, children: [
              Icon(switch (chip.kind) {
                ChipKind.folder => Icons.folder_outlined,
                ChipKind.text => Icons.notes,
                ChipKind.image => Icons.image_outlined,
                _ => Icons.description_outlined,
              }, size: 20, color: chip.refused ? error : look.muted),
              Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(chip.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 12)),
                Text(
                  chip.note ?? chip.detail,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: chip.refused ? error : look.muted),
                ),
              ]),
            ]),
          );
    final card = Stack(key: ValueKey('attachment ${chip.name}'), children: [
      Container(
        decoration: BoxDecoration(border: Border.all(color: side), borderRadius: BorderRadius.circular(12)),
        clipBehavior: Clip.antiAlias,
        child: body,
      ),
      if (picture && chip.refused) Positioned(left: 4, bottom: 4, child: Icon(Icons.error_outline, size: 18, color: error)),
      Positioned(
        top: 0,
        right: 0,
        child: IconButton(
          tooltip: 'Remove ${chip.name}',
          visualDensity: VisualDensity.compact,
          padding: EdgeInsets.zero,
          constraints: const BoxConstraints.tightFor(width: 24, height: 24),
          iconSize: 14,
          onPressed: () => onRemove(chip.key),
          icon: const Icon(Icons.close),
        ),
      ),
    ]);
    final tip = picture ? [chip.name, chip.note ?? chip.detail].join('\n') : chip.note;
    return tip == null ? card : Tooltip(message: tip, child: card);
  }
}

/// The `+` at the box's left: the host's entries, then the files, the folder and paste.
class _PlusButton extends StatelessWidget {
  const _PlusButton({required this.composer});

  final ComposerState composer;

  @override
  Widget build(BuildContext context) {
    final widget = composer.widget;
    final host = widget.attachments;
    return MenuAnchor(
      menuChildren: [
        ...widget.plusItems,
        if (host != null) ...[
          MenuItemButton(
            key: const Key('add-files'),
            leadingIcon: const Icon(Icons.attach_file, size: 18),
            onPressed: widget.pickFiles == null || !host.canAttach ? null : () => unawaited(composer._addFiles()),
            child: const Text('Add files or images…'),
          ),
          MenuItemButton(
            key: const Key('add-folder'),
            leadingIcon: const Icon(Icons.folder_outlined, size: 18),
            onPressed: widget.pickFolder == null || !host.canAttach ? null : () => unawaited(composer._addFolder()),
            child: const Text('Add a folder reference…'),
          ),
          MenuItemButton(
            key: const Key('paste'),
            leadingIcon: const Icon(Icons.content_paste, size: 18),
            onPressed: host.canAttach ? () => unawaited(composer.paste()) : null,
            child: const Text('Paste from clipboard'),
          ),
        ],
      ],
      builder: (context, controller, _) => IconButton(
        key: const Key('composer-plus'),
        tooltip: 'Add to the message',
        visualDensity: VisualDensity.compact,
        onPressed: widget.enabled && ((host?.canAttach ?? false) || widget.plusItems.isNotEmpty)
            ? () => controller.isOpen ? controller.close() : controller.open()
            : null,
        icon: const Icon(Icons.add),
      ),
    );
  }
}

/// The box's right end. From the right: the host's primary control while the message is empty, then Send; while a turn runs, Stop
/// (always), and for a message typed Queue and Steer; a spinner while the conversation starts.
class _Trailing extends StatelessWidget {
  const _Trailing({required this.composer, required this.attached});

  final ComposerState composer;

  /// Whether anything is attached, which is a message to send though no text is typed.
  final bool attached;

  @override
  Widget build(BuildContext context) {
    final widget = composer.widget;
    return ValueListenableBuilder<TextEditingValue>(
      valueListenable: widget.controller,
      builder: (context, value, _) {
        final empty = value.text.trim().isEmpty && !attached;
        final steers = widget.working && widget.onSteer != null;
        final primary = widget.primary;
        final showPrimary = primary != null && (empty || widget.holdPrimary);
        final Widget? action;
        if (widget.starting) {
          action = const Padding(padding: EdgeInsets.all(8), child: SizedBox.square(dimension: 18, child: CircularProgressIndicator(strokeWidth: 2)));
        } else if (widget.working) {
          final waits = widget.canQueue || steers;
          action = Row(mainAxisSize: MainAxisSize.min, children: [
            if (steers && !empty)
              IconButton(
                key: const Key('composer-steer'),
                tooltip: widget.enterSteers ? 'Steer: stop the turn and send this first (Enter)' : 'Steer: stop the turn and send this first (Alt+Enter)',
                onPressed: widget.enabled ? widget.onSteer : null,
                icon: const Icon(Icons.bolt),
              ),
            if (waits && !empty)
              IconButton(
                key: const Key('composer-queue'),
                tooltip: !steers
                    ? 'Queue'
                    : widget.enterSteers
                    ? 'Queue: send this when the turn ends (Alt+Enter)'
                    : 'Queue: send this when the turn ends (Enter)',
                onPressed: widget.enabled ? widget.onSend : null,
                icon: const Icon(Icons.schedule_send_outlined),
              ),
            IconButton(key: const Key('composer-stop'), tooltip: 'Stop', onPressed: widget.onStop, icon: const Icon(Icons.stop_rounded)),
          ]);
        } else if (!empty || primary == null) {
          action = IconButton(
            key: const Key('composer-send'),
            tooltip: 'Send',
            onPressed: widget.enabled ? widget.onSend : null,
            icon: const Icon(Icons.keyboard_return),
          );
        } else {
          action = null;
        }
        return Row(mainAxisSize: MainAxisSize.min, children: [
          ?action,
          // Kept in the tree while it is put away, so what it holds (a dictation's attachment to the message) outlives the typing.
          if (primary != null) Visibility(visible: showPrimary, maintainState: true, child: KeyedSubtree(key: const Key('composer-primary'), child: primary)),
        ]);
      },
    );
  }
}

/// The host's choices under the box, as plain text: right-aligned when the composer is [Composer.wideFrom] wide, else from the
/// left, and on as many lines as they need.
class _Below extends StatelessWidget {
  const _Below({required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, box) => ComposerQuiet(
      child: Padding(
        key: const Key('composer-below'),
        padding: const EdgeInsets.fromLTRB(8, 2, 8, 0),
        child: Align(
          alignment: box.maxWidth >= Composer.wideFrom ? Alignment.centerRight : Alignment.centerLeft,
          child: Wrap(
            alignment: box.maxWidth >= Composer.wideFrom ? WrapAlignment.end : WrapAlignment.start,
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: 2,
            runSpacing: 0,
            children: children,
          ),
        ),
      ),
    ),
  );
}

/// The agent's commands, and the host's own actions, matching what is being typed. Nothing is sent until the person sends it; an
/// action runs when chosen.
class CommandSuggestions extends StatelessWidget {
  const CommandSuggestions({
    super.key,
    required this.commands,
    required this.typed,
    required this.onPick,
    this.actions = const [],
    this.onRun,
    this.commandsTitle = 'Agent commands',
    this.actionsTitle = 'Host actions',
    this.limit = shown,
    this.highlighted,
  });

  /// The most each group lists, and the row Enter would pick.
  final int limit;
  final int? highlighted;

  /// The most each group lists.
  static const int shown = 6;

  final List<AgentCommandInfo> commands;
  final List<ComposerAction> actions;
  final String commandsTitle, actionsTitle;
  final String typed;
  final void Function(AgentCommandInfo command) onPick;
  final void Function(ComposerAction action)? onRun;

  @override
  Widget build(BuildContext context) {
    if (!typed.startsWith('/') || typed.contains(' ')) return const SizedBox.shrink();
    final (commands: found, actions: foundActions) = suggestionsFor(typed, commands, actions, limit: limit);
    if (found.isEmpty && foundActions.isEmpty) return const SizedBox.shrink();
    final look = context.chatLook;
    final titled = actions.isNotEmpty;
    Widget title(String text, Key key) => Padding(
      key: key,
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
      child: Align(
        alignment: Alignment.centerLeft,
        child: Text(text, style: TextStyle(fontSize: 11.5, fontWeight: FontWeight.w600, color: look.muted)),
      ),
    );
    final selected = look.accent.withValues(alpha: 0.14);
    return Material(
      color: look.composer,
      borderRadius: BorderRadius.circular(look.radius / 2),
      clipBehavior: Clip.antiAlias,
      child: ConstrainedBox(
        // The list is as tall as its rows, up to most of the window, and scrolls past that.
        constraints: BoxConstraints(maxHeight: MediaQuery.sizeOf(context).height * 0.65),
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (titled && found.isNotEmpty) title(commandsTitle, const Key('commands-group')),
              for (final (index, command) in found.indexed)
                ListTile(
                  key: ValueKey('command ${command.name}'),
                  dense: true,
                  visualDensity: VisualDensity.compact,
                  tileColor: highlighted == index ? selected : null,
                  title: Text.rich(
                    TextSpan(children: [
                      TextSpan(text: '/${command.name}', style: TextStyle(fontFamily: look.codeFont)),
                      if (command.hint != null) TextSpan(text: ' ${command.hint}', style: TextStyle(color: look.muted)),
                    ]),
                  ),
                  subtitle: command.description.isEmpty ? null : Text(command.description, maxLines: 2, overflow: TextOverflow.ellipsis),
                  onTap: () => onPick(command),
                ),
              for (final (index, action) in foundActions.indexed) ...[
                if (index == 0 || (action.group ?? actionsTitle) != (foundActions[index - 1].group ?? actionsTitle))
                  title(action.group ?? actionsTitle, action.group == null ? const Key('actions-group') : Key('group ${action.group}')),
                ListTile(
                  key: ValueKey(action.group == null ? 'action ${action.name}' : 'action ${action.group} ${action.name}'),
                  dense: true,
                  visualDensity: VisualDensity.compact,
                  tileColor: highlighted == found.length + index ? selected : null,
                  title: Text('/${action.name}', style: TextStyle(fontFamily: look.codeFont)),
                  subtitle: action.description.isEmpty ? null : Text(action.description, maxLines: 2, overflow: TextOverflow.ellipsis),
                  onTap: () => onRun?.call(action),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// What `/` offers: commands and actions, in the order the list shows them.
typedef Suggested = ({List<AgentCommandInfo> commands, List<ComposerAction> actions});

/// The commands and actions [typed] asks for, at most [limit] of each: the names that start with the word after the `/` first,
/// then those whose name or description holds it, without regard to case. Nothing once the text is past its first word.
Suggested suggestionsFor(String typed, List<AgentCommandInfo> commands, List<ComposerAction> actions, {int limit = CommandSuggestions.shown}) {
  if (!typed.startsWith('/') || typed.contains(RegExp(r'\s'))) return (commands: const [], actions: const []);
  final wanted = typed.substring(1).toLowerCase();
  List<T> pick<T>(List<T> all, String Function(T) name, String Function(T) description) {
    final starts = <T>[], holds = <T>[];
    for (final item in all) {
      final text = name(item).toLowerCase();
      if (text.startsWith(wanted)) {
        starts.add(item);
      } else if (text.contains(wanted) || description(item).toLowerCase().contains(wanted)) {
        holds.add(item);
      }
    }
    return [...starts, ...holds].take(limit).toList();
  }

  // A group's entries stay together, the groups in the order the host listed them.
  final groups = <String?>[];
  for (final action in actions) {
    if (!groups.contains(action.group)) groups.add(action.group);
  }
  final found = pick(actions, (action) => action.name, (action) => action.description);
  return (
    commands: pick(commands, (command) => command.name, (command) => command.description),
    actions: [
      for (final group in groups) ...found.where((action) => action.group == group),
    ],
  );
}

/// What the host's [ComposerMentions] offers for the `@` being typed: the files and folders matching [query], each with where it
/// is. Choosing one goes to [onPick]. A search that is still out when the query changes is ignored.
class MentionSuggestions extends StatefulWidget {
  const MentionSuggestions({super.key, required this.source, required this.query, required this.onPick});

  /// The most the list shows.
  static const int shown = 8;

  final ComposerMentions source;
  final String query;
  final void Function(MentionCandidate candidate) onPick;

  @override
  State<MentionSuggestions> createState() => _MentionSuggestionsState();
}

class _MentionSuggestionsState extends State<MentionSuggestions> {
  List<MentionCandidate> _found = const [];
  var _asked = 0;

  @override
  void initState() {
    super.initState();
    _search();
  }

  @override
  void didUpdateWidget(MentionSuggestions old) {
    super.didUpdateWidget(old);
    if (old.query != widget.query || old.source != widget.source) _search();
  }

  Future<void> _search() async {
    final mine = ++_asked;
    List<MentionCandidate> found;
    try {
      found = await widget.source.search(widget.query);
    } on Object {
      found = const [];
    }
    if (!mounted || mine != _asked) return;
    setState(() => _found = found.take(MentionSuggestions.shown).toList());
  }

  @override
  Widget build(BuildContext context) {
    if (_found.isEmpty) return const SizedBox.shrink();
    final look = context.chatLook;
    return Material(
      color: look.composer,
      borderRadius: BorderRadius.circular(look.radius / 2),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final (index, candidate) in _found.indexed) ...[
            if (candidate.group != null && (index == 0 || _found[index - 1].group != candidate.group))
              Padding(
                key: Key('mention-group ${candidate.group}'),
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: Text(candidate.group!, style: TextStyle(fontSize: 11.5, fontWeight: FontWeight.w600, color: look.muted)),
                ),
              ),
            ListTile(
              key: ValueKey(candidate.group == null ? 'mention ${candidate.label}' : 'mention ${candidate.group} ${candidate.label}'),
              dense: true,
              visualDensity: VisualDensity.compact,
              leading: Icon(candidate.icon ?? (candidate.folder ? Icons.folder_outlined : Icons.description_outlined), size: 18, color: look.muted),
              title: Text(candidate.label, maxLines: 1, overflow: TextOverflow.ellipsis, style: TextStyle(fontFamily: look.codeFont)),
              subtitle: candidate.detail == null ? null : Text(candidate.detail!, maxLines: 1, overflow: TextOverflow.ellipsis),
              onTap: () => widget.onPick(candidate),
            ),
          ],
        ],
      ),
    );
  }
}

/// How much of the account's usage allowance is used: the bucket that matters most with its refill, and a menu of every bucket the
/// agent reported. In the look's `warning` from the configured fraction used (a status), and in its `danger` once a bucket is
/// exhausted. The host decides whether to show it at all: an agent that reports no usage has no such control.
class UsagePill extends StatelessWidget {
  const UsagePill({super.key, required this.report, this.warnPermille = defaultWarnPermille, this.now});

  final UsageReport? report;
  final int warnPermille;

  /// Unix seconds; the clock's when null.
  final int? now;

  @override
  Widget build(BuildContext context) {
    final at = now ?? DateTime.now().millisecondsSinceEpoch ~/ 1000;
    final report = this.report;
    final headline = report == null ? null : headlineOf(report, at);
    final level = headline == null ? UsageLevel.calm : levelOf(headline, at, warnPermille: warnPermille);
    final text = report == null ? 'No usage yet' : pillText(report, at);
    return MenuAnchor(
      menuChildren: [UsageMenu(report: report, now: at)],
      builder: (context, controller, _) => InkWell(
        key: const Key('usage'),
        borderRadius: BorderRadius.circular(8),
        onTap: () => controller.isOpen ? controller.close() : controller.open(),
        child: Tooltip(
          message: 'Usage of the account behind this agent',
          child: ComposerPill(
            label: text,
            icon: Icons.data_usage,
            danger: level == UsageLevel.danger,
            status: level == UsageLevel.warn,
            maxWidth: 320,
          ),
        ),
      ),
    );
  }
}

/// What the usage control's menu lists: every bucket, the most pressing first, with what is known of it.
class UsageMenu extends StatelessWidget {
  const UsageMenu({super.key, required this.report, required this.now});

  final UsageReport? report;

  /// Unix seconds.
  final int now;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final report = this.report;
    final small = TextStyle(fontSize: 11.5, color: look.muted);
    final buckets = report == null ? const <UsageBucket>[] : ordered(report, now);
    final hit = report?.limit;
    return SelectionArea(
      child: ConstrainedBox(
        constraints: const BoxConstraints(minWidth: 240, maxWidth: 380),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (buckets.isEmpty) Text('Nothing reported yet', key: const Key('usage-empty'), style: small),
              for (final bucket in buckets)
                Padding(
                  key: Key('usage-bucket ${bucket.id}'),
                  padding: const EdgeInsets.only(bottom: 8),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text(
                      bucketLine(bucket, now),
                      style: TextStyle(
                        fontSize: 13,
                        color: switch (levelOf(bucket, now)) {
                          UsageLevel.danger => look.danger,
                          UsageLevel.warn => look.warning,
                          UsageLevel.calm => null,
                        },
                      ),
                    ),
                    for (final line in bucketDetails(bucket, now)) Text(line, style: small),
                  ]),
                ),
              if (hit != null)
                Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: Text(
                    [
                      'Last refusal',
                      if (hit.text != null) hit.text!,
                      if (hit.retryAfter != null) 'retry after ${spanText(hit.retryAfter!.inSeconds)}',
                      if (hit.resetsAt != null && hit.resetsAt! > now) 'back in ${spanText(hit.resetsAt! - now)}',
                    ].join(' · '),
                    style: small,
                  ),
                ),
              if (report?.account case final account?) Text('Account: $account', style: small),
              if (report != null && report.sources.isNotEmpty && !report.sources.contains('reported'))
                Text('The agent reports no figures: these are inferred from its errors', style: small),
            ],
          ),
        ),
      ),
    );
  }
}
