import 'package:material_ui/material_ui.dart';

import 'form_model.dart';

/// What a field that names something may hold: what to offer, and whether a value not offered
/// is taken too.
class FieldChoices {
  const FieldChoices(this.values, {this.open = false, this.labels = const {}});

  final List<String> values;

  /// A list that cannot be complete, so a typed value is taken.
  final bool open;
  final Map<String, String> labels;
}

/// Answers the choices for a `format` the schema names; `null` when that format offers none.
typedef ChoicesFor = FieldChoices? Function(String format);

/// Draws a [FormModel]: its sections in order, each control by what the schema says the field
/// holds, the fields the model hides left out, and what is rarely changed folded away.
class RecordFormView extends StatelessWidget {
  const RecordFormView({
    required this.model,
    this.choicesFor,
    this.problems = const {},
    this.enabled = true,
    super.key,
  });

  final FormModel model;
  final ChoicesFor? choicesFor;

  /// What is wrong with a field, by its path.
  final Map<String, String> problems;
  final bool enabled;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: model,
    builder: (context, _) => _Fields(
      context: _FormContext(model, choicesFor, problems, enabled),
      path: '',
      schema: model.schema,
    ),
  );
}

class _FormContext {
  const _FormContext(this.model, this.choicesFor, this.problems, this.enabled);

  final FormModel model;
  final ChoicesFor? choicesFor;
  final Map<String, String> problems;
  final bool enabled;

  bool hasProblemIn(String path) => problems.keys.any(
    (key) =>
        key == path || key.startsWith('$path.') || key.startsWith('$path['),
  );
}

/// The members of what is at `path`.
class _Fields extends StatelessWidget {
  const _Fields({
    required this.context,
    required this.path,
    required this.schema,
  });

  final _FormContext context;
  final String path;
  final FieldSchema schema;

  @override
  Widget build(BuildContext buildContext) {
    final model = context.model;
    final children = <Widget>[];
    final groups = model.layout(path);
    for (final group in groups) {
      final basic = <Widget>[];
      final advanced = <Widget>[];
      var advancedProblem = false;
      for (final key in group.fields) {
        if (!model.isVisible(path, key)) continue;
        final field = schema.member(key);
        if (field == null) continue;
        final full = path.isEmpty ? key : '$path.$key';
        final control = _Control(
          key: ValueKey('form-control-$full'),
          context: context,
          path: full,
          schema: field,
          hints: model.hints(path, key),
          required: schema.requiredKeys.contains(key),
        );
        if (field.advanced) {
          advanced.add(control);
          advancedProblem = advancedProblem || context.hasProblemIn(full);
        } else {
          basic.add(control);
        }
      }
      if (basic.isEmpty && advanced.isEmpty) continue;
      final title = group.title;
      children.add(
        _Section(
          id: path.isEmpty ? group.id : '$path/${group.id}',
          title: title,
          help: group.help,
          basic: basic,
          advanced: advanced,
          showAdvanced: advancedProblem,
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: children,
    );
  }
}

/// One section: what most people need, then what few do, folded. A section of only the latter
/// is folded whole.
class _Section extends StatelessWidget {
  const _Section({
    required this.id,
    required this.title,
    required this.help,
    required this.basic,
    required this.advanced,
    required this.showAdvanced,
  });

  final String id;
  final String title;
  final String help;
  final List<Widget> basic;
  final List<Widget> advanced;
  final bool showAdvanced;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    Widget folded(String label) => ExpansionTile(
      key: ValueKey('form-more-$id-$showAdvanced'),
      tilePadding: EdgeInsets.zero,
      initiallyExpanded: showAdvanced,
      title: Text(label),
      subtitle: basic.isEmpty && help.isNotEmpty ? Text(help) : null,
      childrenPadding: const EdgeInsets.only(bottom: 8),
      children: advanced,
    );
    if (basic.isEmpty) {
      return folded(title.isEmpty ? 'More settings' : title);
    }
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (title.isNotEmpty)
            Semantics(
              header: true,
              child: Text(title, style: theme.textTheme.titleSmall),
            ),
          if (help.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(help, style: theme.textTheme.bodySmall),
            ),
          const SizedBox(height: 8),
          ...basic,
          if (advanced.isNotEmpty) folded('More settings'),
        ],
      ),
    );
  }
}

/// Picks the control for one field, from what its schema says it holds.
class _Control extends StatelessWidget {
  const _Control({
    required this.context,
    required this.path,
    required this.schema,
    required this.hints,
    required this.required,
    super.key,
  });

  final _FormContext context;
  final String path;
  final FieldSchema schema;
  final FieldHints hints;
  final bool required;

  @override
  Widget build(BuildContext buildContext) {
    final form = context;
    final format = schema.format;
    final choices = format == null ? null : form.choicesFor?.call(format);
    final Widget control;
    if (schema.memberArm != null) {
      control = _ArmChoice(form, path, schema);
    } else if (schema.numberArm != null && schema.wordArm != null) {
      control = _NumberOrWord(form, path, schema, hints);
    } else if (schema.hasMembers) {
      control = _Group(form, path, schema);
    } else if (schema.isList) {
      final items = schema.items ?? FieldSchema(const {});
      control = items.hasMembers
          ? _ListOfGroups(form, path, schema, items)
          : _ListOfValues(
              form,
              path,
              schema,
              items,
              choices ??
                  (items.format == null
                      ? null
                      : form.choicesFor?.call(items.format!)),
            );
    } else if (schema.isObject) {
      final values = schema.values ?? FieldSchema(const {});
      control = values.hasMembers
          ? _MapOfGroups(form, path, schema, values)
          : _MapOfValues(form, path, schema, values);
    } else if (schema.isBool) {
      control = _Switch(form, path, schema);
    } else if (schema.choices != null) {
      control = _Choice(form, path, schema, required);
    } else if (choices != null) {
      control = _Picker(form, path, schema, choices);
    } else {
      control = _TextControl(form, path, schema, hints, required);
    }
    return Padding(padding: const EdgeInsets.only(bottom: 12), child: control);
  }
}

InputDecoration _decoration(
  _FormContext form,
  String path,
  FieldSchema schema, {
  bool required = false,
  String? hint,
  String? suffix,
  String? extra,
}) {
  final words = [
    schema.description,
    ?extra,
  ].where((text) => text.isNotEmpty).join(' ');
  return InputDecoration(
    labelText: required ? '${schema.title} (required)' : schema.title,
    helperText: words.isEmpty ? null : words,
    helperMaxLines: 6,
    errorMaxLines: 6,
    errorText: form.problems[path],
    hintText: hint,
    suffixText: suffix,
    border: const OutlineInputBorder(),
  );
}

/// How a field's default reads in an empty box.
String? _defaultText(FieldSchema schema) {
  final value = schema.defaultValue;
  return switch (value) {
    null => null,
    final String text when text.isEmpty => null,
    final String text => 'Default: ${schema.labelOf(text)}',
    final num number => 'Default: ${_grouped(number)}',
    final bool on => on ? 'Default: on' : 'Default: off',
    final List<Object?> list when list.isNotEmpty =>
      'Default: ${list.join(', ')}',
    _ => null,
  };
}

String _grouped(num number) {
  final text = number.toString();
  if (number is! int) return text;
  final out = StringBuffer();
  for (var index = 0; index < text.length; index++) {
    if (index > 0 && (text.length - index) % 3 == 0 && text[index - 1] != '-') {
      out.write(',');
    }
    out.write(text[index]);
  }
  return out.toString();
}

/// What a number counted in seconds comes to.
String? _spanWords(num seconds) {
  if (seconds < 60) return null;
  const steps = [(86400, 'day'), (3600, 'hour'), (60, 'minute')];
  for (final (size, name) in steps) {
    if (seconds >= size) {
      final count = seconds / size;
      final shown = count == count.roundToDouble()
          ? count.toStringAsFixed(0)
          : count.toStringAsFixed(1);
      return '$shown $name${shown == '1' ? '' : 's'}';
    }
  }
  return null;
}

/// What a number counted in bytes comes to.
String? _sizeWords(num bytes) {
  const steps = [(1 << 30, 'GiB'), (1 << 20, 'MiB'), (1 << 10, 'KiB')];
  for (final (size, name) in steps) {
    if (bytes >= size) {
      final count = bytes / size;
      final shown = count == count.roundToDouble()
          ? count.toStringAsFixed(0)
          : count.toStringAsFixed(1);
      return '$shown $name';
    }
  }
  return null;
}

/// The unit to show beside a number, and what the number comes to in plainer words.
(String?, String?) _numberWords(
  FieldSchema schema,
  FieldHints hints,
  String? text,
) {
  final number = text == null ? null : num.tryParse(text.trim());
  return switch (schema.format) {
    'seconds' => ('seconds', number == null ? null : _spanWords(number)),
    'bytes' => ('bytes', number == null ? null : _sizeWords(number)),
    _ => (hints.unit, null),
  };
}

class _TextControl extends StatefulWidget {
  const _TextControl(
    this.form,
    this.path,
    this.schema,
    this.hints,
    this.required,
  );

  final _FormContext form;
  final String path;
  final FieldSchema schema;
  final FieldHints hints;
  final bool required;

  @override
  State<_TextControl> createState() => _TextControlState();
}

class _TextControlState extends State<_TextControl> {
  late final _sensitive = widget.schema.sensitive;

  // A secret is never put back in a box: an empty one keeps what is stored.
  late final _controller = TextEditingController(
    text: _sensitive ? '' : widget.form.model.text(widget.path) ?? '',
  );
  late final _stored = widget.form.model.text(widget.path);

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final schema = widget.schema;
    final model = widget.form.model;
    final isNumber = schema.isInteger || schema.isNumber;
    final multiline = widget.hints.widget == 'textarea';
    final (unit, words) = isNumber
        ? _numberWords(schema, widget.hints, _controller.text)
        : (null, null);
    final hint = _sensitive && _stored != null
        ? 'Set; type to replace it'
        : widget.hints.placeholder ?? _defaultText(schema);
    return TextField(
      key: Key('form-field-${widget.path}'),
      controller: _controller,
      enabled: widget.form.enabled,
      obscureText: _sensitive,
      enableSuggestions: !_sensitive,
      autocorrect: !_sensitive,
      minLines: multiline ? 3 : 1,
      maxLines: multiline ? 8 : 1,
      keyboardType: isNumber
          ? const TextInputType.numberWithOptions(decimal: true, signed: true)
          : multiline
          ? TextInputType.multiline
          : TextInputType.text,
      decoration: _decoration(
        widget.form,
        widget.path,
        schema,
        required: widget.required,
        hint: hint,
        suffix: unit,
        extra: words == null ? null : 'Now $words.',
      ),
      onChanged: (text) {
        if (_sensitive && text.isEmpty) {
          model.setText(widget.path, _stored);
        } else {
          model.setText(widget.path, text);
        }
        if (isNumber) setState(() {});
      },
    );
  }
}

class _Switch extends StatelessWidget {
  const _Switch(this.form, this.path, this.schema);

  final _FormContext form;
  final String path;
  final FieldSchema schema;

  @override
  Widget build(BuildContext context) {
    final stored = form.model.text(path);
    final on = stored == null ? schema.defaultValue == true : stored == 'true';
    final problem = form.problems[path];
    return SwitchListTile(
      key: Key('form-field-$path'),
      contentPadding: EdgeInsets.zero,
      title: Text(schema.title),
      subtitle: problem != null
          ? Text(
              problem,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            )
          : schema.description.isEmpty
          ? null
          : Text(schema.description),
      value: on,
      onChanged: form.enabled
          ? (value) => form.model.setText(path, value ? 'true' : 'false')
          : null,
    );
  }
}

class _Choice extends StatelessWidget {
  const _Choice(this.form, this.path, this.schema, this.required);

  final _FormContext form;
  final String path;
  final FieldSchema schema;
  final bool required;

  @override
  Widget build(BuildContext context) {
    final values = schema.choices!;
    final stored = form.model.text(path);
    final fallback = schema.defaultValue;
    return DropdownButtonFormField<String>(
      key: Key('form-field-$path'),
      isExpanded: true,
      initialValue: stored ?? '',
      decoration: _decoration(form, path, schema, required: required),
      items: [
        if (!required)
          DropdownMenuItem(
            value: '',
            child: Text(
              fallback is String
                  ? 'Default: ${schema.labelOf(fallback)}'
                  : 'Not set',
              overflow: TextOverflow.ellipsis,
            ),
          ),
        for (final value in {...values, ?stored})
          DropdownMenuItem(
            value: value,
            child: Text(schema.labelOf(value), overflow: TextOverflow.ellipsis),
          ),
      ],
      onChanged: form.enabled
          ? (value) => form.model.setText(path, value)
          : null,
    );
  }
}

/// A field that names something from a list the page supplies: a choice when the list is
/// complete, and a box with suggestions when it cannot be.
class _Picker extends StatelessWidget {
  const _Picker(this.form, this.path, this.schema, this.choices);

  final _FormContext form;
  final String path;
  final FieldSchema schema;
  final FieldChoices choices;

  @override
  Widget build(BuildContext context) {
    final stored = form.model.text(path);
    if (!choices.open) {
      return DropdownButtonFormField<String>(
        key: Key('form-field-$path'),
        isExpanded: true,
        initialValue: stored ?? '',
        decoration: _decoration(form, path, schema),
        items: [
          DropdownMenuItem(
            value: '',
            child: Text(_defaultText(schema) ?? 'None'),
          ),
          for (final value in {
            ...choices.values,
            ...?schema.suggestions,
            ?stored,
          })
            DropdownMenuItem(
              value: value,
              child: Text(
                choices.labels[value] ?? value,
                overflow: TextOverflow.ellipsis,
              ),
            ),
        ],
        onChanged: form.enabled
            ? (value) => form.model.setText(path, value)
            : null,
      );
    }
    return Autocomplete<String>(
      key: Key('form-field-$path'),
      initialValue: TextEditingValue(text: stored ?? ''),
      optionsBuilder: (value) {
        final text = value.text.toLowerCase();
        return choices.values.where(
          (choice) => choice.toLowerCase().contains(text),
        );
      },
      onSelected: (value) => form.model.setText(path, value),
      fieldViewBuilder: (context, controller, focus, onSubmitted) => TextField(
        controller: controller,
        focusNode: focus,
        enabled: form.enabled,
        decoration: _decoration(form, path, schema, hint: _defaultText(schema)),
        onChanged: (text) => form.model.setText(path, text),
        onSubmitted: (_) => onSubmitted(),
      ),
    );
  }
}

/// A number, or a word that takes its place, such as no limit at all.
class _NumberOrWord extends StatefulWidget {
  const _NumberOrWord(this.form, this.path, this.schema, this.hints);

  final _FormContext form;
  final String path;
  final FieldSchema schema;
  final FieldHints hints;

  @override
  State<_NumberOrWord> createState() => _NumberOrWordState();
}

class _NumberOrWordState extends State<_NumberOrWord> {
  late final _word = widget.schema.wordArm!.choices!.first;
  late final _controller = TextEditingController(
    text: widget.form.model.text(widget.path) == _word
        ? ''
        : widget.form.model.text(widget.path) ?? '',
  );

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final model = widget.form.model;
    final schema = widget.schema;
    final lifted = model.text(widget.path) == _word;
    final (unit, words) = _numberWords(schema, widget.hints, _controller.text);
    final wordArm = schema.wordArm!;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TextField(
          key: Key('form-field-${widget.path}'),
          controller: _controller,
          enabled: widget.form.enabled && !lifted,
          keyboardType: const TextInputType.numberWithOptions(decimal: true),
          decoration: _decoration(
            widget.form,
            widget.path,
            schema,
            hint: _defaultText(schema),
            suffix: unit,
            extra: words == null ? null : 'Now $words.',
          ),
          onChanged: (text) {
            model.setText(widget.path, text);
            setState(() {});
          },
        ),
        CheckboxListTile(
          key: Key('form-word-${widget.path}'),
          contentPadding: EdgeInsets.zero,
          controlAffinity: ListTileControlAffinity.leading,
          dense: true,
          title: Text(wordArm.labelOf(_word)),
          value: lifted,
          onChanged: widget.form.enabled
              ? (value) => model.setText(
                  widget.path,
                  value == true ? _word : _controller.text,
                )
              : null,
        ),
      ],
    );
  }
}

/// One of several arms: a few words, or a group of fields of its own.
class _ArmChoice extends StatelessWidget {
  const _ArmChoice(this.form, this.path, this.schema);

  final _FormContext form;
  final String path;
  final FieldSchema schema;

  static const _memberValue = '\u0000members';

  @override
  Widget build(BuildContext context) {
    final model = form.model;
    final member = schema.memberArm!;
    final words = schema.wordArm;
    final current = model.usesMemberArm(path)
        ? _memberValue
        : model.text(path) ?? '';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        DropdownButtonFormField<String>(
          key: Key('form-field-$path'),
          isExpanded: true,
          initialValue: current,
          decoration: _decoration(form, path, schema),
          items: [
            const DropdownMenuItem(value: '', child: Text('Default')),
            for (final word in words?.choices ?? const <String>[])
              DropdownMenuItem(value: word, child: Text(words!.labelOf(word))),
            DropdownMenuItem(
              value: _memberValue,
              child: Text(member.title.isEmpty ? 'Custom' : member.title),
            ),
          ],
          onChanged: form.enabled
              ? (value) =>
                    model.chooseArm(path, value == _memberValue ? null : value)
              : null,
        ),
        if (model.usesMemberArm(path))
          Padding(
            padding: const EdgeInsets.only(top: 12),
            child: _Fields(context: form, path: path, schema: member),
          ),
      ],
    );
  }
}

/// A field with members of its own: folded unless most people need it.
class _Group extends StatelessWidget {
  const _Group(this.form, this.path, this.schema);

  final _FormContext form;
  final String path;
  final FieldSchema schema;

  @override
  Widget build(BuildContext context) {
    final problem = form.hasProblemIn(path);
    return ExpansionTile(
      key: ValueKey('form-group-$path-$problem'),
      tilePadding: EdgeInsets.zero,
      initiallyExpanded: problem,
      title: Text(schema.title),
      subtitle: schema.description.isEmpty ? null : Text(schema.description),
      childrenPadding: const EdgeInsets.only(left: 12, bottom: 8),
      children: [_Fields(context: form, path: path, schema: schema)],
    );
  }
}

class _ListOfValues extends StatefulWidget {
  const _ListOfValues(
    this.form,
    this.path,
    this.schema,
    this.items,
    this.choices,
  );

  final _FormContext form;
  final String path;
  final FieldSchema schema;
  final FieldSchema items;
  final FieldChoices? choices;

  @override
  State<_ListOfValues> createState() => _ListOfValuesState();
}

class _ListOfValuesState extends State<_ListOfValues> {
  final _adding = TextEditingController();

  @override
  void dispose() {
    _adding.dispose();
    super.dispose();
  }

  void _add(String text) {
    final value = text.trim();
    if (value.isEmpty) return;
    widget.form.model.addToList(widget.path, value);
    _adding.clear();
  }

  @override
  Widget build(BuildContext context) {
    final form = widget.form;
    final model = form.model;
    final path = widget.path;
    final held = [
      for (final at in model.positions(path))
        (at, model.text('$path[$at]') ?? ''),
    ];
    final closed = widget.items.choices;
    final choices = closed != null
        ? FieldChoices(
            closed,
            labels: {
              for (final value in closed) value: widget.items.labelOf(value),
            },
          )
        : widget.choices;
    final theme = Theme.of(context);
    final offered = [
      for (final value in choices?.values ?? const <String>[])
        if (!held.any((entry) => entry.$2 == value)) value,
    ];
    final fallback = _defaultText(widget.schema);
    final problem = form.problems[path];
    return Column(
      key: Key('form-field-$path'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(widget.schema.title, style: theme.textTheme.bodyMedium),
        if (widget.schema.description.isNotEmpty)
          Text(widget.schema.description, style: theme.textTheme.bodySmall),
        if (problem != null)
          Text(
            problem,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.error,
            ),
          ),
        const SizedBox(height: 6),
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final (at, value) in held)
              InputChip(
                label: Text(choices?.labels[value] ?? value),
                onDeleted: form.enabled
                    ? () => model.removeFromList(path, at)
                    : null,
                deleteButtonTooltipMessage: 'Remove $value',
              ),
            for (final value in offered)
              ActionChip(
                avatar: const Icon(Icons.add, size: 16),
                label: Text(choices?.labels[value] ?? value),
                onPressed: form.enabled ? () => _add(value) : null,
              ),
          ],
        ),
        if (held.isEmpty && fallback != null)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(fallback, style: theme.textTheme.bodySmall),
          ),
        if (choices == null || choices.open)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: TextField(
              key: Key('form-add-$path'),
              controller: _adding,
              enabled: form.enabled,
              decoration: InputDecoration(
                isDense: true,
                labelText: 'Add to ${widget.schema.title}',
                suffixIcon: IconButton(
                  tooltip: 'Add',
                  icon: const Icon(Icons.add),
                  onPressed: form.enabled ? () => _add(_adding.text) : null,
                ),
              ),
              onSubmitted: _add,
            ),
          ),
      ],
    );
  }
}

class _ListOfGroups extends StatelessWidget {
  const _ListOfGroups(this.form, this.path, this.schema, this.items);

  final _FormContext form;
  final String path;
  final FieldSchema schema;
  final FieldSchema items;

  @override
  Widget build(BuildContext context) {
    final model = form.model;
    return _Titled(
      schema: schema,
      problem: form.problems[path],
      children: [
        for (final at in model.positions(path))
          _Entry(
            title: '${schema.title} ${at + 1}',
            onRemove: form.enabled
                ? () => model.removeFromList(path, at)
                : null,
            child: _Fields(context: form, path: '$path[$at]', schema: items),
          ),
        Align(
          alignment: Alignment.centerLeft,
          child: TextButton.icon(
            onPressed: form.enabled
                ? () => model.setText('$path[${model.nextPosition(path)}]', ' ')
                : null,
            icon: const Icon(Icons.add),
            label: Text('Add to ${schema.title}'),
          ),
        ),
      ],
    );
  }
}

class _Titled extends StatelessWidget {
  const _Titled({required this.schema, required this.children, this.problem});

  final FieldSchema schema;
  final String? problem;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(schema.title, style: theme.textTheme.bodyMedium),
        if (schema.description.isNotEmpty)
          Text(schema.description, style: theme.textTheme.bodySmall),
        if (problem case final problem?)
          Text(
            problem,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.error,
            ),
          ),
        const SizedBox(height: 6),
        ...children,
      ],
    );
  }
}

class _Entry extends StatelessWidget {
  const _Entry({
    required this.title,
    required this.child,
    this.onRemove,
    super.key,
  });

  final String title;
  final Widget child;
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 8),
    child: DecoratedBox(
      decoration: BoxDecoration(
        border: Border(
          left: BorderSide(color: Theme.of(context).dividerColor, width: 2),
        ),
      ),
      child: Padding(
        padding: const EdgeInsets.only(left: 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(child: Text(title)),
                IconButton(
                  tooltip: 'Remove $title',
                  icon: const Icon(Icons.delete_outline),
                  onPressed: onRemove,
                ),
              ],
            ),
            child,
          ],
        ),
      ),
    ),
  );
}

/// A box asking for a new key, for a map whose keys are data.
class _NewKey extends StatefulWidget {
  const _NewKey({required this.path, required this.label, this.onAdd});

  final String path;
  final String label;
  final ValueChanged<String>? onAdd;

  @override
  State<_NewKey> createState() => _NewKeyState();
}

class _NewKeyState extends State<_NewKey> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _add() {
    final key = _controller.text.trim();
    if (key.isEmpty) return;
    widget.onAdd?.call(key);
    _controller.clear();
  }

  @override
  Widget build(BuildContext context) => TextField(
    key: Key('form-add-${widget.path}'),
    controller: _controller,
    enabled: widget.onAdd != null,
    decoration: InputDecoration(
      isDense: true,
      labelText: widget.label,
      suffixIcon: IconButton(
        tooltip: 'Add',
        icon: const Icon(Icons.add),
        onPressed: widget.onAdd == null ? null : _add,
      ),
    ),
    onSubmitted: (_) => _add(),
  );
}

class _MapOfValues extends StatelessWidget {
  const _MapOfValues(this.form, this.path, this.schema, this.values);

  final _FormContext form;
  final String path;
  final FieldSchema schema;
  final FieldSchema values;

  @override
  Widget build(BuildContext context) {
    final model = form.model;
    return _Titled(
      schema: schema,
      problem: form.problems[path],
      children: [
        for (final key in model.mapKeys(path))
          Padding(
            key: ValueKey('form-entry-$path-$key'),
            padding: const EdgeInsets.only(bottom: 8),
            child: Row(
              children: [
                Expanded(child: Text(key, overflow: TextOverflow.ellipsis)),
                const SizedBox(width: 8),
                Expanded(
                  flex: 2,
                  child: _EntryText(
                    form: form,
                    path: keyedPath(path, key),
                    key_: key,
                  ),
                ),
                IconButton(
                  tooltip: 'Remove $key',
                  icon: const Icon(Icons.delete_outline),
                  onPressed: form.enabled
                      ? () => model.removeFromMap(path, key)
                      : null,
                ),
              ],
            ),
          ),
        _NewKey(
          path: path,
          label: 'Add to ${schema.title}: name',
          onAdd: form.enabled ? (key) => model.addToMap(path, key) : null,
        ),
      ],
    );
  }
}

class _EntryText extends StatefulWidget {
  const _EntryText({
    required this.form,
    required this.path,
    required this.key_,
  });

  final _FormContext form;
  final String path;
  final String key_;

  @override
  State<_EntryText> createState() => _EntryTextState();
}

class _EntryTextState extends State<_EntryText> {
  late final _controller = TextEditingController(
    text: widget.form.model.text(widget.path)?.trim() ?? '',
  );

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => TextField(
    key: Key('form-field-${widget.path}'),
    controller: _controller,
    enabled: widget.form.enabled,
    decoration: InputDecoration(
      isDense: true,
      labelText: 'Value of ${widget.key_}',
      errorText: widget.form.problems[widget.path],
    ),
    onChanged: (text) {
      widget.form.model.setEntry(widget.path, text);
    },
  );
}

class _MapOfGroups extends StatelessWidget {
  const _MapOfGroups(this.form, this.path, this.schema, this.values);

  final _FormContext form;
  final String path;
  final FieldSchema schema;
  final FieldSchema values;

  @override
  Widget build(BuildContext context) {
    final model = form.model;
    return _Titled(
      schema: schema,
      problem: form.problems[path],
      children: [
        for (final key in model.mapKeys(path))
          _Entry(
            key: ValueKey('form-entry-$path-$key'),
            title: key,
            onRemove: form.enabled
                ? () => model.removeFromMap(path, key)
                : null,
            child: _Fields(
              context: form,
              path: keyedPath(path, key),
              schema: values,
            ),
          ),
        _NewKey(
          path: path,
          label: 'Add to ${schema.title}: name',
          onAdd: form.enabled ? (key) => model.addToMap(path, key) : null,
        ),
      ],
    );
  }
}
