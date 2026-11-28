import 'dart:async';
import 'dart:convert';

import 'package:coqui_acp/coqui_acp.dart';
import 'package:coqui_acp/secret.dart';
import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';

import 'cards.dart' show plainText, shortLine;
import 'entry.dart';
import 'look.dart';
import 'renderers.dart';

/// What the cards of the secret-prompt extension (`docs/secrets.md` of coqui-acp) need from the app, the same for every request: who
/// the person's own identity is and how it proves, which identities it knows, and which one the host is.
///
/// The identity's key stays wherever the app keeps it: [prover] and [verifier] take functions, never a key.
final class SecretCardEnvironment {
  const SecretCardEnvironment({
    required this.prover,
    required this.verifier,
    required this.expected,
    this.host = 'This app',
    this.now,
  });

  /// This frontend's identity, which binds the answer of a request and the key of a show.
  final AcpSecretProver prover;

  /// Checks what the host sent; its [AcpSecretVerifier.own] is [prover]'s id and it knows [expected].
  final AcpSecretVerifier verifier;

  /// The only identity a request may come from: the host this app is paired with. Another one that [verifier] knows is refused.
  final AcpIdentityId expected;

  /// What the app calls itself in the cards' sentences.
  final String host;

  /// The clock, in Unix seconds; the system's when null. A test sets it.
  final int Function()? now;

  int get seconds => now?.call() ?? DateTime.now().millisecondsSinceEpoch ~/ 1000;
}

const String _mask = '••••••••••••';

/// The part of a card every secret prompt shares: who asks, what it says, and the one sentence about where the answer goes.
class _SecretFrame extends StatelessWidget {
  const _SecretFrame({
    required this.cardKey,
    required this.agent,
    required this.verb,
    required this.title,
    required this.message,
    required this.note,
    required this.rows,
    required this.actions,
    this.more = 0,
  });

  final Key cardKey;
  final String agent;
  final String verb;
  final String? title;
  final String message;
  final String note;
  final List<Widget> rows;
  final List<Widget> actions;
  final int more;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
      child: Card(
        key: cardKey,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: MediaQuery.sizeOf(context).height * 0.7),
          child: SingleChildScrollView(
            padding: const EdgeInsets.symmetric(vertical: 12),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 16),
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Row(children: [
                    const Icon(Icons.lock_outline),
                    const SizedBox(width: 12),
                    Expanded(child: Text('${shortLine(agent) ?? 'The agent'} $verb', style: theme.textTheme.labelMedium)),
                  ]),
                  if (title != null) Padding(padding: const EdgeInsets.only(top: 4), child: Text(shortLine(title) ?? '', style: theme.textTheme.titleSmall)),
                  SelectableText(plainText(message)),
                  Padding(padding: const EdgeInsets.only(top: 4), child: Text(note, style: theme.textTheme.bodySmall?.copyWith(color: context.chatLook.muted))),
                ]),
              ),
              const SizedBox(height: 4),
              ...rows,
              if (more > 0) Padding(padding: const EdgeInsets.fromLTRB(16, 4, 16, 0), child: Text('$more more after this', style: theme.textTheme.labelSmall)),
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
                child: Wrap(alignment: WrapAlignment.end, spacing: 8, children: actions),
              ),
            ]),
          ),
        ),
      ),
    );
  }
}

/// A row of a form in Settings' grammar: the title, the words under it, and the control, with a rule between rows.
class _Row extends StatelessWidget {
  const _Row({required this.title, this.description, this.control, this.trailing, this.tag});

  final String title;
  final String? description;

  /// Under the words, the whole row's width (a field).
  final Widget? control;

  /// Beside the title (a switch or a drop-down).
  final Widget? trailing;

  /// Where the answer goes, in words: a row of a form says whether it is sealed or the agent sees it.
  final String? tag;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(mainAxisSize: MainAxisSize.min, children: [
      const Divider(height: 1),
      ListTile(
        title: Text(title),
        subtitle: description == null && control == null && tag == null
            ? null
            : Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                if (description != null) Text(description!, style: theme.textTheme.bodySmall),
                if (tag != null) Text(tag!, key: const ValueKey('where-it-goes'), style: theme.textTheme.labelSmall?.copyWith(fontWeight: FontWeight.w600)),
                if (control != null) Padding(padding: const EdgeInsets.only(top: 6), child: control),
              ]),
        trailing: trailing,
      ),
    ]);
  }
}

String _label(AcpField field) => '${shortLine(field.title) ?? shortLine(field.name) ?? 'A question'}${field.required ? ' *' : ''}';

String? _description(AcpField field) => field.description == null ? null : plainText(field.description!);

/// An agent's request for values, one of them a secret, as a form built from the request's schema. The secret fields are masked as
/// they are typed, cannot be selected or copied, and are sealed to the host's per-request key on Submit; the sealed answer goes to
/// [onReply] and nowhere else. The agent is never sent the answer. A request the app cannot verify (its key's binding fails, it is
/// not from the host, its form breaks a rule) draws nothing to fill and offers only Cancel.
///
/// A new [request] object starts a new form.
class SecretRequestCard extends StatefulWidget {
  const SecretRequestCard({
    super.key,
    required this.request,
    required this.environment,
    required this.onReply,
    this.agent = 'The agent',
    this.more = 0,
  });

  final AcpSecretRequestParams request;
  final SecretCardEnvironment environment;

  /// Told once: the sealed answer (accept), decline or cancel.
  final ValueChanged<AcpSecretRequestReply> onReply;
  final String agent;
  final int more;

  @override
  State<SecretRequestCard> createState() => _SecretRequestCardState();
}

class _SecretRequestCardState extends State<SecretRequestCard> {
  AcpSecretCollector? _collector;
  bool _refused = false;
  bool _busy = false;
  bool _replied = false;
  String? _failed;
  final Map<String, Object> _values = {};
  final Map<String, TextEditingController> _texts = {};

  @override
  void initState() {
    super.initState();
    _begin();
  }

  @override
  void didUpdateWidget(SecretRequestCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(widget.request, oldWidget.request)) {
      _wipe();
      _begin();
    }
  }

  void _begin() {
    _collector = null;
    _refused = false;
    _busy = false;
    _replied = false;
    _failed = null;
    final request = widget.request;
    final environment = widget.environment;
    unawaited(() async {
      try {
        final collector = await environment.verifier.collector(
          environment.expected,
          AcpSecretMethod.request,
          request.context,
          request.recipient,
          environment.seconds,
        );
        if (!mounted || !identical(request, widget.request)) return;
        setState(() {
          _collector = collector;
          for (final each in collector.form.fields) {
            final field = each.field;
            if (field is AcpTextField || field is AcpNumberField) _texts[field.name] = TextEditingController();
            if (!each.secret) {
              final start = switch (field) {
                AcpChoiceField(:final defaultValue) => defaultValue,
                AcpChoicesField(:final defaultValue) => defaultValue,
                AcpBooleanField(:final defaultValue) => defaultValue,
                _ => null,
              };
              if (start != null) _values[field.name] = start;
            }
          }
        });
      } on Object {
        // Nothing the person could read says more than that it was not asked: a refused key tells the agent no more either.
        if (mounted && identical(request, widget.request)) setState(() => _refused = true);
      }
    }());
  }

  /// Forgets what was typed and held, including the text inside the controllers.
  void _wipe() {
    for (final controller in _texts.values) {
      controller.clear();
      controller.dispose();
    }
    _texts.clear();
    _values.clear();
  }

  @override
  void dispose() {
    _wipe();
    super.dispose();
  }

  /// Answers once. The form is replaced by a final state, so no field is left to undo into or to edit, and the controllers go once
  /// the fields have left the tree.
  void _reply(AcpSecretRequestReply reply) {
    if (_replied) return;
    _replied = true;
    if (mounted) {
      setState(() {});
      WidgetsBinding.instance.addPostFrameCallback((_) => _wipe());
    } else {
      _wipe();
    }
    widget.onReply(reply);
  }

  /// What was typed in the text and number fields, by name; empty fields are left out.
  Map<String, String> get _typed => {
    for (final MapEntry(:key, :value) in _texts.entries)
      if (value.text.isNotEmpty) key: value.text,
  };

  /// The plain answers as a map, numbers read from what was typed.
  Map<String, Object?> _plainMap(AcpSecretForm form) {
    final typed = _typed;
    final plain = <String, Object?>{};
    for (final each in form.fields) {
      if (each.secret) continue;
      final field = each.field;
      switch (field) {
        case AcpTextField():
          if (typed[field.name] case final text?) plain[field.name] = text;
        case AcpNumberField(:final integer):
          if (typed[field.name] case final text?) {
            final number = integer ? int.tryParse(text) : num.tryParse(text);
            if (number != null) plain[field.name] = number;
          }
        default:
          if (_values[field.name] case final value?) plain[field.name] = value;
      }
    }
    return plain;
  }

  /// Why the answer cannot be sealed yet, or null when it can.
  String? _problem(AcpSecretForm form) {
    final typed = _typed;
    for (final each in form.fields) {
      final field = each.field;
      final label = shortLine(field.title) ?? shortLine(field.name) ?? 'A question';
      if (field is AcpUnsupportedField) return '$label cannot be answered here, so only Decline or Cancel can answer this.';
      if (!each.secret) continue;
      final text = typed[field.name];
      if (text == null) {
        if (field.required) return '$label is required.';
        continue;
      }
      try {
        form.checkSecret(field.name, utf8.encode(text));
      } on AcpSecretException {
        return '$label does not fit what was asked.';
      }
    }
    for (final each in form.fields) {
      final field = each.field;
      if (each.secret) continue;
      final label = shortLine(field.title) ?? shortLine(field.name) ?? 'A question';
      if (field is AcpNumberField && (typed[field.name]?.isNotEmpty ?? false) && _plainMap(form)[field.name] == null) {
        return '$label is not a number.';
      }
    }
    final plain = _plainMap(form);
    for (final each in form.fields) {
      if (!each.secret && each.field.required && !plain.containsKey(each.field.name)) {
        return '${shortLine(each.field.title) ?? shortLine(each.field.name) ?? 'A question'} is required.';
      }
    }
    try {
      form.checkPlain(plain);
    } on AcpSecretException {
      return 'An answer does not fit what was asked.';
    }
    return null;
  }

  Future<void> _submit() async {
    final collector = _collector;
    if (collector == null || _busy || _replied) return;
    setState(() {
      _busy = true;
      _failed = null;
    });
    try {
      final typed = _typed;
      final plain = jsonEncode(_plainMap(collector.form));
      final sealed = <String, AcpSealed>{};
      for (final each in collector.form.fields) {
        if (!each.secret) continue;
        if (typed[each.field.name] case final text?) sealed[each.field.name] = await collector.seal(each.field.name, text);
      }
      final auth = await collector.authenticate(widget.environment.prover, plain, sealed, widget.environment.seconds);
      // A request that replaced this one while the seal ran is not answered with this answer.
      if (!mounted || !identical(collector, _collector)) return;
      _reply(AcpSecretAccepted(plain, sealed, auth));
    } on Object {
      if (mounted && identical(collector, _collector)) {
        setState(() {
          _busy = false;
          _failed = 'The answer could not be sealed, so nothing was sent. Try again, or cancel.';
        });
      }
    }
  }

  Widget _secretField(AcpSecretField each) {
    final field = each.field;
    return _Row(
      title: _label(field),
      description: _description(field),
      tag: 'Sealed: only the host can read it',
      control: TextField(
        key: ValueKey('secret-${field.name}'),
        controller: _texts[field.name],
        obscureText: true,
        // What is typed never leaves the field: an obscured field has no copy or cut, its menu holds Paste alone (a sign-in code
        // is pasted from the page that showed it), and there are no suggestions or saved words.
        contextMenuBuilder: (context, state) {
          final paste = [for (final item in state.contextMenuButtonItems) if (item.type == ContextMenuButtonType.paste) item];
          return paste.isEmpty
              ? const SizedBox.shrink()
              : AdaptiveTextSelectionToolbar.buttonItems(anchors: state.contextMenuAnchors, buttonItems: paste);
        },
        enableSuggestions: false,
        autocorrect: false,
        enableIMEPersonalizedLearning: false,
        autofillHints: null,
        keyboardType: field is AcpNumberField ? TextInputType.numberWithOptions(decimal: !field.integer, signed: true) : TextInputType.visiblePassword,
        decoration: const InputDecoration(isDense: true, border: OutlineInputBorder()),
        onChanged: (_) => setState(() {}),
      ),
    );
  }

  static const _seen = 'The agent sees this';

  Widget _plainField(AcpField field) {
    void set(Object? value) => setState(() => value == null ? _values.remove(field.name) : _values[field.name] = value);
    final key = ValueKey('question-${field.name}');
    switch (field) {
      case AcpTextField():
        return _Row(
          title: _label(field),
          tag: _seen,
          description: _description(field),
          control: TextField(key: key, controller: _texts[field.name], decoration: const InputDecoration(isDense: true, border: OutlineInputBorder()), onChanged: (_) => setState(() {})),
        );
      case AcpNumberField(:final integer):
        return _Row(
          title: _label(field),
          tag: _seen,
          description: _description(field),
          control: TextField(
            key: key,
            controller: _texts[field.name],
            keyboardType: TextInputType.numberWithOptions(decimal: !integer, signed: true),
            inputFormatters: [FilteringTextInputFormatter.allow(RegExp(integer ? r'[-0-9]' : r'[-0-9.eE+]'))],
            decoration: const InputDecoration(isDense: true, border: OutlineInputBorder()),
            onChanged: (_) => setState(() {}),
          ),
        );
      case AcpBooleanField():
        return SwitchListTile(
          key: key,
          title: Text(_label(field)),
          subtitle: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            if (_description(field) != null) Text(_description(field)!),
            Text(_seen, key: const ValueKey('where-it-goes'), style: Theme.of(context).textTheme.labelSmall?.copyWith(fontWeight: FontWeight.w600)),
          ]),
          value: _values[field.name] == true,
          onChanged: set,
        );
      case AcpChoiceField(:final options):
        return _Row(
          title: _label(field),
          tag: _seen,
          description: _description(field),
          trailing: DropdownButton<String>(
            key: key,
            value: _values[field.name] as String?,
            hint: const Text('Choose'),
            items: [for (final option in options) DropdownMenuItem(value: option.value, child: Text(shortLine(option.title) ?? shortLine(option.value) ?? '?'))],
            onChanged: set,
          ),
        );
      case AcpChoicesField(:final options):
        final chosen = [...(_values[field.name] as List<String>?) ?? const <String>[]];
        return _Row(
          title: _label(field),
          tag: _seen,
          description: _description(field),
          control: Wrap(spacing: 6, runSpacing: 4, children: [
            for (final option in options)
              FilterChip(
                key: ValueKey('question-${field.name}-${option.value}'),
                label: Text(shortLine(option.title) ?? shortLine(option.value) ?? '?'),
                selected: chosen.contains(option.value),
                onSelected: (on) {
                  final now = [...(_values[field.name] as List<String>?) ?? const <String>[]];
                  on ? now.add(option.value) : now.remove(option.value);
                  set([for (final each in options) if (now.contains(each.value)) each.value].nullIfEmpty);
                },
              ),
          ]),
        );
      case AcpUnsupportedField(:final type):
        return _Row(title: _label(field), tag: _seen, description: 'A question of a kind ${widget.environment.host} cannot show (${shortLine(type) ?? 'no type'}), so it is left unanswered.');
    }
  }

  @override
  Widget build(BuildContext context) {
    final request = widget.request;
    final collector = _collector;
    const note = 'Only the fields marked Sealed are sealed on this device for the host and kept from the agent. A field marked The agent '
        'sees this is given to the agent as you type it, whatever its title says.';
    if (_refused || _replied) {
      return _SecretFrame(
        cardKey: const Key('secret-request-card'),
        agent: widget.agent,
        verb: 'asks for a secret',
        title: null,
        message: plainText(request.context.message),
        note: _replied
            ? 'Answered. The form was cleared.'
            : 'This request cannot be verified as coming from the host this app is paired with, or its form breaks a rule, so nothing can be '
                  'entered. The agent is told only that you were not asked.',
        rows: const [],
        more: widget.more,
        actions: _replied ? const [] : [TextButton(onPressed: () => _reply(const AcpSecretCancelled()), child: const Text('Cancel'))],
      );
    }
    final form = collector?.form;
    final problem = form == null ? null : _problem(form);
    return _SecretFrame(
      cardKey: const Key('secret-request-card'),
      agent: widget.agent,
      verb: 'asks for a secret',
      title: form?.title,
      message: request.context.message,
      note: note,
      more: widget.more,
      rows: [
        if (form == null) const Padding(padding: EdgeInsets.all(16), child: LinearProgressIndicator()),
        if (form != null && form.description != null) Padding(padding: const EdgeInsets.fromLTRB(16, 0, 16, 4), child: Text(plainText(form.description!), style: Theme.of(context).textTheme.bodySmall)),
        if (form != null)
          for (final each in form.fields) each.secret ? _secretField(each) : _plainField(each.field),
        const Divider(height: 1),
        if (problem != null) Padding(padding: const EdgeInsets.fromLTRB(16, 8, 16, 0), child: Text(problem, style: Theme.of(context).textTheme.labelSmall?.copyWith(fontWeight: FontWeight.w600))),
        if (_failed != null) Padding(padding: const EdgeInsets.fromLTRB(16, 8, 16, 0), child: Text(_failed!, style: Theme.of(context).textTheme.labelSmall?.copyWith(color: context.chatLook.danger))),
      ],
      actions: [
        TextButton(onPressed: () => _reply(const AcpSecretCancelled()), child: const Text('Cancel')),
        TextButton(onPressed: () => _reply(const AcpSecretDeclined()), child: const Text('Decline')),
        FilledButton(onPressed: form != null && problem == null && !_busy ? _submit : null, child: const Text('Send sealed')),
      ],
    );
  }
}

extension on List<String> {
  List<String>? get nullIfEmpty => isEmpty ? null : this;
}

/// Where a [SecretShowCard] is in its two steps.
enum _ShowStage { asking, waiting, shown, closed, refused }

/// An agent's request to show the person values, some of them secret. The person first says whether to look; then the card answers the
/// open step with a per-request key bound to the app's identity ([onReady]); the host's sealed values arrive in [deliver], are opened
/// here and shown masked, to reveal or copy on demand. [onDone] says whether the person saw them. The values are kept only by this
/// card and overwritten when it leaves.
class SecretShowCard extends StatefulWidget {
  const SecretShowCard({
    super.key,
    required this.open,
    required this.environment,
    required this.deliver,
    required this.onReady,
    required this.onDone,
    this.agent = 'The agent',
    this.more = 0,
    this.copy,
  });

  final AcpSecretShowOpen open;
  final SecretCardEnvironment environment;

  /// The host's second step, null until it arrives. A card whose request id or session differs from [open]'s refuses it.
  final ValueListenable<AcpSecretShowDeliver?> deliver;

  /// Told once with the answer to the open step: ready (with the per-request key), decline or cancel.
  final ValueChanged<AcpSecretShowOpened> onReady;

  /// Told once with the answer to the deliver step: true when the person saw the values and closed the card, false for cancel.
  final ValueChanged<bool> onDone;
  final String agent;
  final int more;

  /// Puts text on the clipboard; the system's when null. Called only when the person presses Copy.
  final Future<void> Function(String text)? copy;

  @override
  State<SecretShowCard> createState() => _SecretShowCardState();
}

class _SecretShowCardState extends State<SecretShowCard> {
  _ShowStage _stage = _ShowStage.asking;
  AcpSecretReceiver? _receiver;
  AcpOpened? _opened;
  AcpSecretForm? _form;
  final Set<String> _revealed = {};
  String? _copied;
  bool _readied = false;
  bool _finished = false;
  bool _opening = false;
  bool _looking = false;
  String? _copiedValue;

  @override
  void initState() {
    super.initState();
    widget.deliver.addListener(_delivered);
    _parse();
  }

  void _parse() {
    _form = null;
    _stage = _ShowStage.asking;
    try {
      _form = AcpSecretForm.parseText(widget.open.context.schema, widget.environment.verifier.limits);
    } on AcpSecretException {
      _stage = _ShowStage.refused;
    }
  }

  @override
  void didUpdateWidget(SecretShowCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.deliver, widget.deliver)) {
      oldWidget.deliver.removeListener(_delivered);
      widget.deliver.addListener(_delivered);
    }
    // Another show reuses this state as a new one: nothing of the first is kept, and what it was waiting on is not answered by this.
    if (!identical(oldWidget.open, widget.open)) {
      _forget();
      _receiver = null;
      _readied = _finished = _opening = _looking = false;
      _copied = null;
      _parse();
    }
  }

  @override
  void dispose() {
    widget.deliver.removeListener(_delivered);
    _forget();
    unawaited(_clearClipboard());
    super.dispose();
  }

  /// Best effort: a value this card copied is taken off the clipboard if it is still what the clipboard holds.
  Future<void> _clearClipboard() async {
    final value = _copiedValue;
    _copiedValue = null;
    if (value == null) return;
    try {
      final now = await Clipboard.getData(Clipboard.kTextPlain);
      if (now?.text == value) await Clipboard.setData(const ClipboardData(text: ''));
    } on Object {
      // No clipboard to clear.
    }
  }

  /// Overwrites the opened values and the private key.
  void _forget() {
    _receiver?.clear();
    for (final secret in _opened?.secrets.values ?? const <AcpSecretValue>[]) {
      secret.clear();
    }
    _receiver = null;
    _opened = null;
    _revealed.clear();
  }

  void _ready(AcpSecretShowOpened reply) {
    if (_readied) return;
    _readied = true;
    widget.onReady(reply);
  }

  void _done(bool shown) {
    if (_finished) return;
    _finished = true;
    widget.onDone(shown);
  }

  Future<void> _look() async {
    // Set before the await: a second press while the key is made makes no second key.
    if (_looking || _stage != _ShowStage.asking) return;
    _looking = true;
    final environment = widget.environment;
    final open = widget.open;
    try {
      final receiver = await AcpSecretReceiver.create(
        AcpSecretMethod.show,
        open.context,
        environment.prover,
        environment.expected,
        environment.seconds,
        limits: environment.verifier.limits,
      );
      if (!mounted || !identical(open, widget.open)) {
        receiver.clear();
        return;
      }
      setState(() {
        _receiver = receiver;
        _stage = _ShowStage.waiting;
      });
      _ready(AcpSecretShowOpened.ready(receiver.recipient));
      // The host may have answered before the card listened.
      _delivered();
    } on Object {
      if (mounted && identical(open, widget.open)) setState(() => _stage = _ShowStage.refused);
    }
  }

  Future<void> _delivered() async {
    final deliver = widget.deliver.value;
    final receiver = _receiver;
    final open = widget.open;
    final context = open.context;
    if (deliver == null || receiver == null || _stage != _ShowStage.waiting || _opening) return;
    _opening = true;
    // A deliver for another request is not this card's, and nothing is opened for it.
    if (deliver.requestId != context.requestId || deliver.sessionId != context.sessionId) {
      setState(() {
        _forget();
        _stage = _ShowStage.refused;
      });
      _done(false);
      return;
    }
    final environment = widget.environment;
    try {
      final opened = await receiver.openValues(environment.verifier, environment.expected, context.values ?? '{}', deliver.sealed, deliver.auth, environment.seconds);
      if (!mounted || !identical(open, widget.open) || !identical(receiver, _receiver)) {
        for (final secret in opened.secrets.values) {
          secret.clear();
        }
        return;
      }
      setState(() {
        _opened = opened;
        _stage = _ShowStage.shown;
      });
    } on Object {
      if (mounted) {
        setState(() {
          _forget();
          _stage = _ShowStage.refused;
        });
      }
      _done(false);
    }
  }

  /// Ends the card: the host is told, and the values are overwritten.
  void _close(bool shown) {
    _done(shown);
    setState(() {
      _forget();
      _stage = _ShowStage.closed;
    });
    unawaited(_clearClipboard());
  }

  Future<void> _copy(String name, String text) async {
    await (widget.copy ?? (text) => Clipboard.setData(ClipboardData(text: text)))(text);
    _copiedValue = text;
    if (mounted) setState(() => _copied = name);
  }

  String _plainText(Object? value) => switch (value) {
    final List list => list.join(', '),
    null => '',
    _ => '$value',
  };

  Widget _value(AcpSecretField each) {
    final field = each.field;
    final opened = _opened;
    if (!each.secret) {
      return _Row(title: _label(field), control: SelectableText(plainText(_plainText(opened?.plain[field.name]))));
    }
    final value = opened?.secrets[field.name];
    final shown = _revealed.contains(field.name);
    return _Row(
      title: _label(field),
      description: _description(field),
      control: Row(children: [
        Expanded(
          child: value == null
              ? const Text('Not given')
              : shown
              ? SelectableText(value.text, key: ValueKey('shown-${field.name}'), style: TextStyle(fontFamily: context.chatLook.codeFont))
              : Text(_mask, key: ValueKey('masked-${field.name}')),
        ),
        if (value != null) ...[
          IconButton(
            key: ValueKey('reveal-${field.name}'),
            tooltip: shown ? 'Hide' : 'Show',
            icon: Icon(shown ? Icons.visibility_off_outlined : Icons.visibility_outlined),
            onPressed: () => setState(() => shown ? _revealed.remove(field.name) : _revealed.add(field.name)),
          ),
          IconButton(
            key: ValueKey('copy-${field.name}'),
            tooltip: _copied == field.name ? 'Copied' : 'Copy',
            icon: Icon(_copied == field.name ? Icons.check : Icons.copy_outlined),
            onPressed: () => _copy(field.name, value.text),
          ),
        ],
      ]),
    );
  }

  @override
  Widget build(BuildContext context) {
    final context0 = widget.open.context;
    final form = _form;
    final children = <Widget>[];
    List<Widget> actions;
    String note;
    switch (_stage) {
      case _ShowStage.asking:
        note = 'Nothing is shown until you choose to look. The values come sealed to this device from the host that runs the agent, never through the agent.';
        if (form != null) {
          for (final each in form.fields) {
            children.add(_Row(title: _label(each.field), description: _description(each.field), control: each.secret ? const Text(_mask) : null));
          }
        }
        actions = [
          TextButton(onPressed: () => _ready(const AcpSecretShowOpened.cancelled()), child: const Text('Cancel')),
          TextButton(onPressed: () => _ready(const AcpSecretShowOpened.declined()), child: const Text('Decline')),
          FilledButton(onPressed: _look, child: const Text('Show')),
        ];
      case _ShowStage.waiting:
        note = 'Waiting for the values from the host.';
        children.add(const Padding(padding: EdgeInsets.all(16), child: LinearProgressIndicator()));
        actions = [TextButton(onPressed: () => _close(false), child: const Text('Cancel'))];
      case _ShowStage.shown:
        note = 'The values are masked until you show them and are cleared when you close this. Copy puts one on the clipboard, which other apps can read.';
        if (form != null) children.addAll([for (final each in form.fields) _value(each)]);
        actions = [
          TextButton(onPressed: () => _close(false), child: const Text('Cancel')),
          FilledButton(onPressed: () => _close(true), child: const Text('Done')),
        ];
      case _ShowStage.closed:
        note = 'Closed. The values were cleared from this device.';
        actions = const [];
      case _ShowStage.refused:
        note = 'These values cannot be verified as coming from the host this app is paired with, or what was sent breaks a rule, so nothing is shown.';
        actions = [
          TextButton(
            onPressed: () {
              _ready(const AcpSecretShowOpened.cancelled());
              _done(false);
            },
            child: const Text('Close'),
          ),
        ];
    }
    return _SecretFrame(
      cardKey: const Key('secret-show-card'),
      agent: widget.agent,
      verb: 'wants to show you a secret',
      title: _stage == _ShowStage.refused ? null : form?.title,
      message: context0.message,
      note: note,
      more: widget.more,
      rows: children,
      actions: actions,
    );
  }
}

/// A secret request in a conversation, for [secretRenderers] to draw. It reads as the message the agent asked, never a value.
final class SecretRequestEntry implements ChatEntryData {
  SecretRequestEntry({required this.request, required this.onReply, required this.at, this.agent = 'The agent'});

  final AcpSecretRequestParams request;
  final ValueChanged<AcpSecretRequestReply> onReply;
  final String agent;

  @override
  final DateTime at;
  @override
  MessageRole get role => MessageRole.notice;
  @override
  String get text => 'Secret requested: ${plainText(request.context.message)}';
  @override
  ToolCall? get tool => null;
  @override
  Compaction? get compaction => null;
}

/// A secret show in a conversation, for [secretRenderers] to draw. [deliver] is set when the host's second step arrives.
final class SecretShowEntry implements ChatEntryData {
  SecretShowEntry({required this.open, required this.onReady, required this.onDone, required this.at, this.agent = 'The agent'}) : deliver = ValueNotifier(null);

  final AcpSecretShowOpen open;
  final ValueChanged<AcpSecretShowOpened> onReady;
  final ValueChanged<bool> onDone;
  final String agent;
  final ValueNotifier<AcpSecretShowDeliver?> deliver;

  @override
  final DateTime at;
  @override
  MessageRole get role => MessageRole.notice;
  @override
  String get text => 'Secret shown: ${plainText(open.context.message)}';
  @override
  ToolCall? get tool => null;
  @override
  Compaction? get compaction => null;
}

/// The renderers that draw [SecretRequestEntry] and [SecretShowEntry] as their cards. Merge them into a transcript's
/// [ChatRenderers]: `ChatTranscript(renderers: mine.merge(secretRenderers(environment)))`.
ChatRenderers secretRenderers(SecretCardEnvironment environment) => ChatRenderers(
  entry: [
    (context, entry) => switch (entry) {
      SecretRequestEntry() => SecretRequestCard(key: ValueKey('secret-${entry.request.context.sessionId}-${entry.request.context.requestId}'), request: entry.request, environment: environment, onReply: entry.onReply, agent: entry.agent),
      SecretShowEntry() => SecretShowCard(key: ValueKey('secret-${entry.open.context.sessionId}-${entry.open.context.requestId}'), open: entry.open, environment: environment, deliver: entry.deliver, onReady: entry.onReady, onDone: entry.onDone, agent: entry.agent),
      _ => null,
    },
  ],
);
