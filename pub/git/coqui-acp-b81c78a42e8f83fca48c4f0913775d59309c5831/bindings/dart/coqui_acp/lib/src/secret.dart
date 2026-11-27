/// The secret-prompt extension, `docs/secrets.md`: a form whose secret fields travel sealed (HPKE, RFC 9180) to a per-request key that
/// is bound to an identity the collector already knows, and sealed values the receiver opens only from the identity that sent them.
/// The Rust crate holds the same, and vectors/secret.json holds both to it.
library;

import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:pointycastle/export.dart' as pc;

import '../coqui_acp.dart';
import 'hpke.dart' as hpke;

/// The `_meta` key in which a frontend says it takes the extension.
const acpSecretsCapability = 'dev.boriken.coqui.acp/secrets';

/// The request that asks the person for values, some of them secret.
const acpSecretRequestMethod = '_coqui_acp/secret_request';

/// The first step of showing the person values: the host says what it will show, the frontend answers with its key.
const acpSecretShowOpenMethod = '_coqui_acp/secret_show_open';

/// The second step: the secret values, sealed to the key the first step gave. A frontend answers it only for a session and request id
/// it has an open step pending for, and once.
const acpSecretShowDeliverMethod = '_coqui_acp/secret_show_deliver';

/// The extension's version.
const acpSecretsVersion = 1;

/// The binding method for an identity that signs: ECDSA over P-256 and SHA-256, IEEE P1363, `s` low.
const acpBindingEcdsa = 'ecdsa-p256-sha256';

/// The binding method for an X25519 identity: HMAC-SHA-256 under a key derived from the two static keys' Diffie-Hellman.
const acpBindingDh = 'x25519-hmac-sha256';

/// The binding method for the in-process link only: HMAC-SHA-256 under a key derived from the link's own secret. The secret never
/// travels, so only the two ends of that link can make one. It is never offered on a link that has a relay.
const acpBindingLink = 'link-hmac-sha256';

/// How long a receiver's binding lasts by default: well under the 5 minutes a collector accepts, so it still verifies at a peer whose
/// clock is behind by the default skew.
const acpSecretDefaultLifetime = Duration(seconds: 240);

/// Why a secret message was refused; the same names as the Rust crate's error kinds.
enum AcpSecretFailure {
  schema('schema'),
  malformed('malformed'),
  unsupported('unsupported'),
  key('key'),
  unknownIdentity('unknown_identity'),
  badProof('bad_proof'),
  expired('expired'),
  wrongAudience('wrong_audience'),
  field('field'),
  seal('seal'),
  open('open'),
  tooLarge('too_large');

  const AcpSecretFailure(this.kind);

  /// The kind as `vectors/secret.json` names it.
  final String kind;
}

/// A refusal. It holds which check refused and a fixed reason: never a value, a key or a signature.
final class AcpSecretException implements Exception {
  const AcpSecretException(this.failure, [this.reason = '']);

  final AcpSecretFailure failure;
  final String reason;

  @override
  String toString() => 'AcpSecretException(${failure.kind}: $reason)';
}

AcpSecretException _fail(AcpSecretFailure failure, [String reason = '']) =>
    AcpSecretException(failure, reason);

/// Whether [text] holds a surrogate that is not half of a pair. UTF-8 cannot carry one: the encoder replaces it with U+FFFD, and two
/// different strings would then hash alike.
bool _hasLoneSurrogate(String text) {
  final units = text.codeUnits;
  for (var i = 0; i < units.length; i++) {
    final unit = units[i];
    if (unit >= 0xD800 && unit <= 0xDBFF) {
      if (i + 1 < units.length &&
          units[i + 1] >= 0xDC00 &&
          units[i + 1] <= 0xDFFF) {
        i++;
        continue;
      }
      return true;
    }
    if (unit >= 0xDC00 && unit <= 0xDFFF) return true;
  }
  return false;
}

/// UTF-8 of [text], or a `malformed` refusal for a string that has a lone surrogate. Every part that is hashed or sealed goes through it.
Uint8List _utf8(String text) {
  if (_hasLoneSurrogate(text)) {
    throw _fail(AcpSecretFailure.malformed, 'an unpaired surrogate');
  }
  return Uint8List.fromList(utf8.encode(text));
}

/// Reads JSON text strictly: an object that repeats a key, a string with a lone surrogate and nesting past 128 are refused, none of
/// which `jsonDecode` refuses. Throws [FormatException] whose message is `twice` for a repeated key.
final class _StrictJson {
  _StrictJson(this.text);

  final String text;
  var _at = 0;
  var _depth = 0;

  static Object? read(String text) {
    final reader = _StrictJson(text);
    final value = reader._value();
    reader._space();
    if (reader._at != text.length) throw const FormatException('trailing');
    return value;
  }

  String get _peek => _at < text.length ? text[_at] : '';

  void _space() {
    while (_at < text.length && ' \t\n\r'.contains(text[_at])) {
      _at++;
    }
  }

  Object? _value() {
    if (++_depth > 128) throw const FormatException('deep');
    _space();
    final Object? value;
    switch (_peek) {
      case '{':
        value = _object();
      case '[':
        value = _array();
      case '"':
        value = _string();
      case '':
        throw const FormatException('end');
      default:
        final start = _at;
        while (_at < text.length && !',]} \t\n\r'.contains(text[_at])) {
          _at++;
        }
        value = jsonDecode(text.substring(start, _at));
    }
    _depth--;
    return value;
  }

  String _string() {
    final start = _at++;
    while (_at < text.length && text[_at] != '"') {
      _at += text[_at] == r'\' ? 2 : 1;
    }
    if (_at >= text.length) throw const FormatException('end');
    _at++;
    final value = jsonDecode(text.substring(start, _at)) as String;
    if (_hasLoneSurrogate(value)) throw const FormatException('surrogate');
    return value;
  }

  Map<String, Object?> _object() {
    _at++;
    final map = <String, Object?>{};
    _space();
    if (_peek == '}') {
      _at++;
      return map;
    }
    while (true) {
      _space();
      if (_peek != '"') throw const FormatException('key');
      final key = _string();
      _space();
      if (_peek != ':') throw const FormatException('colon');
      _at++;
      final value = _value();
      if (map.containsKey(key)) throw const FormatException('twice');
      map[key] = value;
      _space();
      if (_peek == ',') {
        _at++;
        continue;
      }
      if (_peek == '}') {
        _at++;
        return map;
      }
      throw const FormatException('object');
    }
  }

  List<Object?> _array() {
    _at++;
    final list = <Object?>[];
    _space();
    if (_peek == ']') {
      _at++;
      return list;
    }
    while (true) {
      list.add(_value());
      _space();
      if (_peek == ',') {
        _at++;
        continue;
      }
      if (_peek == ']') {
        _at++;
        return list;
      }
      throw const FormatException('array');
    }
  }
}

/// [_StrictJson.read], with the refusal [failure] and the reasons for text that is no JSON and for one that repeats a key.
Object? _strictJson(
  String text,
  AcpSecretFailure failure,
  String what,
  String twice,
) {
  try {
    return _StrictJson.read(text);
  } on FormatException catch (error) {
    throw _fail(failure, error.message == 'twice' ? twice : what);
  }
}

/// How much of a form is taken; every value from 0 up is taken.
final class AcpSecretLimits {
  const AcpSecretLimits({
    this.fields = 64,
    this.options = 256,
    this.schemaBytes = 64 * 1024,
    this.nameBytes = 128,
    this.valueBytes = 64 * 1024,
  });

  final int fields, options, schemaBytes, nameBytes, valueBytes;
}

/// One question of a secret form.
final class AcpSecretField {
  const AcpSecretField(
    this.field, {
    required this.secret,
    this.encoding,
    this.mediaType,
  });

  /// The question as a plain form has it.
  final AcpField field;

  /// Whether its value is a secret: sealed on the way. A frontend masks or hides a value from this alone, never from `format`, a
  /// title or a name.
  final bool secret;

  /// The schema's `contentEncoding`.
  final String? encoding;

  /// The schema's `contentMediaType`.
  final String? mediaType;
}

/// The largest whole number every language the extension is written in holds exactly: 2^53 - 1.
const _maxSafe = 9007199254740991;

bool _digitsOnly(String text) =>
    text.isNotEmpty &&
    text.codeUnits.every((unit) => unit >= 0x30 && unit <= 0x39);

/// A whole number as text: an optional minus, then `0` or a digit 1 to 9 and more digits, within 2^53 - 1.
int? _jsonInteger(String text) {
  final digits = text.startsWith('-') ? text.substring(1) : text;
  if (!_digitsOnly(digits) ||
      (digits != '0' && digits.startsWith('0')) ||
      digits.length > 16) {
    return null;
  }
  final value = int.tryParse(text);
  return value != null && value.abs() <= _maxSafe ? value : null;
}

/// A number as text in JSON's grammar, finite.
double? _jsonNumber(String text) {
  var rest = text.startsWith('-') ? text.substring(1) : text;
  var length = 0;
  while (length < rest.length && _digitsOnly(rest[length])) {
    length++;
  }
  final whole = rest.substring(0, length);
  if (whole.isEmpty || (whole.length > 1 && whole.startsWith('0'))) return null;
  rest = rest.substring(length);
  if (rest.startsWith('.')) {
    var fraction = 1;
    while (fraction < rest.length && _digitsOnly(rest[fraction])) {
      fraction++;
    }
    if (fraction == 1) return null;
    rest = rest.substring(fraction);
  }
  if (rest.startsWith('e') || rest.startsWith('E')) {
    var exponent = rest.substring(1);
    if (exponent.startsWith('+') || exponent.startsWith('-')) {
      exponent = exponent.substring(1);
    }
    if (!_digitsOnly(exponent)) return null;
    rest = '';
  }
  if (rest.isNotEmpty) return null;
  final value = double.tryParse(text);
  return value != null && value.isFinite ? value : null;
}

/// Canonical base64 (standard alphabet when [url] is false, padded) or base64url (unpadded): no other padding, no other alphabet,
/// zero trailing bits.
bool _base64Canonical(String text, {required bool url}) {
  final alphabet = url
      ? RegExp(r'^[A-Za-z0-9_-]*$')
      : RegExp(r'^[A-Za-z0-9+/]*={0,2}$');
  if (!alphabet.hasMatch(text)) return false;
  if (url ? text.length % 4 == 1 : text.length % 4 != 0) return false;
  try {
    final bytes = url
        ? base64Url.decode(text.padRight((text.length + 3) ~/ 4 * 4, '='))
        : base64.decode(text);
    final again = url ? _b64u(bytes) : base64.encode(bytes);
    return again == text;
  } on FormatException {
    return false;
  }
}

/// The only keywords the schema's root may have: a flat object of questions.
const _rootAllowed = {'type', 'properties', 'required', 'title', 'description'};

/// The only keywords a secret field may have. Anything else could carry a value, a condition or a hint that makes a frontend draw or
/// treat the field as something other than a secret.
const _secretAllowed = {
  'type',
  'title',
  'description',
  'minLength',
  'maxLength',
  'minimum',
  'maximum',
  'writeOnly',
  'x-sensitive',
  'contentEncoding',
  'contentMediaType',
  'format',
};

/// The widgets of a layout for a question that is not secret (guatiao-intake's suggestions, minus the masking one).
const _plainWidgets = {
  'text',
  'textarea',
  'number',
  'slider',
  'checkbox',
  'toggle',
  'select',
  'radio',
  'dialog',
};

bool _flag(Map property, String key) {
  if (!property.containsKey(key)) return false;
  final value = property[key];
  if (value is bool) return value;
  throw _fail(
    AcpSecretFailure.schema,
    'writeOnly or x-sensitive is not a boolean',
  );
}

String? _text(Map map, String key) =>
    map[key] is String ? map[key] as String : null;

/// A form read from a schema whose secret properties are `writeOnly` or `x-sensitive`.
final class AcpSecretForm {
  AcpSecretForm._(this.title, this.description, this.fields, this.limits);

  final String? title;
  final String? description;
  final List<AcpSecretField> fields;
  final AcpSecretLimits limits;

  /// Reads a schema from the JSON text that was sent, or throws [AcpSecretException] (`schema`) naming the rule it breaks.
  factory AcpSecretForm.parseText(
    String schema, [
    AcpSecretLimits limits = const AcpSecretLimits(),
  ]) {
    if (_hasLoneSurrogate(schema)) {
      throw _fail(AcpSecretFailure.schema, 'the schema is not JSON');
    }
    if (_utf8(schema).length > limits.schemaBytes) {
      throw _fail(
        AcpSecretFailure.schema,
        'the schema is larger than the limit',
      );
    }
    final value = _strictJson(
      schema,
      AcpSecretFailure.schema,
      'the schema is not JSON',
      'the schema repeats a key',
    );
    return AcpSecretForm.parse(value, limits);
  }

  /// Reads a schema already parsed.
  factory AcpSecretForm.parse(
    Object? schema, [
    AcpSecretLimits limits = const AcpSecretLimits(),
  ]) {
    AcpSecretException bad(String why) => _fail(AcpSecretFailure.schema, why);
    if (schema is! Map) throw bad('the schema is not an object');
    if (_utf8(jsonEncode(schema)).length > limits.schemaBytes) {
      throw bad('the schema is larger than the limit');
    }
    if (schema.keys.any((key) => !_rootAllowed.contains(key))) {
      throw bad('the schema has a keyword a flat form does not');
    }
    if (schema.containsKey('type') && schema['type'] != 'object') {
      throw bad("the schema's type is not object");
    }
    final properties = schema['properties'];
    if (properties is! Map) throw bad('the schema has no properties');
    if (properties.isEmpty) throw bad('the schema asks nothing');
    if (properties.length > limits.fields) {
      throw bad('the schema has more questions than the limit');
    }
    final required = <String>[];
    if (schema.containsKey('required')) {
      final names = schema['required'];
      if (names is! List) throw bad('required is not a list');
      for (final name in names) {
        if (name is! String) {
          throw bad('required holds a name that is not text');
        }
        required.add(name);
      }
      if (required.any((name) => !properties.containsKey(name))) {
        throw bad('required names a property the schema lacks');
      }
    }
    final fields = <AcpSecretField>[];
    for (final MapEntry(:key, :value) in properties.entries) {
      final name = '$key';
      if (name.isEmpty ||
          _utf8(name).length > limits.nameBytes ||
          name.runes.any(
            (rune) => rune < 0x20 || (rune >= 0x7f && rune < 0xa0),
          )) {
        throw bad(
          "a property's name is empty, too long or holds a control character",
        );
      }
      if (value is! Map) throw bad('a property is not an object');
      final field = acpFieldFromSchema(
        name,
        value,
        required: required.contains(name),
      );
      final options = switch (field) {
        AcpChoiceField(:final options) ||
        AcpChoicesField(:final options) => options.length,
        _ => 0,
      };
      if (options > limits.options) {
        throw bad('a question has more choices than the limit');
      }
      if (field is AcpUnsupportedField) {
        throw bad("a property's type is one no control can show");
      }
      final secret = _flag(value, 'writeOnly') | _flag(value, 'x-sensitive');
      if (!secret && value['format'] == 'password') {
        throw bad('a password format without writeOnly or x-sensitive');
      }
      if (field is AcpNumberField &&
          field.integer &&
          [
            field.minimum,
            field.maximum,
          ].any((bound) => bound != null && bound.abs() > _maxSafe)) {
        throw bad('a whole-number bound beyond 2^53 - 1');
      }
      if (secret) {
        if (field is! AcpTextField && field is! AcpNumberField) {
          throw bad('a secret field is not text or a number');
        }
        if (value.keys.any((key) => !_secretAllowed.contains(key))) {
          throw bad('a secret field has a keyword a secret field may not');
        }
        if (value.containsKey('format') && value['format'] != 'password') {
          throw bad("a secret field's format is not password");
        }
      }
      fields.add(
        AcpSecretField(
          field,
          secret: secret,
          encoding: _text(value, 'contentEncoding'),
          mediaType: _text(value, 'contentMediaType'),
        ),
      );
    }
    return AcpSecretForm._(
      _text(schema, 'title'),
      _text(schema, 'description'),
      fields,
      limits,
    );
  }

  /// Checks the layout that came with the form, as the JSON text that was sent: guatiao-intake's form document over a flat form,
  /// `{"sections": [{"id", "title"?, "description"?}], "fields": {<question>: {"widget"?, "placeholder"?, "unit"?, "visibleWhen"?}}}`.
  /// A layout may place and label the questions; it may not change what a secret question is: a secret question has the `password`
  /// widget or none, no other question has it, and a condition may not read a secret question's value. Throws `schema`.
  void checkLayout(String text) {
    AcpSecretException bad(String why) => _fail(AcpSecretFailure.schema, why);
    if (_hasLoneSurrogate(text)) {
      throw bad('the layout is not JSON');
    }
    if (_utf8(text).length > limits.schemaBytes) {
      throw _fail(AcpSecretFailure.tooLarge, 'the layout');
    }
    final layout = _strictJson(
      text,
      AcpSecretFailure.schema,
      'the layout is not JSON',
      'the layout repeats a key',
    );
    if (layout is! Map) throw bad('the layout is not an object');
    bool only(Map map, Set<String> keys) =>
        map.keys.every((key) => keys.contains(key));
    if (!only(layout, {'sections', 'fields'})) {
      throw bad('the layout has a key the form document does not define');
    }
    const wrong = 'the layout is not a form document';
    final sections = layout['sections'];
    if (layout.containsKey('sections')) {
      if (sections is! List) throw bad(wrong);
      for (final section in sections) {
        if (section is! Map ||
            !only(section, {'id', 'title', 'description'}) ||
            section['id'] is! String ||
            (section.containsKey('title') && section['title'] is! String) ||
            (section.containsKey('description') &&
                section['description'] is! String)) {
          throw bad(wrong);
        }
      }
    }
    if (layout.containsKey('fields')) {
      final fields = layout['fields'];
      if (fields is! Map) throw bad(wrong);
      for (final MapEntry(:key, :value) in fields.entries) {
        final question = field('$key');
        if (question == null) {
          throw bad('the layout names a question the form lacks');
        }
        if (value is! Map) throw bad(wrong);
        if (!only(value, {'widget', 'placeholder', 'unit', 'visibleWhen'})) {
          throw bad('the layout has a hint a flat form does not take');
        }
        for (final hint in const ['placeholder', 'unit']) {
          if (value.containsKey(hint) && value[hint] is! String) {
            throw bad(wrong);
          }
        }
        if (value.containsKey('widget')) {
          final widget = value['widget'];
          if (widget is! String) throw bad(wrong);
          final fits = question.secret
              ? widget == 'password'
              : _plainWidgets.contains(widget);
          if (!fits) {
            throw bad(
              'a widget that hides, shows or relabels a question the schema says otherwise',
            );
          }
        }
        if (value.containsKey('visibleWhen')) {
          final when = value['visibleWhen'];
          final target = when is Map && when['field'] is String
              ? field(when['field'] as String)
              : null;
          if (when is! Map ||
              !only(when, {'field', 'equals', 'in'}) ||
              when.containsKey('equals') == when.containsKey('in') ||
              (when.containsKey('in') && when['in'] is! List) ||
              target == null ||
              target.secret) {
            throw bad(
              'a condition that is not on a plain question of the form',
            );
          }
        }
      }
    }
  }

  /// The question named [name], or null.
  AcpSecretField? field(String name) {
    for (final field in fields) {
      if (field.field.name == name) return field;
    }
    return null;
  }

  /// Throws `field` unless [values] answer the plain questions: no name that is not one, none that is secret, every required one
  /// present, each of its type and within its bounds.
  void checkPlain(Map<String, Object?> values) {
    for (final name in values.keys) {
      final field = this.field(name);
      if (field == null) {
        throw _fail(
          AcpSecretFailure.field,
          'a value for a question the form lacks',
        );
      }
      if (field.secret) {
        throw _fail(
          AcpSecretFailure.field,
          'a secret field holds a plain value',
        );
      }
    }
    final plain = AcpElicitation(
      sessionId: '',
      message: '',
      fields: [
        for (final field in fields)
          if (!field.secret) field.field,
      ],
    );
    if (acpMisfit(plain, values) != null) {
      throw _fail(
        AcpSecretFailure.field,
        'a plain value does not fit its question',
      );
    }
  }

  /// Throws `field` unless [names] are secret questions with every required one present.
  void checkSealed(Iterable<String> names) {
    final given = names.toList();
    for (final name in given) {
      if (!(field(name)?.secret ?? false)) {
        throw _fail(
          AcpSecretFailure.field,
          'a sealed value for a question that is not secret',
        );
      }
    }
    if (fields.any(
      (field) =>
          field.secret &&
          field.field.required &&
          !given.contains(field.field.name),
    )) {
      throw _fail(
        AcpSecretFailure.field,
        'a required secret question has no value',
      );
    }
  }

  /// Throws unless [value] fits secret question [name]: UTF-8 text of its length, a number as text in JSON's grammar within its
  /// bounds, and canonical base64 when the schema says so.
  void checkSecret(String name, List<int> value) {
    final question = field(name);
    if (question == null || !question.secret) {
      throw _fail(AcpSecretFailure.field, 'not a secret question of the form');
    }
    if (value.length > limits.valueBytes) {
      throw _fail(AcpSecretFailure.tooLarge, 'a secret value');
    }
    final String text;
    try {
      text = utf8.decode(value);
    } on FormatException {
      throw _fail(AcpSecretFailure.field, 'a secret value is not text');
    }
    bool within(num at, num? least, num? most) =>
        (least == null || at >= least) && (most == null || at <= most);
    final fits = switch (question.field) {
      AcpTextField(:final minLength, :final maxLength) =>
        within(text.runes.length, minLength, maxLength) &&
            switch (question.encoding) {
              'base64' => _base64Canonical(text, url: false),
              'base64url' => _base64Canonical(text, url: true),
              _ => true,
            },
      AcpNumberField(:final integer, :final minimum, :final maximum) =>
        switch (integer ? _jsonInteger(text) : _jsonNumber(text)) {
          final num at => within(at, minimum, maximum),
          null => false,
        },
      _ => false,
    };
    if (!fits) {
      throw _fail(
        AcpSecretFailure.field,
        'a secret value does not fit its question',
      );
    }
  }
}

String _b64u(List<int> bytes) => base64Url.encode(bytes).replaceAll('=', '');

Uint8List _b64uDecode(String text) {
  if (!_base64Canonical(text, url: true)) {
    throw _fail(AcpSecretFailure.malformed, 'not unpadded base64url');
  }
  return Uint8List.fromList(
    base64Url.decode(text.padRight((text.length + 3) ~/ 4 * 4, '=')),
  );
}

String _hex(List<int> bytes) =>
    bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();

Uint8List _unhex(String text) => Uint8List.fromList([
  for (var i = 0; i < text.length; i += 2)
    int.parse(text.substring(i, i + 2), radix: 16),
]);

Uint8List _concat(Iterable<List<int>> parts) {
  final out = BytesBuilder(copy: false);
  parts.forEach(out.add);
  return out.toBytes();
}

Uint8List _lp(List<int> bytes) => _concat([
  (ByteData(4)..setUint32(0, bytes.length)).buffer.asUint8List(),
  bytes,
]);

/// A number as 8 big-endian bytes, written as two 32-bit words so it also runs where `int` is a JavaScript number.
Uint8List _u64(int value) {
  final data = ByteData(8)
    ..setUint32(0, value ~/ 0x100000000)
    ..setUint32(4, value % 0x100000000);
  return data.buffer.asUint8List();
}

bool _sameBytes(List<int> a, List<int> b) {
  if (a.length != b.length) return false;
  var diff = 0;
  for (var i = 0; i < a.length; i++) {
    diff |= a[i] ^ b[i];
  }
  return diff == 0;
}

Uint8List _sha256(List<int> data) =>
    pc.SHA256Digest().process(Uint8List.fromList(data));

/// An identity's id as the peer methods write it: `sha256:` and the lower-case hexadecimal SHA-256 of its `SubjectPublicKeyInfo`. A
/// proof's bytes are not an identifier of anything: an ECDSA signature may be re-encoded by someone who has seen it.
final class AcpIdentityId implements Comparable<AcpIdentityId> {
  AcpIdentityId._(this.bytes);

  /// The id of the public key whose encoding is [spki].
  factory AcpIdentityId.of(List<int> spki) => AcpIdentityId._(_sha256(spki));

  /// Reads `sha256:` and 64 hexadecimal digits, either case.
  factory AcpIdentityId.parse(String text) {
    final failure = _fail(
      AcpSecretFailure.malformed,
      'an identity is sha256: and 64 hexadecimal digits',
    );
    if (!text.startsWith('sha256:')) throw failure;
    final hex = text.substring(7);
    if (!RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(hex)) throw failure;
    return AcpIdentityId._(_unhex(hex.toLowerCase()));
  }

  final Uint8List bytes;

  @override
  String toString() => 'sha256:${_hex(bytes)}';

  @override
  bool operator ==(Object other) =>
      other is AcpIdentityId && _sameBytes(bytes, other.bytes);

  @override
  int get hashCode => Object.hashAll(bytes);

  @override
  int compareTo(AcpIdentityId other) => toString().compareTo(other.toString());
}

/// Which of the extension's two methods a request is; it is bound into every sealed field and key binding.
enum AcpSecretMethod {
  request('request'),
  show('show');

  const AcpSecretMethod(this.text);
  final String text;
}

/// What the person is asked or shown, exactly as the host sent it. A binding and every sealed field cover the digest of these bytes,
/// so a form changed on the way fails the binding instead of being drawn.
final class AcpSecretContext {
  const AcpSecretContext({
    required this.sessionId,
    required this.requestId,
    required this.message,
    required this.schema,
    this.form,
    this.values,
  });

  final String sessionId, requestId, message;

  /// The questions as the JSON Schema text that was sent: these exact bytes are hashed, never a re-serialization.
  final String schema;

  /// The layout as the JSON text that was sent, when there is one.
  final String? form;

  /// For a show: the values that are not secret, as the JSON object text that was sent, covered like the rest.
  final String? values;

  /// SHA-256 of the method and every part, each with a 4-byte length, a byte that says whether a layout was sent and one that says
  /// whether plain values were.
  Uint8List digest(AcpSecretMethod method) => _sha256(
    _concat([
      _lp(ascii.encode('dev.boriken.coqui.acp/secret/v1 context')),
      _lp(ascii.encode(method.text)),
      _lp(_utf8(sessionId)),
      _lp(_utf8(requestId)),
      _lp(_utf8(message)),
      _lp(_utf8(schema)),
      _lp([if (form != null) 1 else 0]),
      _lp(_utf8(form ?? '')),
      _lp([if (values != null) 1 else 0]),
      _lp(_utf8(values ?? '')),
    ]),
  );
}

/// A key binding as it travels.
final class AcpSecretBinding {
  const AcpSecretBinding({
    required this.method,
    required this.key,
    required this.expires,
    required this.audience,
    required this.proof,
  });

  /// [acpBindingEcdsa], [acpBindingDh] or [acpBindingLink].
  final String method;

  /// The holder's `SubjectPublicKeyInfo` (for [acpBindingLink], its role's 32-byte tag).
  final Uint8List key;

  /// When it ends, in Unix seconds.
  final int expires;

  /// The identity it was made for: a verifier refuses a binding made for anyone but itself.
  final AcpIdentityId audience;

  /// The signature or the tag.
  final Uint8List proof;

  Map<String, Object?> toJson() => {
    'method': method,
    'key': _b64u(key),
    'expires': expires,
    'audience': audience.toString(),
    'proof': _b64u(proof),
  };

  factory AcpSecretBinding.fromJson(Object? json) {
    if (json is! Map) {
      throw _fail(AcpSecretFailure.malformed, 'a binding is not an object');
    }
    String need(String key, String why) => json[key] is String
        ? json[key] as String
        : throw _fail(AcpSecretFailure.malformed, why);
    final expires = json['expires'];
    if (expires is! int || expires < 0) {
      throw _fail(AcpSecretFailure.malformed, 'a binding has no end');
    }
    return AcpSecretBinding(
      method: need('method', 'a binding has no method'),
      key: _b64uDecode(need('key', 'a binding has no key')),
      expires: expires,
      audience: AcpIdentityId.parse(
        need('audience', 'a binding has no audience'),
      ),
      proof: _b64uDecode(need('proof', 'a binding has no proof')),
    );
  }
}

/// A sealed field as it travels: HPKE's encapsulated key and the ciphertext with its tag.
final class AcpSealed {
  const AcpSealed(this.enc, this.ct);

  /// The encapsulated key, 32 bytes.
  final Uint8List enc;

  /// The ciphertext followed by its 16-byte tag.
  final Uint8List ct;

  Map<String, Object?> toJson() => {'enc': _b64u(enc), 'ct': _b64u(ct)};

  factory AcpSealed.fromJson(Object? json) {
    if (json is! Map ||
        json.length != 2 ||
        json['enc'] is! String ||
        json['ct'] is! String) {
      throw _fail(
        AcpSecretFailure.malformed,
        'a sealed value is not {enc, ct}',
      );
    }
    final enc = _b64uDecode(json['enc'] as String);
    final ct = _b64uDecode(json['ct'] as String);
    if (ct.length < 16) {
      throw _fail(
        AcpSecretFailure.malformed,
        'a sealed value is shorter than its tag',
      );
    }
    if (enc.length != 32) {
      throw _fail(
        AcpSecretFailure.malformed,
        "a sealed value's key is not 32 bytes",
      );
    }
    return AcpSealed(enc, ct);
  }
}

/// The receiver's per-request public key with its binding.
final class AcpSecretRecipient {
  const AcpSecretRecipient(this.key, this.binding);

  final Uint8List key;
  final AcpSecretBinding binding;

  Map<String, Object?> toJson() => {
    'key': _b64u(key),
    'binding': binding.toJson(),
  };

  factory AcpSecretRecipient.fromJson(Object? json) {
    if (json is! Map || json['key'] is! String || json['binding'] == null) {
      throw _fail(
        AcpSecretFailure.malformed,
        'a recipient has no key or binding',
      );
    }
    final key = _b64uDecode(json['key'] as String);
    if (key.length != 32) {
      throw _fail(
        AcpSecretFailure.malformed,
        "a recipient's key is not 32 bytes",
      );
    }
    return AcpSecretRecipient(key, AcpSecretBinding.fromJson(json['binding']));
  }
}

/// A secret value held for its use. Dart cannot zero memory it has handed to the garbage collector, so [clear] overwrites the bytes
/// this object holds, and nothing prints it.
final class AcpSecretValue {
  AcpSecretValue(this._bytes);

  final Uint8List _bytes;

  /// The value as bytes.
  Uint8List get bytes => _bytes;

  /// The value as text; it was checked to be UTF-8 when it was opened.
  String get text => utf8.decode(_bytes);

  /// Overwrites the held bytes.
  void clear() => _bytes.fillRange(0, _bytes.length, 0);

  @override
  String toString() => 'AcpSecretValue(..)';
}

/// What a receiver gets back: the plain answers, and each secret one opened.
final class AcpOpened {
  const AcpOpened(this.plain, this.secrets);

  final Map<String, Object?> plain;
  final Map<String, AcpSecretValue> secrets;
}

// ---- Statements.

enum _Kind { key, answer }

Uint8List _statement(
  _Kind kind,
  AcpSecretMethod method,
  String requestId,
  List<int> digest,
  List<int> key,
  List<int> payload,
  int expires,
  AcpIdentityId audience,
) => _concat([
  _lp(
    ascii.encode(
      kind == _Kind.key
          ? 'dev.boriken.coqui.acp/secret/v1 binding'
          : 'dev.boriken.coqui.acp/secret/v1 values',
    ),
  ),
  _lp(ascii.encode(method.text)),
  _lp(_utf8(requestId)),
  _lp(digest),
  _lp(key),
  _lp(payload),
  _lp(_u64(expires)),
  _lp(audience.bytes),
]);

/// The plain values of an answer or a show, from the JSON object text that was sent.
Map<String, Object?> _plainValues(String text) {
  final value = _strictJson(
    text,
    AcpSecretFailure.malformed,
    'the plain values are not a JSON object',
    'the plain values repeat a key',
  );
  if (value is! Map) {
    throw _fail(
      AcpSecretFailure.malformed,
      'the plain values are not a JSON object',
    );
  }
  return value.cast<String, Object?>();
}

/// The digest of a whole answer: the plain values as the JSON object text that was sent, then the sealed values in the byte order of
/// their names. It is what the sender's binding covers.
Uint8List _valuesDigest(String plain, Map<String, AcpSealed> sealed) {
  final names = sealed.keys.toList()
    ..sort((a, b) {
      final x = _utf8(a), y = _utf8(b);
      for (var i = 0; i < x.length && i < y.length; i++) {
        if (x[i] != y[i]) return x[i] - y[i];
      }
      return x.length - y.length;
    });
  final label = ascii.encode('dev.boriken.coqui.acp/secret/v1 values digest');
  return _sha256(
    _concat([
      (ByteData(4)..setUint32(0, label.length)).buffer.asUint8List(),
      label,
      (ByteData(4)..setUint32(0, _utf8(plain).length)).buffer.asUint8List(),
      _utf8(plain),
      for (final name in names) ...[
        (ByteData(4)..setUint32(0, _utf8(name).length)).buffer.asUint8List(),
        _utf8(name),
        sealed[name]!.enc,
        (ByteData(
          4,
        )..setUint32(0, sealed[name]!.ct.length)).buffer.asUint8List(),
        sealed[name]!.ct,
      ],
    ]),
  );
}

Uint8List _fieldInfo(
  AcpSecretMethod method,
  String requestId,
  List<int> digest,
  String field,
) => _concat([
  _lp(ascii.encode('dev.boriken.coqui.acp/secret/v1 field')),
  _lp(ascii.encode(method.text)),
  _lp(_utf8(requestId)),
  _lp(digest),
  _lp(_utf8(field)),
]);

/// What a context must be besides its id and its schema: a layout that changes nothing a secret question is, and plain values only in a
/// show, which fit the form.
void _checkContext(
  AcpSecretMethod method,
  AcpSecretContext context,
  AcpSecretForm form,
) {
  if (context.form != null) form.checkLayout(context.form!);
  switch ((method, context.values)) {
    case (AcpSecretMethod.show, final String values):
      form.checkPlain(_plainValues(values));
    case (AcpSecretMethod.show, null):
      throw _fail(AcpSecretFailure.malformed, 'a show has no values');
    case (AcpSecretMethod.request, final String _):
      throw _fail(AcpSecretFailure.malformed, 'a request carries no values');
    case (AcpSecretMethod.request, null):
      break;
  }
}

void _checkRequestId(String requestId) {
  if (requestId.isEmpty ||
      _utf8(requestId).length > 128 ||
      requestId.runes.any(
        (rune) => rune < 0x20 || (rune >= 0x7f && rune < 0xa0),
      )) {
    throw _fail(
      AcpSecretFailure.malformed,
      'a request id is 1 to 128 bytes with no control character',
    );
  }
}

// ---- Bindings.

final _p256 = pc.ECDomainParameters('prime256v1');
final _p256SpkiPrefix = _unhex(
  '3059301306072a8648ce3d020106082a8648ce3d030107034200',
);
final _x25519SpkiPrefix = _unhex('302a300506032b656e032100');
final _dhLabel = ascii.encode('dev.boriken.coqui.acp/secret/v1 dh');
final _linkMacLabel = ascii.encode('dev.boriken.coqui.acp/secret/v1 link mac');
final _linkRoleLabel = ascii.encode(
  'dev.boriken.coqui.acp/secret/v1 link role',
);

Uint8List _bigBytes(BigInt value) {
  final out = Uint8List(32);
  for (var i = 31; i >= 0; i--) {
    out[i] = (value & BigInt.from(0xff)).toInt();
    value = value >> 8;
  }
  return out;
}

BigInt _bigOf(List<int> bytes) =>
    bytes.fold(BigInt.zero, (value, byte) => (value << 8) | BigInt.from(byte));

Uint8List _x25519OfSpki(List<int> spki) {
  if (spki.length != 44 ||
      !_sameBytes(spki.sublist(0, 12), _x25519SpkiPrefix)) {
    throw _fail(AcpSecretFailure.key, 'not an X25519 SubjectPublicKeyInfo');
  }
  return Uint8List.fromList(spki.sublist(12));
}

pc.ECPoint _p256PointOfSpki(List<int> spki) {
  final point =
      spki.length == 91 && _sameBytes(spki.sublist(0, 26), _p256SpkiPrefix)
      ? _p256.curve.decodePoint(spki.sublist(26))
      : null;
  if (point == null) {
    throw _fail(AcpSecretFailure.key, 'not a P-256 SubjectPublicKeyInfo');
  }
  return point;
}

/// The Diffie-Hellman of the identity's static key and a peer's, from the caller's key agreement; a refusal of the suite is a key
/// failure.
Future<Uint8List> _agreeChecked(
  AcpSecretAgreement agree,
  Uint8List peer,
) async {
  final shared = await agree(peer);
  if (shared.length != 32 || shared.every((byte) => byte == 0)) {
    throw _fail(AcpSecretFailure.key, 'a Diffie-Hellman key of low order');
  }
  return shared;
}

Future<Uint8List> _dhMacKey(
  Uint8List shared,
  List<int> holder,
  List<int> collector,
) async {
  final prk = await hpke.hmacSha256(Uint8List(32), shared);
  final key = await hpke.expandOne(prk, [
    ..._dhLabel,
    ...holder,
    ...collector,
  ], 32);
  prk.fillRange(0, prk.length, 0);
  shared.fillRange(0, shared.length, 0);
  return key;
}

/// Which end of the in-process link a party is: the host that runs the agent, or the application in the same process.
enum AcpLinkRole {
  host,
  app;

  AcpLinkRole get other => this == host ? app : host;
}

/// What the two ends of the in-process link derive from the secret they share: the MAC key and each role's public tag, which names the
/// role as an identity (the SHA-256 of the 32-byte tag). The tags and the key come from one HKDF-SHA-256 extraction under different
/// labels, so a tag shows nothing of the key. The secret is minted by the host, held in memory, and sent over no wire and to no agent;
/// this library only derives from it and does not keep it.
final class AcpLinkKey {
  AcpLinkKey._(this._mac, this._host, this._app);

  /// Derives the link's keys from its [secret], at least 16 bytes.
  static Future<AcpLinkKey> derive(List<int> secret) async {
    if (secret.length < 16) {
      throw _fail(AcpSecretFailure.key, 'a link secret is at least 16 bytes');
    }
    final prk = await hpke.hmacSha256(Uint8List(32), secret);
    final key = AcpLinkKey._(
      await hpke.expandOne(prk, _linkMacLabel, 32),
      await hpke.expandOne(prk, [
        ..._linkRoleLabel,
        ...ascii.encode('host'),
      ], 32),
      await hpke.expandOne(prk, [
        ..._linkRoleLabel,
        ...ascii.encode('app'),
      ], 32),
    );
    prk.fillRange(0, prk.length, 0);
    return key;
  }

  final Uint8List _mac;
  final Uint8List _host;
  final Uint8List _app;

  Uint8List _tag(AcpLinkRole role) => role == AcpLinkRole.host ? _host : _app;

  /// The identity of a role on this link, which a verifier for that role is built for.
  AcpIdentityId identity(AcpLinkRole role) => AcpIdentityId.of(_tag(role));
}

/// Signs a message with an identity's P-256 key: the 64-byte IEEE P1363 signature (ECDSA, SHA-256), deterministic or randomized, with
/// `s` high or low. The key can stay in a platform key store or in WebCrypto.
typedef AcpSecretSigner = Future<Uint8List> Function(Uint8List message);

/// The X25519 Diffie-Hellman of an identity's static key and a peer's public key, 32 bytes.
typedef AcpSecretAgreement = Future<Uint8List> Function(Uint8List peerPublic);

/// The holder's side of a binding: an identity that signs or agrees on keys through functions the caller supplies, so its private key
/// never enters this library. Raw keys held in memory are made by `package:coqui_acp/secret_unprotected.dart`, for tests and tools.
final class AcpSecretProver {
  /// An identity that signs; [spki] is its P-256 `SubjectPublicKeyInfo`.
  AcpSecretProver.signing({
    required List<int> spki,
    required AcpSecretSigner sign,
  }) : spki = Uint8List.fromList(spki),
       _sign = sign,
       _agree = null,
       _collector = null,
       _audience = null,
       _link = null {
    _p256PointOfSpki(spki);
  }

  /// An X25519 identity for the one collector whose `SubjectPublicKeyInfo` is [collectorSpki]; [agree] is its key agreement.
  AcpSecretProver.agreeing({
    required List<int> spki,
    required AcpSecretAgreement agree,
    required List<int> collectorSpki,
  }) : spki = Uint8List.fromList(spki),
       _sign = null,
       _agree = agree,
       _collector = _x25519OfSpki(collectorSpki),
       _audience = AcpIdentityId.of(collectorSpki),
       _link = null {
    _x25519OfSpki(spki);
  }

  /// The [role] end of the in-process link whose secret [link] was derived from, binding for the other end only.
  AcpSecretProver.linked(AcpLinkKey link, AcpLinkRole role)
    : spki = Uint8List.fromList(link._tag(role)),
      _sign = null,
      _agree = null,
      _collector = null,
      _audience = link.identity(role.other),
      _link = link._mac;

  /// The identity's public key as `SubjectPublicKeyInfo`.
  final Uint8List spki;
  final AcpSecretSigner? _sign;
  final AcpSecretAgreement? _agree;
  final Uint8List? _collector;
  final AcpIdentityId? _audience;
  final Uint8List? _link;

  /// The identity's id.
  AcpIdentityId get id => AcpIdentityId.of(spki);

  Future<AcpSecretBinding> _bind(
    _Kind kind,
    AcpSecretMethod method,
    String requestId,
    List<int> digest,
    List<int> key,
    List<int> payload,
    int expires,
    AcpIdentityId audience,
  ) async {
    final statement = _statement(
      kind,
      method,
      requestId,
      digest,
      key,
      payload,
      expires,
      audience,
    );
    if (_link != null) {
      if (audience != _audience) throw _fail(AcpSecretFailure.wrongAudience);
      return AcpSecretBinding(
        method: acpBindingLink,
        key: spki,
        expires: expires,
        audience: audience,
        proof: await hpke.hmacSha256(_link, statement),
      );
    }
    if (_sign != null) {
      final signature = await _sign(statement);
      if (signature.length != 64) {
        throw _fail(AcpSecretFailure.seal, 'a signature is not 64 bytes');
      }
      final r = _bigOf(signature.sublist(0, 32));
      var s = _bigOf(signature.sublist(32));
      // Low s, so one statement has one signature; a verifier refuses the other.
      if (s > _p256.n >> 1) s = _p256.n - s;
      return AcpSecretBinding(
        method: acpBindingEcdsa,
        key: spki,
        expires: expires,
        audience: audience,
        proof: _concat([_bigBytes(r), _bigBytes(s)]),
      );
    }
    if (audience != _audience) {
      throw _fail(AcpSecretFailure.wrongAudience);
    }
    final shared = await _agreeChecked(_agree!, _collector!);
    final mac = await _dhMacKey(shared, _x25519OfSpki(spki), _collector);
    final proof = await hpke.hmacSha256(mac, statement);
    mac.fillRange(0, mac.length, 0);
    return AcpSecretBinding(
      method: acpBindingDh,
      key: spki,
      expires: expires,
      audience: audience,
      proof: proof,
    );
  }
}

/// The collector's side: the identities it knows by pairing or pinning, its own id, its key agreement when it takes [acpBindingDh], and
/// the link's key when it is an end of the in-process link and takes [acpBindingLink].
final class AcpSecretVerifier {
  /// A verifier that is [own], knows [known], and accepts a binding only if it was made for [own]. [agreementSpki] and [agree] are
  /// its own X25519 identity, which must be [own]; without them [acpBindingDh] is refused. [link] and [linkRole] make it the
  /// [linkRole] end of the in-process link, which knows the other end and takes [acpBindingLink] from it; give them only on the
  /// in-process link: a link with a relay never takes the method.
  AcpSecretVerifier({
    required this.own,
    Iterable<AcpIdentityId> known = const [],
    List<int>? agreementSpki,
    AcpSecretAgreement? agree,
    AcpLinkKey? link,
    AcpLinkRole? linkRole,
    this.maxLifetime = const Duration(minutes: 5),
    this.skew = const Duration(seconds: 30),
    this.limits = const AcpSecretLimits(),
  }) : _known = {
         for (final id in known) id.toString(),
         if (link != null && linkRole != null)
           link.identity(linkRole.other).toString(),
       },
       _agree = agree,
       _linkMac = link?._mac,
       _linkPeer = link == null || linkRole == null
           ? null
           : Uint8List.fromList(link._tag(linkRole.other)),
       _public = agreementSpki == null ? null : _x25519OfSpki(agreementSpki) {
    if ((link == null) != (linkRole == null)) {
      throw _fail(
        AcpSecretFailure.key,
        'a link needs both its key and its role',
      );
    }
    if (link != null && link.identity(linkRole!) != own) {
      throw _fail(
        AcpSecretFailure.key,
        "the link is not this verifier's identity",
      );
    }
    if ((agreementSpki == null) != (agree == null)) {
      throw _fail(
        AcpSecretFailure.key,
        'a key agreement needs both its identity and its function',
      );
    }
    if (agreementSpki != null && AcpIdentityId.of(agreementSpki) != own) {
      throw _fail(
        AcpSecretFailure.key,
        "the key agreement is not this verifier's identity",
      );
    }
  }

  /// This verifier's own identity.
  final AcpIdentityId own;
  final Set<String> _known;
  final AcpSecretAgreement? _agree;
  final Uint8List? _public;
  final Uint8List? _linkMac;
  final Uint8List? _linkPeer;

  /// The longest a binding may last from now.
  final Duration maxLifetime;

  /// How far the peer's clock may differ from this one's: a binding is taken that long after it ended and that much beyond the longest
  /// it may last.
  final Duration skew;

  /// What forms this verifier takes.
  final AcpSecretLimits limits;

  /// Knows an identity by its id.
  void trust(AcpIdentityId id) => _known.add(id.toString());

  /// Knows an identity by its `SubjectPublicKeyInfo`.
  void trustSpki(List<int> spki) => trust(AcpIdentityId.of(spki));

  /// The order is the Rust crate's: method and key shape, whether the holder is known, the end, the audience, last the proof.
  Future<AcpIdentityId> _verify(
    _Kind kind,
    AcpSecretMethod method,
    String requestId,
    List<int> digest,
    List<int> key,
    List<int> payload,
    AcpSecretBinding binding,
    int now,
  ) async {
    final ecdsa = switch (binding.method) {
      acpBindingEcdsa => true,
      acpBindingDh || acpBindingLink => false,
      _ => throw _fail(
        AcpSecretFailure.unsupported,
        'a binding method this side does not have',
      ),
    };
    final point = ecdsa ? _p256PointOfSpki(binding.key) : null;
    if (binding.method == acpBindingLink) {
      if (binding.key.length != 32) {
        throw _fail(AcpSecretFailure.key, 'a link tag is 32 bytes');
      }
    } else if (!ecdsa) {
      _x25519OfSpki(binding.key);
    }
    final holder = AcpIdentityId.of(binding.key);
    if (!_known.contains(holder.toString())) {
      throw _fail(AcpSecretFailure.unknownIdentity);
    }
    final slack = skew.inSeconds;
    if (now > binding.expires + slack ||
        binding.expires > now + maxLifetime.inSeconds + slack) {
      throw _fail(AcpSecretFailure.expired);
    }
    if (binding.audience != own) throw _fail(AcpSecretFailure.wrongAudience);
    final signed = _statement(
      kind,
      method,
      requestId,
      digest,
      key,
      payload,
      binding.expires,
      binding.audience,
    );
    if (ecdsa) {
      if (binding.proof.length != 64) throw _fail(AcpSecretFailure.badProof);
      final r = _bigOf(binding.proof.sublist(0, 32));
      final s = _bigOf(binding.proof.sublist(32));
      // One statement has one signature: a high s is the same signature written the other way.
      if (s > _p256.n >> 1) throw _fail(AcpSecretFailure.badProof);
      final verifier = pc.ECDSASigner(pc.SHA256Digest())
        ..init(false, pc.PublicKeyParameter(pc.ECPublicKey(point!, _p256)));
      if (!verifier.verifySignature(signed, pc.ECSignature(r, s))) {
        throw _fail(AcpSecretFailure.badProof);
      }
    } else if (binding.method == acpBindingLink) {
      if (_linkMac == null) {
        throw _fail(
          AcpSecretFailure.unsupported,
          'this collector is not an end of the in-process link',
        );
      }
      if (!_sameBytes(binding.key, _linkPeer!)) {
        throw _fail(AcpSecretFailure.unknownIdentity);
      }
      final tag = await hpke.hmacSha256(_linkMac, signed);
      if (!_sameBytes(tag, binding.proof)) {
        throw _fail(AcpSecretFailure.badProof);
      }
    } else {
      if (_agree == null) {
        throw _fail(
          AcpSecretFailure.unsupported,
          'this collector has no X25519 identity of its own',
        );
      }
      final holderKey = _x25519OfSpki(binding.key);
      final shared = await _agreeChecked(_agree, holderKey);
      final mac = await _dhMacKey(shared, holderKey, _public!);
      final tag = await hpke.hmacSha256(mac, signed);
      mac.fillRange(0, mac.length, 0);
      if (!_sameBytes(tag, binding.proof)) {
        throw _fail(AcpSecretFailure.badProof);
      }
    }
    return holder;
  }

  /// Checks what the person is asked and the receiver's key and its binding at [now] (Unix seconds), and returns the collector that
  /// seals to it. The binding covers the digest of [context] as this side received it, so a form changed on the way is refused here
  /// before it is drawn. Only [expected], the identity this request must come from, is taken: another that this verifier knows is
  /// refused.
  Future<AcpSecretCollector> collector(
    AcpIdentityId expected,
    AcpSecretMethod method,
    AcpSecretContext context,
    AcpSecretRecipient recipient,
    int now,
  ) async {
    _checkRequestId(context.requestId);
    final digest = context.digest(method);
    final holder = await _verify(
      _Kind.key,
      method,
      context.requestId,
      digest,
      recipient.key,
      const [],
      recipient.binding,
      now,
    );
    if (holder != expected) throw _fail(AcpSecretFailure.unknownIdentity);
    final form = AcpSecretForm.parseText(context.schema, limits);
    _checkContext(method, context, form);
    return AcpSecretCollector._(
      method,
      context.requestId,
      digest,
      form,
      recipient.key,
      holder,
    );
  }
}

/// The collecting side of one request, after it checked the receiver's key.
final class AcpSecretCollector {
  AcpSecretCollector._(
    this.method,
    this.requestId,
    this._digest,
    this.form,
    this._key,
    this.holder,
  );

  final AcpSecretMethod method;
  final String requestId;
  final Uint8List _digest;

  /// The form this collector draws or sends, read from the context its binding covers.
  final AcpSecretForm form;
  final Uint8List _key;

  /// The identity that stands behind the key.
  final AcpIdentityId holder;

  /// Whether the field is one this collector would seal.
  bool isSecret(String name) => form.field(name)?.secret ?? false;

  /// Seals the value of secret field [name]: UTF-8 text, or a number as decimal text. The ciphertext is as long as the value plus 48
  /// bytes (the encapsulated key and the tag), so its length tells a relay how long the secret is; nothing is padded.
  Future<AcpSealed> seal(String name, String value) {
    final source = Random.secure();
    final ikm = Uint8List.fromList([
      for (var i = 0; i < 32; i++) source.nextInt(256),
    ]);
    return _seal(name, value, ikm);
  }

  Future<AcpSealed> _seal(String name, String value, List<int> ikm) async {
    final bytes = _utf8(value);
    try {
      form.checkSecret(name, bytes);
      final made = await hpke.hpkeSeal(
        _key,
        _fieldInfo(method, requestId, _digest, name),
        const [],
        bytes,
        ikm,
      );
      return AcpSealed(made.enc, made.ct);
    } on hpke.HpkeFailure {
      throw _fail(AcpSecretFailure.seal);
    } finally {
      bytes.fillRange(0, bytes.length, 0);
    }
  }

  /// Binds the whole answer to [prover]'s identity for this request and key until [now] plus [lifetime]: what the receiver verifies
  /// before it reads or opens anything. [plain] is the JSON object text of the plain values exactly as it will be sent (for a show, the
  /// open step's values), and [sealed] is every sealed value of the answer.
  Future<AcpSecretBinding> authenticate(
    AcpSecretProver prover,
    String plain,
    Map<String, AcpSealed> sealed,
    int now, {
    Duration lifetime = acpSecretDefaultLifetime,
  }) {
    // A sender signs only an answer the form allows: a plain value for a secret question, or a required secret left out, is refused
    // here, before anything is signed.
    form.checkPlain(_plainValues(plain));
    form.checkSealed(sealed.keys);
    return prover._bind(
      _Kind.answer,
      method,
      requestId,
      _digest,
      _key,
      _valuesDigest(plain, sealed),
      now + lifetime.inSeconds,
      holder,
    );
  }
}

/// The receiving side of one request: it holds the per-request private key and gives the collector the public key with a binding.
/// It opens one answer: [openValues] consumes it. Whoever owns a receiver remembers the request id for as long as a binding can
/// verify (the verifier's `maxLifetime` and `skew`) so that the same request is not answered twice.
final class AcpSecretReceiver {
  AcpSecretReceiver._(
    this.method,
    this.requestId,
    this._digest,
    this.form,
    this._secret,
    this.recipient,
  );

  /// Makes a key pair for one request and binds the public key to [prover]'s identity, for what [context] asks, until [now] plus
  /// [lifetime] (Unix seconds), for the collector [audience].
  static Future<AcpSecretReceiver> create(
    AcpSecretMethod method,
    AcpSecretContext context,
    AcpSecretProver prover,
    AcpIdentityId audience,
    int now, {
    Duration lifetime = acpSecretDefaultLifetime,
    AcpSecretLimits limits = const AcpSecretLimits(),
  }) {
    final source = Random.secure();
    final ikm = Uint8List.fromList([
      for (var i = 0; i < 32; i++) source.nextInt(256),
    ]);
    return _fromIkm(
      method,
      context,
      prover,
      audience,
      now,
      lifetime,
      limits,
      ikm,
    );
  }

  static Future<AcpSecretReceiver> _fromIkm(
    AcpSecretMethod method,
    AcpSecretContext context,
    AcpSecretProver prover,
    AcpIdentityId audience,
    int now,
    Duration lifetime,
    AcpSecretLimits limits,
    List<int> ikm,
  ) async {
    _checkRequestId(context.requestId);
    final form = AcpSecretForm.parseText(context.schema, limits);
    _checkContext(method, context, form);
    final digest = context.digest(method);
    final secret = await hpke.deriveKey(ikm);
    final publicKey = await hpke.x25519Public(secret);
    final binding = await prover._bind(
      _Kind.key,
      method,
      context.requestId,
      digest,
      publicKey,
      const [],
      now + lifetime.inSeconds,
      audience,
    );
    return AcpSecretReceiver._(
      method,
      context.requestId,
      digest,
      form,
      secret,
      AcpSecretRecipient(publicKey, binding),
    );
  }

  final AcpSecretMethod method;
  final String requestId;
  final Uint8List _digest;
  final AcpSecretForm form;
  final Uint8List _secret;
  var _used = false;

  /// What to send the collector.
  final AcpSecretRecipient recipient;

  /// Overwrites the private key this receiver holds. [openValues] does it.
  void clear() => _secret.fillRange(0, _secret.length, 0);

  Future<AcpSecretValue> _open(String name, AcpSealed sealed) async {
    if (!(form.field(name)?.secret ?? false)) {
      throw _fail(AcpSecretFailure.field, 'not a secret question of the form');
    }
    final Uint8List plain;
    try {
      plain = await hpke.hpkeOpen(
        _secret,
        _fieldInfo(method, requestId, _digest, name),
        const [],
        hpke.HpkeSealed(sealed.enc, sealed.ct),
      );
    } on hpke.HpkeFailure {
      throw _fail(
        AcpSecretFailure.open,
        'the value cannot be opened for this request and field',
      );
    }
    try {
      form.checkSecret(name, plain);
    } on Object {
      plain.fillRange(0, plain.length, 0);
      rethrow;
    }
    return AcpSecretValue(plain);
  }

  /// Opens an answer, once: checks that [sender], an identity [verifier] knows, sent this answer ([auth] binds the plain values, as
  /// the JSON object text that was sent, and the sealed ones, to this request and key), checks the plain values against the form, and
  /// opens every sealed one. Nothing is read or opened before the sender is verified. For a show, [plain] is the text of the open
  /// step's values. The private key is overwritten whatever the outcome.
  Future<AcpOpened> openValues(
    AcpSecretVerifier verifier,
    AcpIdentityId sender,
    String plain,
    Map<String, AcpSealed> sealed,
    AcpSecretBinding auth,
    int now,
  ) async {
    if (_used) {
      throw _fail(
        AcpSecretFailure.open,
        'this receiver already opened an answer',
      );
    }
    _used = true;
    final secrets = <String, AcpSecretValue>{};
    try {
      // The binding it made is over; a later answer is refused whatever its sender proves.
      if (now > recipient.binding.expires + verifier.skew.inSeconds) {
        throw _fail(AcpSecretFailure.expired);
      }
      form.checkSealed(sealed.keys);
      final holder = await verifier._verify(
        _Kind.answer,
        method,
        requestId,
        _digest,
        recipient.key,
        _valuesDigest(plain, sealed),
        auth,
        now,
      );
      if (holder != sender) throw _fail(AcpSecretFailure.unknownIdentity);
      final values = _plainValues(plain);
      form.checkPlain(values);
      for (final MapEntry(:key, :value) in sealed.entries) {
        secrets[key] = await _open(key, value);
      }
      return AcpOpened(values, secrets);
    } on Object {
      for (final secret in secrets.values) {
        secret.clear();
      }
      rethrow;
    } finally {
      clear();
    }
  }
}

// ---- Messages.

/// What a frontend offers under [acpSecretsCapability].
final class AcpSecretsCapability {
  const AcpSecretsCapability({
    this.request = true,
    this.show = true,
    this.bindings = const [acpBindingEcdsa, acpBindingDh],
  });

  final bool request, show;
  final List<String> bindings;

  Map<String, Object?> toJson() => {
    'version': acpSecretsVersion,
    'methods': [if (request) 'request', if (show) 'show'],
    'bindings': bindings,
  };

  /// Reads the capability from a `_meta` object, or null when it is absent, of another version, or offers no method: a host then
  /// serves no secret tools to that client.
  static AcpSecretsCapability? fromMeta(Map<String, Object?> meta) {
    final map = meta[acpSecretsCapability];
    if (map is! Map || map['version'] != acpSecretsVersion) return null;
    final methods = map['methods'];
    if (methods is! List) return null;
    final bindings = map['bindings'];
    final capability = AcpSecretsCapability(
      request: methods.contains('request'),
      show: methods.contains('show'),
      bindings: bindings is List
          ? bindings.whereType<String>().toList()
          : const [],
    );
    return capability.request || capability.show ? capability : null;
  }
}

Map<String, Object?> _mapOf(Object? value, String why) => value is Map
    ? value.cast<String, Object?>()
    : throw _fail(AcpSecretFailure.malformed, why);

String _textOf(Map map, String key, String why) => map[key] is String
    ? map[key] as String
    : throw _fail(AcpSecretFailure.malformed, why);

AcpSecretContext _contextOf(Map map) => AcpSecretContext(
  sessionId: _textOf(map, 'sessionId', 'the message has no session'),
  requestId: _textOf(map, 'requestId', 'the message has no id'),
  message: _textOf(map, 'message', 'the message has no message'),
  schema: _textOf(map, 'schema', 'the schema is not JSON text'),
  form: switch (map['form']) {
    null => null,
    final String form => form,
    _ => throw _fail(
      AcpSecretFailure.malformed,
      'the form overlay is not JSON text',
    ),
  },
  values: switch (map['values']) {
    null => null,
    final String values => values,
    _ => throw _fail(
      AcpSecretFailure.malformed,
      'the values are not JSON text',
    ),
  },
);

Map<String, Object?> _contextJson(AcpSecretContext context) => {
  'sessionId': context.sessionId,
  'requestId': context.requestId,
  'message': context.message,
  'schema': context.schema,
  if (context.form != null) 'form': context.form,
  if (context.values != null) 'values': context.values,
};

/// [acpSecretRequestMethod]'s parameters. The schema and the layout are JSON text, and these exact bytes are what the receiver's
/// binding covers.
final class AcpSecretRequestParams {
  const AcpSecretRequestParams({
    required this.context,
    required this.recipient,
  });

  final AcpSecretContext context;
  final AcpSecretRecipient recipient;

  Map<String, Object?> toJson() => {
    ..._contextJson(context),
    'recipient': recipient.toJson(),
  };

  factory AcpSecretRequestParams.fromJson(Object? json) {
    final map = _mapOf(json, 'the request is not an object');
    return AcpSecretRequestParams(
      context: _contextOf(map),
      recipient: AcpSecretRecipient.fromJson(map['recipient']),
    );
  }
}

/// The person's answer to a request.
sealed class AcpSecretRequestReply {
  const AcpSecretRequestReply();

  Map<String, Object?> toJson();

  /// Reads an answer; [isSecret] says which fields are secret (the form's).
  factory AcpSecretRequestReply.fromJson(
    Object? json,
    bool Function(String name) isSecret,
  ) {
    final map = _mapOf(json, 'the answer is not an object');
    switch (map['action']) {
      case 'decline':
        return const AcpSecretDeclined();
      case 'cancel':
        return const AcpSecretCancelled();
      case 'accept':
        final values = _mapOf(map['values'], 'the values are not an object');
        final sealed = <String, AcpSealed>{};
        for (final MapEntry(:key, :value) in values.entries) {
          if (!isSecret(key)) {
            throw _fail(
              AcpSecretFailure.malformed,
              'a plain value outside the plain text',
            );
          }
          sealed[key] = AcpSealed.fromJson(value);
        }
        return AcpSecretAccepted(
          _textOf(map, 'plain', 'the answer has no plain values'),
          sealed,
          AcpSecretBinding.fromJson(map['auth']),
        );
    }
    throw _fail(
      AcpSecretFailure.malformed,
      "the answer's action is not accept, decline or cancel",
    );
  }
}

final class AcpSecretAccepted extends AcpSecretRequestReply {
  const AcpSecretAccepted(this.plain, this.sealed, this.auth);

  /// The plain answers as the JSON object text that is sent: these exact bytes are what [auth] covers.
  final String plain;
  final Map<String, AcpSealed> sealed;

  /// The sender's binding of the whole answer, from [AcpSecretCollector.authenticate].
  final AcpSecretBinding auth;

  @override
  Map<String, Object?> toJson() => {
    'action': 'accept',
    'plain': plain,
    'values': {
      for (final MapEntry(:key, :value) in sealed.entries) key: value.toJson(),
    },
    'auth': auth.toJson(),
  };
}

final class AcpSecretDeclined extends AcpSecretRequestReply {
  const AcpSecretDeclined();

  @override
  Map<String, Object?> toJson() => {'action': 'decline'};
}

final class AcpSecretCancelled extends AcpSecretRequestReply {
  const AcpSecretCancelled();

  @override
  Map<String, Object?> toJson() => {'action': 'cancel'};
}

/// [acpSecretShowOpenMethod]'s parameters.
final class AcpSecretShowOpen {
  const AcpSecretShowOpen({required this.context});

  /// What the person is shown; its `values` are the values that are not secret, as the JSON object text that was sent.
  final AcpSecretContext context;

  Map<String, Object?> toJson() => _contextJson(context);

  factory AcpSecretShowOpen.fromJson(Object? json) {
    final context = _contextOf(_mapOf(json, 'the show is not an object'));
    if (context.values == null) {
      throw _fail(AcpSecretFailure.malformed, 'the show has no values');
    }
    return AcpSecretShowOpen(context: context);
  }
}

/// The answer to an open step: [recipient] when ready, else null with [action] `decline` or `cancel`.
final class AcpSecretShowOpened {
  const AcpSecretShowOpened.ready(AcpSecretRecipient this.recipient)
    : action = 'ready';
  const AcpSecretShowOpened.declined() : action = 'decline', recipient = null;
  const AcpSecretShowOpened.cancelled() : action = 'cancel', recipient = null;

  final String action;
  final AcpSecretRecipient? recipient;

  Map<String, Object?> toJson() => {
    'action': action,
    if (recipient != null) 'recipient': recipient!.toJson(),
  };

  factory AcpSecretShowOpened.fromJson(Object? json) {
    final map = _mapOf(json, 'the answer is not an object');
    return switch (map['action']) {
      'ready' => AcpSecretShowOpened.ready(
        AcpSecretRecipient.fromJson(map['recipient']),
      ),
      'decline' => const AcpSecretShowOpened.declined(),
      'cancel' => const AcpSecretShowOpened.cancelled(),
      _ => throw _fail(
        AcpSecretFailure.malformed,
        "the answer's action is not ready, decline or cancel",
      ),
    };
  }
}

/// [acpSecretShowDeliverMethod]'s parameters.
final class AcpSecretShowDeliver {
  const AcpSecretShowDeliver({
    required this.sessionId,
    required this.requestId,
    required this.sealed,
    required this.auth,
  });

  final String sessionId, requestId;
  final Map<String, AcpSealed> sealed;

  /// The host's binding of [sealed], from [AcpSecretCollector.authenticate].
  final AcpSecretBinding auth;

  Map<String, Object?> toJson() => {
    'sessionId': sessionId,
    'requestId': requestId,
    'values': {
      for (final MapEntry(:key, :value) in sealed.entries) key: value.toJson(),
    },
    'auth': auth.toJson(),
  };

  factory AcpSecretShowDeliver.fromJson(Object? json) {
    final map = _mapOf(json, 'the show is not an object');
    final values = _mapOf(map['values'], 'the show has no values');
    return AcpSecretShowDeliver(
      sessionId: _textOf(map, 'sessionId', 'the show has no session'),
      requestId: _textOf(map, 'requestId', 'the show has no id'),
      sealed: {
        for (final MapEntry(:key, :value) in values.entries)
          key: AcpSealed.fromJson(value),
      },
      auth: AcpSecretBinding.fromJson(map['auth']),
    );
  }
}

/// The answer to a deliver step: `shown` when the person saw the values, else `cancel`.
Map<String, Object?> acpSecretShowDone({required bool shown}) => {
  'action': shown ? 'shown' : 'cancel',
};

/// Reads the answer to a deliver step: true for `shown`.
bool acpSecretShowDoneFromJson(Object? json) =>
    switch (_mapOf(json, 'the answer is not an object')['action']) {
      'shown' => true,
      'cancel' => false,
      _ => throw _fail(
        AcpSecretFailure.malformed,
        "the answer's action is not shown or cancel",
      ),
    };

// ---- Hooks for the vector tests. They are not exported by `package:coqui_acp/secret.dart`.

/// A receiver whose key pair is derived from [ikm] (RFC 9180's DeriveKeyPair).
Future<AcpSecretReceiver> secretReceiverFromIkm(
  AcpSecretMethod method,
  AcpSecretContext context,
  AcpSecretProver prover,
  AcpIdentityId audience,
  int now,
  Duration lifetime,
  List<int> ikm, {
  AcpSecretLimits limits = const AcpSecretLimits(),
}) => AcpSecretReceiver._fromIkm(
  method,
  context,
  prover,
  audience,
  now,
  lifetime,
  limits,
  ikm,
);

/// Seals with the ephemeral key derived from [ikm].
Future<AcpSealed> secretSealWithIkm(
  AcpSecretCollector collector,
  String name,
  String value,
  List<int> ikm,
) => collector._seal(name, value, ikm);

/// The statement of a key binding.
Uint8List secretKeyStatement(
  AcpSecretMethod method,
  String requestId,
  List<int> digest,
  List<int> key,
  int expires,
  AcpIdentityId audience,
) => _statement(
  _Kind.key,
  method,
  requestId,
  digest,
  key,
  const [],
  expires,
  audience,
);

/// The statement of a values binding.
Uint8List secretValuesStatement(
  AcpSecretMethod method,
  String requestId,
  List<int> digest,
  List<int> key,
  String plain,
  Map<String, AcpSealed> sealed,
  int expires,
  AcpIdentityId audience,
) => _statement(
  _Kind.answer,
  method,
  requestId,
  digest,
  key,
  _valuesDigest(plain, sealed),
  expires,
  audience,
);

/// The digest of a whole answer.
Uint8List secretValuesDigest(String plain, Map<String, AcpSealed> sealed) =>
    _valuesDigest(plain, sealed);

/// The grammar of a whole number, a number and the two base64 forms, as text.
bool secretGrammar(String kind, String text) => switch (kind) {
  'integer' => _jsonInteger(text) != null,
  'number' => _jsonNumber(text) != null,
  'base64url' => _base64Canonical(text, url: true),
  _ => _base64Canonical(text, url: false),
};

/// Replaces a binding's proof; for tests that change one thing about a binding.
AcpSecretBinding secretWithProof(AcpSecretBinding binding, Uint8List proof) =>
    AcpSecretBinding(
      method: binding.method,
      key: binding.key,
      expires: binding.expires,
      audience: binding.audience,
      proof: proof,
    );

/// The same signature written with the other s (n - s).
Uint8List secretHighS(Uint8List proof) => _concat([
  proof.sublist(0, 32),
  _bigBytes(_p256.n - _bigOf(proof.sublist(32))),
]);

/// The HPKE info of a field.
Uint8List secretFieldInfo(
  AcpSecretMethod method,
  String requestId,
  List<int> digest,
  String field,
) => _fieldInfo(method, requestId, digest, field);

/// A recipient for any public [key], bound as a receiver would: for a test of how a collector treats a key of low order.
Future<AcpSecretRecipient> secretBindKey(
  AcpSecretProver prover,
  AcpSecretMethod method,
  AcpSecretContext context,
  AcpIdentityId audience,
  List<int> key,
  int now,
) async => AcpSecretRecipient(
  Uint8List.fromList(key),
  await prover._bind(
    _Kind.key,
    method,
    context.requestId,
    context.digest(method),
    key,
    const [],
    now + 240,
    audience,
  ),
);

/// Binds an answer as an authenticated but hostile sender could: without the checks [AcpSecretCollector.authenticate] makes first.
Future<AcpSecretBinding> secretBindAnswer(
  AcpSecretProver prover,
  AcpSecretMethod method,
  AcpSecretContext context,
  List<int> key,
  String plain,
  Map<String, AcpSealed> sealed,
  int expires,
  AcpIdentityId audience,
) => prover._bind(
  _Kind.answer,
  method,
  context.requestId,
  context.digest(method),
  key,
  _valuesDigest(plain, sealed),
  expires,
  audience,
);
