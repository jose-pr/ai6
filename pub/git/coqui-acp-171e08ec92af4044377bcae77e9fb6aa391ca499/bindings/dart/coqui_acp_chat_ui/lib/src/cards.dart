import 'package:coqui_acp/coqui_acp.dart';
import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';

import 'diff.dart';
import 'look.dart';
import 'tools.dart' show DiffView;

const ChatLimits _limits = ChatLimits();

/// [value] as one short line for a person, or null: what an agent called something, with nothing in it that could hide the rest.
String? shortLine(Object? value) => shortText(value, _limits);

/// The verbs a tool's title may start with, which the question keeps: "Edit calc.js" asks "Allow Claude to edit calc.js?".
const Set<String> _verbs = {
  'edit', 'write', 'read', 'run', 'delete', 'remove', 'create', 'fetch', 'search', 'move', 'rename', 'copy', 'open', 'list',
  'update', 'install', 'execute', 'call', 'use', 'access', 'change', 'modify', 'patch', 'download', 'upload', 'send', 'view',
};

/// The question a permission card asks: the tool's own title as a request when it opens with a verb, else the tool's name.
String permissionQuestion(String agent, String title) {
  final text = title.trim();
  final word = text.split(RegExp(r'\s+')).first.toLowerCase();
  if (text.isNotEmpty && _verbs.contains(word)) return 'Allow $agent to ${text[0].toLowerCase()}${text.substring(1)}?';
  return text.isEmpty ? 'Allow $agent to use a tool?' : 'Allow $agent to use $text?';
}

/// The words for an option the agent sent without a name.
String _kindLabel(AcpPermissionOption option) => switch (option.kind) {
  'allow_once' => 'Allow once',
  'allow_always' => 'Allow always',
  'reject_once' => 'Deny',
  'reject_always' => 'Never allow',
  _ => shortLine(option.kind) ?? shortLine(option.optionId) ?? 'An answer',
};

/// An answer under the agent's own name for it.
String _answerName(AcpPermissionOption option) => shortLine(option.name) ?? _kindLabel(option);

/// What an agent is waiting on, laid out as the person decides it: a chip saying it needs them, the question ("Allow Claude to edit
/// calc.js?") with the agent's [reason] under it, what it would do inline at the card's width (an edit's [diff], else [detail]), a
/// sentence on what an "always" answer does, then Deny beside a split primary button. The primary is the agent's "always" where it
/// offers one, else its allow-once; the other answers the agent offers are in its menu. It stays until the person answers.
///
/// A host adds to it without a fork: [notes] are lines under the question (what a remembered rule would do), [extraAnswers] are
/// buttons beside the answers (an "Always allow here" that the host answers itself), [answerHints] the keys that answer for it.
class PermissionCard extends StatelessWidget {
  const PermissionCard({
    super.key,
    required this.agent,
    required this.title,
    required this.options,
    required this.onChoose,
    this.reason,
    this.detail,
    this.diff,
    this.bypassPassed = false,
    this.forPerson = false,
    this.more = 0,
    this.onCancel,
    this.notes = const [],
    this.extraAnswers = const [],
    this.answerHints = const {},
    this.host = 'This app',
    this.rememberedBy,
  });

  /// Narrower than this the key hints stay out: they belong to a keyboard.
  static const double hintsFrom = 560;

  /// The most height what the tool would do takes before it scrolls.
  static const double shown = 260;

  /// The agent's name, as it gave it.
  final String agent;

  /// What the tool is called: the question is made from it.
  final String title;

  /// Why the agent says it needs this, when it said.
  final String? reason;

  /// What it would run or touch, when the agent said, as text.
  final String? detail;

  /// The change an edit would make, shown instead of [detail] when there is one.
  final ToolDiff? diff;
  final List<AcpPermissionOption> options;

  /// Whether Bypass was on and left this to the person, and whether it is a question or a plan, which Bypass never answers.
  final bool bypassPassed, forPerson;

  /// How many requests wait after this one.
  final int more;
  final ValueChanged<AcpPermissionOption> onChoose;

  /// Answers the request cancelled, offered when the agent gave no answer to choose.
  final VoidCallback? onCancel;
  final List<Widget> notes;
  final List<Widget> extraAnswers;

  /// The key that answers an option of a kind, by kind (`reject_once`, `allow_once`), as the host writes it ("Ctrl+Shift+R"); shown
  /// on a card wide enough for a keyboard. The host binds the keys: the card only says them.
  final Map<String, String> answerHints;

  /// What the chat calls itself in the card's sentences: "Notes cannot see or undo what [agent] keeps."
  final String host;

  /// Overrides the sentence about an "always" answer, which names [host]; null for the package's own.
  final String? rememberedBy;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final small = theme.textTheme.labelSmall;
    final look = context.chatLook;
    final name = shortLine(agent) ?? 'The agent';
    final deny = options.where((option) => option.kind == 'reject_once').firstOrNull;
    final always = options.where((option) => option.kind == 'allow_always').firstOrNull;
    final once = options.where((option) => option.kind == 'allow_once').firstOrNull;
    final primary = always ?? once;
    // One-time first, then what the card does not know, then the refusals for good.
    int rank(AcpPermissionOption option) => switch (option.kind) { 'allow_once' => 0, 'reject_always' => 2, _ => 1 };
    final menu = [
      for (final option in options)
        if (option != deny && option != primary) option,
    ]..sort((a, b) => rank(a) - rank(b));
    final remembers = options.any((option) => option.kind.endsWith('_always'));
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(bottom: 6),
            child: Container(
              key: const Key('needs-input'),
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 3),
              decoration: BoxDecoration(color: look.accent.withValues(alpha: 0.16), borderRadius: BorderRadius.circular(99)),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                Icon(Icons.circle, size: 8, color: look.accent),
                const SizedBox(width: 6),
                Text('Needs your input', style: theme.textTheme.labelMedium?.copyWith(color: look.accent, fontWeight: FontWeight.w600)),
              ]),
            ),
          ),
          Card(
            key: const Key('permission-card'),
            margin: EdgeInsets.zero,
            child: Padding(
              padding: const EdgeInsets.all(14),
              child: LayoutBuilder(builder: (context, constraints) {
                final hints = constraints.maxWidth >= hintsFrom;
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Text(permissionQuestion(name, title), style: theme.textTheme.titleMedium),
                    if (reason?.isNotEmpty ?? false)
                      Padding(
                        padding: const EdgeInsets.only(top: 2),
                        child: Text(reason!, style: theme.textTheme.bodySmall?.copyWith(color: look.muted)),
                      ),
                    if (diff != null || (detail?.isNotEmpty ?? false))
                      Padding(padding: const EdgeInsets.only(top: 10), child: _ToApprove(diff: diff, detail: detail)),
                    if (remembers)
                      Padding(
                        padding: const EdgeInsets.only(top: 10),
                        child: Text(
                          rememberedBy ?? 'An "always" answer is remembered by $name, not by $host: $host cannot see or undo what it keeps.',
                          style: small,
                        ),
                      ),
                    for (final note in notes) Padding(padding: const EdgeInsets.only(top: 4), child: note),
                    if (bypassPassed)
                      Padding(
                        padding: const EdgeInsets.only(top: 4),
                        child: Text(
                          forPerson
                              ? 'Bypass leaves a question or a plan to you, so this one is yours to answer.'
                              : 'Bypass answers only a request that offers to allow, so this one is yours to answer.',
                          style: small?.copyWith(fontWeight: FontWeight.w600),
                        ),
                      ),
                    if (options.isEmpty)
                      Padding(padding: const EdgeInsets.only(top: 4), child: Text('The agent offered no answer $host can give.', style: small)),
                    if (more > 0) Padding(padding: const EdgeInsets.only(top: 4), child: Text('$more more after this', style: small)),
                    const SizedBox(height: 12),
                    Wrap(
                      spacing: 8,
                      runSpacing: 8,
                      alignment: WrapAlignment.end,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        if (deny != null)
                          TextButton(
                            key: const Key('permission-deny'),
                            onPressed: () => onChoose(deny),
                            child: Row(mainAxisSize: MainAxisSize.min, children: [
                              const Text('Deny'),
                              if (hints && answerHints['reject_once'] != null) ...[const SizedBox(width: 8), _Hint(answerHints['reject_once']!)],
                            ]),
                          ),
                        ...extraAnswers,
                        if (primary != null)
                          _Split(primary: primary, menu: menu, hints: hints ? answerHints : const {}, onChoose: onChoose)
                        else
                          for (final option in menu)
                            OutlinedButton(
                              key: Key('answer-${option.kind}'),
                              onPressed: () => onChoose(option),
                              child: Text(_answerName(option), maxLines: 2, overflow: TextOverflow.ellipsis),
                            ),
                        if (options.isEmpty && onCancel != null) TextButton(onPressed: onCancel, child: const Text('Cancel the request')),
                      ],
                    ),
                  ],
                );
              }),
            ),
          ),
        ],
      ),
    );
  }
}

/// A key hint, as a small cap beside what it answers.
class _Hint extends StatelessWidget {
  const _Hint(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
      decoration: BoxDecoration(border: Border.all(color: look.border), borderRadius: BorderRadius.circular(4)),
      child: Text(text, style: Theme.of(context).textTheme.labelSmall?.copyWith(color: look.muted)),
    );
  }
}

/// The primary answer, with the others in a menu beside it.
class _Split extends StatelessWidget {
  const _Split({required this.primary, required this.menu, required this.hints, required this.onChoose});

  final AcpPermissionOption primary;
  final List<AcpPermissionOption> menu;
  final Map<String, String> hints;
  final ValueChanged<AcpPermissionOption> onChoose;

  @override
  Widget build(BuildContext context) {
    final onePiece = menu.isEmpty;
    const round = Radius.circular(20);
    final left = RoundedRectangleBorder(
      borderRadius: onePiece ? BorderRadius.circular(20) : const BorderRadius.horizontal(left: round, right: Radius.circular(4)),
    );
    final look = context.chatLook;
    return Row(mainAxisSize: MainAxisSize.min, children: [
      Flexible(
        child: FilledButton(
          key: const Key('permission-primary'),
          style: FilledButton.styleFrom(shape: left),
          onPressed: () => onChoose(primary),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            Flexible(child: Text(_answerName(primary), maxLines: 2, overflow: TextOverflow.ellipsis)),
            if (hints[primary.kind] != null) ...[const SizedBox(width: 8), _Hint(hints[primary.kind]!)],
          ]),
        ),
      ),
      if (!onePiece) ...[
        const SizedBox(width: 2),
        MenuAnchor(
          menuChildren: [
            for (final option in menu)
              MenuItemButton(
                key: Key('answer-${option.kind}'),
                onPressed: () => onChoose(option),
                trailingIcon: hints[option.kind] == null
                    ? null
                    : Padding(padding: const EdgeInsets.only(left: 12), child: Text(hints[option.kind]!, style: TextStyle(color: look.muted, fontSize: 12))),
                child: Text(_answerName(option)),
              ),
          ],
          builder: (context, controller, _) => FilledButton(
            key: const Key('permission-more'),
            style: FilledButton.styleFrom(
              minimumSize: const Size(40, 40),
              padding: EdgeInsets.zero,
              shape: const RoundedRectangleBorder(borderRadius: BorderRadius.horizontal(left: Radius.circular(4), right: round)),
            ),
            onPressed: () => controller.isOpen ? controller.close() : controller.open(),
            child: const Icon(Icons.arrow_drop_down),
          ),
        ),
      ],
    ]);
  }
}

/// What a tool would do, across the card: an edit as a diff, else the command or input as text. It scrolls past [PermissionCard.shown],
/// and a text that does not fit says so, measured at the width it has, so a short one spread over a few lines is not called long.
class _ToApprove extends StatelessWidget {
  const _ToApprove({this.diff, this.detail});

  final ToolDiff? diff;
  final String? detail;

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final mono = TextStyle(fontFamily: look.codeFont, fontSize: 12.5);
    final change = diff;
    return LayoutBuilder(builder: (context, constraints) {
      final Widget body;
      var long = false;
      if (change != null) {
        body = Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          if (change.path.isNotEmpty)
            Padding(padding: const EdgeInsets.only(bottom: 4), child: SelectableText(change.path, style: mono.copyWith(color: look.muted))),
          SizedBox(width: double.infinity, child: DiffView(lines: withContext(diffLines(change.oldText, change.newText)), path: change.path.isEmpty ? null : change.path)),
        ]);
      } else {
        body = SelectableText(detail!, style: mono);
        final painter = TextPainter(
          text: TextSpan(text: detail, style: DefaultTextStyle.of(context).style.merge(mono)),
          textDirection: Directionality.of(context),
          textScaler: MediaQuery.textScalerOf(context),
        )..layout(maxWidth: constraints.maxWidth - 22);
        long = painter.height > PermissionCard.shown - 22;
        painter.dispose();
      }
      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Container(
          key: const Key('permission-detail'),
          width: double.infinity,
          decoration: BoxDecoration(color: look.code, borderRadius: BorderRadius.circular(8), border: Border.all(color: look.border)),
          constraints: const BoxConstraints(maxHeight: PermissionCard.shown),
          padding: const EdgeInsets.all(10),
          child: SingleChildScrollView(child: body),
        ),
        if (long)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              'This is long. Scroll to read all of it before you answer.',
              style: Theme.of(context).textTheme.labelSmall?.copyWith(fontWeight: FontWeight.w600),
            ),
          ),
      ]);
    });
  }
}

/// Says, for as long as it lasts, that a conversation is in Bypass, with the one action that ends it. It is a status, in the
/// colours of the mode's pill, not an alarm.
class BypassBanner extends StatelessWidget {
  const BypassBanner({super.key, required this.onLeave});

  final VoidCallback onLeave;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Material(
      key: const Key('bypass-banner'),
      color: scheme.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
        child: Row(children: [
          Icon(Icons.fast_forward_outlined, color: scheme.onTertiaryContainer),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              'Bypass: each request that offers to allow is allowed without asking you.',
              style: TextStyle(color: scheme.onTertiaryContainer),
            ),
          ),
          TextButton(
            onPressed: onLeave,
            style: TextButton.styleFrom(foregroundColor: scheme.onTertiaryContainer),
            child: const Text('Leave Bypass'),
          ),
        ]),
      ),
    );
  }
}

/// The most of an agent's question shown, since it can send any amount.
const int maxQuestionText = 4000;

final _hidden = RegExp(r'[\u0000-\u0008\u000b-\u001f\u007f-\u009f\u200e\u200f\u2028-\u202e\u2066-\u2069]');

/// An agent's text as plain data: line breaks kept, nothing that could reorder or hide what surrounds it.
String plainText(String text) {
  final clean = text.replaceAll('\r\n', '\n').replaceAll(_hidden, ' ');
  return clean.length > maxQuestionText ? '${clean.substring(0, maxQuestionText)}\n…' : clean;
}

/// An agent's question, as a form built from its schema. The person sends an answer, declines, or cancels; everything the agent
/// wrote is shown as text, never as markup or a link.
class QuestionCard extends StatefulWidget {
  const QuestionCard({
    super.key,
    required this.request,
    required this.agent,
    required this.onAnswer,
    this.more = 0,
    this.host = 'This app',
  });

  /// The question; a new one (not the same object) starts a new form.
  final AcpElicitation request;
  final String agent;
  final int more;
  final ValueChanged<AcpElicitationAnswer> onAnswer;
  final String host;

  @override
  State<QuestionCard> createState() => _QuestionCardState();
}

class _QuestionCardState extends State<QuestionCard> {
  /// What the person gave each field so far, by name; a field with no value is left out of the answer.
  final Map<String, Object> _values = {};
  final Map<String, TextEditingController> _texts = {};

  @override
  void initState() {
    super.initState();
    _start();
  }

  @override
  void didUpdateWidget(QuestionCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(widget.request, oldWidget.request)) {
      _clear();
      _start();
    }
  }

  /// The form of the question, at the values the agent gave as defaults.
  void _start() {
    for (final field in widget.request.fields) {
      final start = switch (field) {
        AcpTextField(:final defaultValue) => defaultValue,
        AcpChoiceField(:final defaultValue) => defaultValue,
        AcpChoicesField(:final defaultValue) => defaultValue,
        AcpNumberField(:final defaultValue) => defaultValue,
        AcpBooleanField(:final defaultValue) => defaultValue,
        AcpUnsupportedField() => null,
      };
      if (start != null) _values[field.name] = start;
      if (field is AcpTextField || field is AcpNumberField) {
        _texts[field.name] = TextEditingController(text: start == null ? '' : '$start');
      }
    }
  }

  void _clear() {
    for (final controller in _texts.values) {
      controller.dispose();
    }
    _texts.clear();
    _values.clear();
  }

  @override
  void dispose() {
    _clear();
    super.dispose();
  }

  /// Why the answer cannot be sent yet, or null when it can.
  String? get _problem {
    for (final field in widget.request.fields) {
      final value = _values[field.name];
      final label = shortLine(field.title) ?? shortLine(field.name) ?? 'A question';
      if (value == null) {
        if (field.required) {
          return field is AcpUnsupportedField ? '$label cannot be answered here, so only Decline or Cancel can answer this.' : '$label is required.';
        }
        continue;
      }
      switch (field) {
        case AcpTextField(:final minLength, :final maxLength):
          final length = (value as String).runes.length;
          if (minLength != null && length < minLength) return '$label needs at least $minLength characters.';
          if (maxLength != null && length > maxLength) return '$label takes at most $maxLength characters.';
        case AcpChoicesField(:final minItems, :final maxItems):
          final count = (value as List).length;
          if (minItems != null && count < minItems) return 'Choose at least $minItems for $label.';
          if (maxItems != null && count > maxItems) return 'Choose at most $maxItems for $label.';
        case AcpNumberField(:final minimum, :final maximum):
          if (minimum != null && (value as num) < minimum) return '$label is at least $minimum.';
          if (maximum != null && (value as num) > maximum) return '$label is at most $maximum.';
        default:
          break;
      }
    }
    return null;
  }

  Widget _field(AcpField field) {
    final title = shortLine(field.title) ?? shortLine(field.name) ?? 'A question';
    final description = field.description == null ? null : plainText(field.description!);
    final label = '$title${field.required ? ' *' : ''}';
    void set(Object? value) => setState(() => value == null ? _values.remove(field.name) : _values[field.name] = value);
    final key = ValueKey('question-${field.name}');
    switch (field) {
      case AcpTextField(:final format):
        return TextField(
          key: key,
          controller: _texts[field.name],
          decoration: InputDecoration(labelText: label, helperText: [?description, ?shortLine(format)].join(' · ').nullIfEmpty),
          onChanged: (text) => set(text.isEmpty ? null : text),
        );
      case AcpNumberField(:final integer):
        return TextField(
          key: key,
          controller: _texts[field.name],
          keyboardType: TextInputType.numberWithOptions(decimal: !integer, signed: true),
          inputFormatters: [FilteringTextInputFormatter.allow(RegExp(integer ? r'[-0-9]' : r'[-0-9.eE+]'))],
          decoration: InputDecoration(labelText: label, helperText: description),
          onChanged: (text) => set(integer ? int.tryParse(text) : num.tryParse(text)),
        );
      case AcpBooleanField():
        return SwitchListTile(
          key: key,
          contentPadding: EdgeInsets.zero,
          title: Text(label),
          subtitle: description == null ? null : Text(description),
          value: _values[field.name] == true,
          onChanged: set,
        );
      case AcpChoiceField(:final options):
        return _Labelled(label: label, description: description, child: Wrap(spacing: 6, runSpacing: 4, children: [
          for (final option in options)
            _Described(
              description: option.description,
              child: ChoiceChip(
                key: ValueKey('question-${field.name}-${option.value}'),
                label: Text(shortLine(option.title) ?? shortLine(option.value) ?? '?'),
                selected: _values[field.name] == option.value,
                onSelected: (on) => set(on ? option.value : null),
              ),
            ),
        ]));
      case AcpChoicesField(:final options):
        final chosen = [...(_values[field.name] as List<String>?) ?? const <String>[]];
        return _Labelled(label: label, description: description, child: Wrap(spacing: 6, runSpacing: 4, children: [
          for (final option in options)
            _Described(
              description: option.description,
              child: FilterChip(
                key: ValueKey('question-${field.name}-${option.value}'),
                label: Text(shortLine(option.title) ?? shortLine(option.value) ?? '?'),
                selected: chosen.contains(option.value),
                onSelected: (on) {
                  // What is chosen now, which a tap since this was built may have changed.
                  final chosen = [...(_values[field.name] as List<String>?) ?? const <String>[]];
                  on ? chosen.add(option.value) : chosen.remove(option.value);
                  // The answer keeps the agent's order.
                  final ordered = [for (final each in options) if (chosen.contains(each.value)) each.value];
                  set(ordered.isEmpty ? null : ordered);
                },
              ),
            ),
        ]));
      case AcpUnsupportedField(:final type):
        return _Labelled(
          label: label,
          description: description,
          child: Text('A question of a kind ${widget.host} cannot show (${shortLine(type) ?? 'no type'}), so it is left unanswered.',
              style: Theme.of(context).textTheme.labelSmall),
        );
    }
  }

  @override
  Widget build(BuildContext context) {
    final request = widget.request;
    final problem = _problem;
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
      child: Card(
        key: const Key('question-card'),
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: MediaQuery.sizeOf(context).height * 0.55),
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(12),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              Row(children: [
                const Icon(Icons.help_outline),
                const SizedBox(width: 12),
                Expanded(child: Text('${shortLine(widget.agent) ?? 'The agent'} asks', style: theme.textTheme.labelMedium)),
              ]),
              if (request.title != null) Text(shortLine(request.title) ?? '', style: theme.textTheme.titleSmall),
              SelectableText(plainText(request.message)),
              if (request.description != null) Text(plainText(request.description!), style: theme.textTheme.bodySmall),
              for (final field in request.fields) Padding(padding: const EdgeInsets.only(top: 8), child: _field(field)),
              if (problem != null)
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Text(problem, style: theme.textTheme.labelSmall?.copyWith(fontWeight: FontWeight.w600)),
                ),
              if (widget.more > 0) Text('${widget.more} more after this', style: theme.textTheme.labelSmall),
              const SizedBox(height: 8),
              Wrap(alignment: WrapAlignment.end, spacing: 8, children: [
                TextButton(onPressed: () => widget.onAnswer(const AcpElicitCancel()), child: const Text('Cancel')),
                TextButton(onPressed: () => widget.onAnswer(const AcpElicitDecline()), child: const Text('Decline')),
                FilledButton(
                  onPressed: problem == null ? () => widget.onAnswer(AcpElicitAccept(Map.of(_values))) : null,
                  child: const Text('Send answer'),
                ),
              ]),
            ]),
          ),
        ),
      ),
    );
  }
}

/// A choice, with the agent's description of it as a tooltip when it gave one.
class _Described extends StatelessWidget {
  const _Described({required this.description, required this.child});

  final String? description;
  final Widget child;

  @override
  Widget build(BuildContext context) => description == null ? child : Tooltip(message: plainText(description!), child: child);
}

class _Labelled extends StatelessWidget {
  const _Labelled({required this.label, required this.child, this.description});

  final String label;
  final String? description;
  final Widget child;

  @override
  Widget build(BuildContext context) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
    Text(label, style: Theme.of(context).textTheme.labelLarge),
    if (description != null) Text(description!, style: Theme.of(context).textTheme.bodySmall),
    const SizedBox(height: 4),
    child,
  ]);
}

extension on String {
  String? get nullIfEmpty => isEmpty ? null : this;
}

/// One message waiting for its turn, as the queue shows it.
class QueuedLine {
  const QueuedLine(this.key, this.text, {this.attachments = const []});

  /// Names the line across rebuilds.
  final Object key;
  final String text;

  /// The names of what goes with it.
  final List<String> attachments;
}

/// The messages written while the agent worked, in the order they go, each editable and removable. While the queue is held,
/// after a turn that was stopped or failed, it says so and waits for the person to send the next or clear them.
///
/// A row's menu offers what the host gives a callback for: Steer now ([onSteer]), Merge with above ([onMerge]), Move to composer
/// ([onToComposer]), and Move up and down ([onMove]). The header offers Merge all ([onMergeAll]) and, held, Merge & send
/// ([onMergeSend]).
class QueuePanel extends StatelessWidget {
  const QueuePanel({
    super.key,
    required this.lines,
    required this.onEdit,
    required this.onRemove,
    required this.onClear,
    this.held,
    this.working = false,
    this.merged = false,
    this.onSendNext,
    this.onMergeSend,
    this.onMergeAll,
    this.onSteer,
    this.onMerge,
    this.onToComposer,
    this.onMove,
    this.dialog,
  });

  final List<QueuedLine> lines;

  /// Whether what waits goes as one message when the turn ends, rather than one message a turn.
  final bool merged;

  /// Sends everything that waits as one message, from a held queue.
  final VoidCallback? onMergeSend;

  /// Joins every message into the one before it.
  final VoidCallback? onMergeAll;

  /// Stops the turn and sends the message at an index first.
  final ValueChanged<int>? onSteer;

  /// Joins the message at an index to the one above it.
  final ValueChanged<int>? onMerge;

  /// Takes the message at an index out of the queue and into the composer.
  final ValueChanged<int>? onToComposer;

  /// Moves the message at an index by -1 (up) or 1 (down).
  final void Function(int index, int by)? onMove;

  /// Why the queue waits for the person rather than going on; null while it goes on.
  final String? held;
  final bool working;
  final void Function(int index, String text) onEdit;
  final ValueChanged<int> onRemove;
  final VoidCallback onClear;

  /// Sends the next message of a held queue.
  final VoidCallback? onSendNext;

  /// Shows a dialog; [showDialog] when null. A host whose dialogs are all selectable passes its own.
  final Future<String?> Function(BuildContext context, WidgetBuilder builder)? dialog;

  Future<void> _edit(BuildContext context, int index) async {
    final show = dialog ?? (context, builder) => showDialog<String>(context: context, builder: builder);
    final text = await show(context, (_) => _EditQueued(text: lines[index].text));
    if (text != null) onEdit(index, text);
  }

  Widget _rowMenu(int index) => MenuAnchor(
    menuChildren: [
      if (onSteer != null)
        MenuItemButton(
          key: const Key('queue-steer'),
          leadingIcon: const Icon(Icons.bolt, size: 18),
          onPressed: () => onSteer!(index),
          child: const Text('Steer now'),
        ),
      if (onMerge != null && index > 0)
        MenuItemButton(
          key: const Key('queue-merge'),
          leadingIcon: const Icon(Icons.call_merge, size: 18),
          onPressed: () => onMerge!(index),
          child: const Text('Merge with above'),
        ),
      if (onToComposer != null)
        MenuItemButton(
          key: const Key('queue-to-composer'),
          leadingIcon: const Icon(Icons.edit_note, size: 18),
          onPressed: () => onToComposer!(index),
          child: const Text('Move to composer'),
        ),
      if (onMove != null && index > 0)
        MenuItemButton(
          key: const Key('queue-up'),
          leadingIcon: const Icon(Icons.arrow_upward, size: 18),
          onPressed: () => onMove!(index, -1),
          child: const Text('Move up'),
        ),
      if (onMove != null && index < lines.length - 1)
        MenuItemButton(
          key: const Key('queue-down'),
          leadingIcon: const Icon(Icons.arrow_downward, size: 18),
          onPressed: () => onMove!(index, 1),
          child: const Text('Move down'),
        ),
    ],
    builder: (context, controller, _) => IconButton(
      key: Key('queue-more-$index'),
      tooltip: 'More for this message',
      icon: const Icon(Icons.more_vert, size: 18),
      onPressed: () => controller.isOpen ? controller.close() : controller.open(),
    ),
  );

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    final small = TextStyle(fontSize: 12, color: look.muted);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 6, 16, 0),
      child: Center(
        heightFactor: 1,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: look.messageWidth + 100),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
            Row(children: [
              Icon(held == null ? Icons.schedule : Icons.pause_circle_outline, size: 16, color: look.muted),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  held == null
                      ? (merged
                            ? '${lines.length} queued: sent together when the turn ends'
                            : '${lines.length} queued: sent one at a time, as each turn ends')
                      : 'Held, since $held: nothing more is sent until you say',
                  style: small,
                ),
              ),
              if (held != null && !working && lines.length > 1 && onMergeSend != null)
                TextButton(key: const Key('queue-merge-send'), onPressed: onMergeSend, child: const Text('Merge & send')),
              if (held != null && !working && onSendNext != null) TextButton(onPressed: onSendNext, child: const Text('Send next')),
              if (lines.length > 1 && onMergeAll != null)
                TextButton(key: const Key('queue-merge-all'), onPressed: onMergeAll, child: const Text('Merge all')),
              TextButton(onPressed: onClear, child: const Text('Clear queue')),
            ]),
            ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 180),
              child: ListView(shrinkWrap: true, children: [
                for (final (index, line) in lines.indexed)
                  Container(
                    key: ObjectKey(line.key),
                    margin: const EdgeInsets.only(bottom: 4),
                    padding: const EdgeInsets.only(left: 12),
                    decoration: BoxDecoration(
                      border: Border.all(color: look.muted.withValues(alpha: 0.6)),
                      borderRadius: BorderRadius.circular(look.radius / 2),
                    ),
                    child: Row(children: [
                      Expanded(
                        child: Text(
                          [
                            line.text.trim(),
                            if (line.attachments.isNotEmpty) '(with ${line.attachments.join(', ')})',
                          ].where((part) => part.isNotEmpty).join(' '),
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                      IconButton(
                        tooltip: 'Edit queued message',
                        icon: const Icon(Icons.edit_outlined, size: 18),
                        onPressed: () => _edit(context, index),
                      ),
                      IconButton(tooltip: 'Remove queued message', icon: const Icon(Icons.close, size: 18), onPressed: () => onRemove(index)),
                      if (onSteer != null || onMerge != null || onToComposer != null || onMove != null) _rowMenu(index),
                    ]),
                  ),
              ]),
            ),
          ]),
        ),
      ),
    );
  }
}

class _EditQueued extends StatefulWidget {
  const _EditQueued({required this.text});

  final String text;

  @override
  State<_EditQueued> createState() => _EditQueuedState();
}

class _EditQueuedState extends State<_EditQueued> {
  late final _text = TextEditingController(text: widget.text);

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Queued message'),
    content: SizedBox(
      width: 480,
      child: TextField(key: const Key('queued-text'), controller: _text, autofocus: true, minLines: 2, maxLines: 8),
    ),
    actions: [
      TextButton(onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
      FilledButton(onPressed: () => Navigator.pop(context, _text.text), child: const Text('Save')),
    ],
  );
}

/// What the session has used, as the agent says, in one quiet line under the conversation; nothing when it said none.
class UsageFooter extends StatelessWidget {
  const UsageFooter({super.key, required this.text});

  final String? text;

  @override
  Widget build(BuildContext context) {
    final text = this.text;
    if (text == null) return const SizedBox.shrink();
    final look = context.chatLook;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16),
      child: Text(text, style: TextStyle(fontSize: look.bodySize - 3, color: look.muted)),
    );
  }
}
