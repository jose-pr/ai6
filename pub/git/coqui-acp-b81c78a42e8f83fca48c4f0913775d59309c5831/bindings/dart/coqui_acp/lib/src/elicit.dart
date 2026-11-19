part of '../coqui_acp.dart';

// An agent's questions to the person: ACP's `elicitation/create`, in form mode. What the agent sends is read into fields a client
// can render, and an answer is sent only when it fits them, as vectors/elicitation.json says for this client and the Rust one.

/// One choice a field offers.
final class AcpChoice {
  const AcpChoice(this.value, this.title, {this.description});

  /// What the answer holds when this is chosen.
  final String value;

  /// The agent's words for it; the value when it gave none.
  final String title;
  final String? description;
}

/// One question of a form. Its text is the agent's, untrusted, to be shown as data.
sealed class AcpField {
  const AcpField(
    this.name, {
    this.title,
    this.description,
    this.required = false,
  });

  /// Its key in the answer.
  final String name;
  final String? title;
  final String? description;

  /// Whether an accepted answer has to hold it.
  final bool required;
}

/// Text, its length counted in characters.
final class AcpTextField extends AcpField {
  const AcpTextField(
    super.name, {
    super.title,
    super.description,
    super.required,
    this.minLength,
    this.maxLength,
    this.format,
    this.defaultValue,
  });
  final int? minLength, maxLength;

  /// `email`, `uri`, `date`, `date-time` or another the agent named, as an annotation.
  final String? format;
  final String? defaultValue;
}

/// One of the options, as text.
final class AcpChoiceField extends AcpField {
  const AcpChoiceField(
    super.name,
    this.options, {
    super.title,
    super.description,
    super.required,
    this.defaultValue,
  });
  final List<AcpChoice> options;
  final String? defaultValue;
}

/// Some of the options, as a list of text with none twice.
final class AcpChoicesField extends AcpField {
  const AcpChoicesField(
    super.name,
    this.options, {
    super.title,
    super.description,
    super.required,
    this.minItems,
    this.maxItems,
    this.defaultValue,
  });
  final List<AcpChoice> options;
  final int? minItems, maxItems;
  final List<String>? defaultValue;
}

/// A number, or a whole number when [integer].
final class AcpNumberField extends AcpField {
  const AcpNumberField(
    super.name, {
    required this.integer,
    super.title,
    super.description,
    super.required,
    this.minimum,
    this.maximum,
    this.defaultValue,
  });
  final bool integer;

  /// Inclusive bounds.
  final num? minimum, maximum;
  final num? defaultValue;
}

/// Yes or no.
final class AcpBooleanField extends AcpField {
  const AcpBooleanField(
    super.name, {
    super.title,
    super.description,
    super.required,
    this.defaultValue,
  });
  final bool? defaultValue;
}

/// A type this client does not know, which is never rendered as one it does: no answer can hold it.
final class AcpUnsupportedField extends AcpField {
  const AcpUnsupportedField(
    super.name,
    this.type, {
    super.title,
    super.description,
    super.required,
  });

  /// The type the agent named, or empty when it named none.
  final String type;
}

/// An agent's question to the person, as a form.
final class AcpElicitation {
  const AcpElicitation({
    required this.sessionId,
    required this.message,
    required this.fields,
    this.toolCallId,
    this.title,
    this.description,
  });
  final String sessionId;

  /// The tool call it is part of, when the agent said.
  final String? toolCallId;

  /// What the agent asks.
  final String message;
  final String? title;
  final String? description;

  /// Its questions, in the agent's order.
  final List<AcpField> fields;
}

/// The person's answer to an [AcpElicitation].
sealed class AcpElicitationAnswer {
  const AcpElicitationAnswer();
}

/// They answered: a value for each field they filled in, by name. It is sent only if it fits the form; otherwise the agent is
/// told the question was cancelled, and an [AcpBlocked] says why.
final class AcpElicitAccept extends AcpElicitationAnswer {
  const AcpElicitAccept(this.content);
  final Map<String, Object?> content;
}

/// They refused to answer.
final class AcpElicitDecline extends AcpElicitationAnswer {
  const AcpElicitDecline();
}

/// They dismissed it without choosing.
final class AcpElicitCancel extends AcpElicitationAnswer {
  const AcpElicitCancel();
}

/// Puts an agent's question to the person.
typedef AcpElicitor =
    Future<AcpElicitationAnswer> Function(AcpElicitation request);

String? _string(Map map, String key) =>
    map[key] is String ? map[key] as String : null;

int? _count(Map map, String key) =>
    map[key] is int && (map[key] as int) >= 0 ? map[key] as int : null;

/// The options of a `oneOf` or `anyOf` list of `{const, title}`, or of an `enum` list of text; what is not text is left out.
List<AcpChoice>? _choices(Map map, String titled) {
  if (map[titled] case final List listed) {
    return [
      for (final item in listed)
        if (item is Map && item['const'] is String)
          AcpChoice(
            item['const'] as String,
            _string(item, 'title') ?? item['const'] as String,
            description: _string(item, 'description'),
          ),
    ];
  }
  if (map['enum'] case final List listed) {
    return [
      for (final value in listed.whereType<String>()) AcpChoice(value, value),
    ];
  }
  return null;
}

AcpField _field(String name, Map schema, bool required) {
  final title = _string(schema, 'title');
  final description = _string(schema, 'description');
  final type = _string(schema, 'type') ?? '';
  switch (type) {
    case 'string':
      final options = _choices(schema, 'oneOf');
      if (options != null) {
        final chosen = _string(schema, 'default');
        return AcpChoiceField(
          name,
          options,
          title: title,
          description: description,
          required: required,
          defaultValue: options.any((option) => option.value == chosen)
              ? chosen
              : null,
        );
      }
      return AcpTextField(
        name,
        title: title,
        description: description,
        required: required,
        minLength: _count(schema, 'minLength'),
        maxLength: _count(schema, 'maxLength'),
        format: _string(schema, 'format'),
        defaultValue: _string(schema, 'default'),
      );
    case 'array':
      final items = schema['items'];
      final options =
          items is Map &&
              (items['type'] == null || items['type'] is String) &&
              (items.containsKey('anyOf') || items['type'] == 'string')
          ? _choices(items, 'anyOf')
          : null;
      if (options == null) break;
      final chosen = schema['default'];
      return AcpChoicesField(
        name,
        options,
        title: title,
        description: description,
        required: required,
        minItems: _count(schema, 'minItems'),
        maxItems: _count(schema, 'maxItems'),
        defaultValue:
            chosen is List &&
                chosen.every(
                  (value) =>
                      value is String &&
                      options.any((option) => option.value == value),
                )
            ? chosen.cast<String>()
            : null,
      );
    case 'number' || 'integer':
      final integer = type == 'integer';
      num? bound(String key) => integer
          ? (schema[key] is int ? schema[key] as int : null)
          : (schema[key] is num ? schema[key] as num : null);
      return AcpNumberField(
        name,
        integer: integer,
        title: title,
        description: description,
        required: required,
        minimum: bound('minimum'),
        maximum: bound('maximum'),
        defaultValue: bound('default'),
      );
    case 'boolean':
      return AcpBooleanField(
        name,
        title: title,
        description: description,
        required: required,
        defaultValue: schema['default'] is bool
            ? schema['default'] as bool
            : null,
      );
  }
  return AcpUnsupportedField(
    name,
    type,
    title: title,
    description: description,
    required: required,
  );
}

/// Reads `elicitation/create`'s parameters, or says why they cannot be put to a person.
(AcpElicitation?, String?) _readElicitation(
  Object? params,
  int maxFields,
  int maxOptions,
) {
  const unreadable =
      'the agent asked a question in a form this client cannot read';
  if (params is! Map) return (null, unreadable);
  final mode = params['mode'];
  if (mode is String && mode != 'form') {
    return (
      null,
      'the agent asked a question by $mode, which this client does not offer',
    );
  }
  if (mode != 'form') return (null, unreadable);
  final sessionId = _string(params, 'sessionId');
  if (sessionId == null) {
    return (null, 'the agent asked a question tied to no session');
  }
  final message = _string(params, 'message');
  final schema = params['requestedSchema'];
  if (message == null || schema is! Map || schema['properties'] is! Map) {
    return (null, unreadable);
  }
  final properties = schema['properties'] as Map;
  if (properties.length > maxFields) {
    return (
      null,
      'the agent asked ${properties.length} questions at once, more than the $maxFields this client takes',
    );
  }
  final required = schema['required'] is List
      ? (schema['required'] as List).whereType<String>().toSet()
      : const <String>{};
  final fields = <AcpField>[];
  for (final MapEntry(:key, :value) in properties.entries) {
    final field = _field(
      '$key',
      value is Map ? value : const {},
      required.contains(key),
    );
    final options = switch (field) {
      AcpChoiceField(:final options) ||
      AcpChoicesField(:final options) => options.length,
      _ => 0,
    };
    if (options > maxOptions) {
      return (
        null,
        'the agent offered $options choices for one question, more than the $maxOptions this client takes',
      );
    }
    fields.add(field);
  }
  return (
    AcpElicitation(
      sessionId: sessionId,
      toolCallId: _string(params, 'toolCallId'),
      message: message,
      title: _string(schema, 'title'),
      description: _string(schema, 'description'),
      fields: fields,
    ),
    null,
  );
}

/// Why [content] is not an answer [request]'s form allows, or null when it is.
String? _misfit(AcpElicitation request, Map<String, Object?> content) {
  for (final name in content.keys) {
    if (!request.fields.any((field) => field.name == name)) {
      return 'the answer has $name, which the agent did not ask';
    }
  }
  for (final field in request.fields) {
    final name = field.name;
    if (!content.containsKey(name)) {
      if (field.required) {
        return 'the answer leaves out $name, which the agent requires';
      }
      continue;
    }
    final value = content[name];
    bool within(num at, num? least, num? most) =>
        (least == null || at >= least) && (most == null || at <= most);
    final fits = switch (field) {
      AcpTextField(:final minLength, :final maxLength) =>
        value is String && within(value.runes.length, minLength, maxLength),
      AcpChoiceField(:final options) =>
        value is String && options.any((option) => option.value == value),
      AcpChoicesField(:final options, :final minItems, :final maxItems) =>
        value is List &&
            value.every(
              (item) =>
                  item is String &&
                  options.any((option) => option.value == item),
            ) &&
            value.toSet().length == value.length &&
            within(value.length, minItems, maxItems),
      AcpNumberField(:final integer, :final minimum, :final maximum) =>
        (integer ? value is int : value is num) &&
            within(value as num, minimum, maximum),
      AcpBooleanField() => value is bool,
      AcpUnsupportedField() => false,
    };
    if (!fits) return "the answer's $name does not fit what the agent asked";
  }
  return null;
}

/// One question of a JSON Schema `properties` map, as [AcpElicitation] reads it: for a library that reads forms of its own.
AcpField acpFieldFromSchema(String name, Map schema, {bool required = false}) =>
    _field(name, schema, required);

/// Why [content] is not an answer to the questions [request] holds, or null when it is.
String? acpMisfit(AcpElicitation request, Map<String, Object?> content) =>
    _misfit(request, content);
