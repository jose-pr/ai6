/// The data model every format reads into.
///
/// A [Value] is a `bool`, a `BigInt` (an integer in the signed 64-bit range), a `double` (a
/// finite float), a `String`, a `List<Object>` or a [Document]. Integers are `BigInt`s because
/// on the web a Dart `int` and a `double` are the same JavaScript number, so `30` and `30.0`
/// could not be told apart. There is no null.
library;

/// One value of the data model: `bool`, `BigInt`, `double`, `String`, `List<Object>` or [Document].
typedef Value = Object;

/// A mapping: keys are text, iteration order is the document's.
typedef Document = Map<String, Object>;

/// Whether [value] is a mapping.
bool isDocument(Object? value) => value is Map<String, Object>;

/// Whether [value] is a list.
bool isList(Object? value) => value is List<Object>;

/// Whether [value] is an integer.
bool isInteger(Object? value) => value is BigInt;

/// The value as plain Dart: integers that fit an `int` become one, the rest stay `BigInt`.
Object toPlain(Object value) {
  if (value is Map<String, Object>) {
    return {for (final entry in value.entries) entry.key: toPlain(entry.value)};
  }
  if (value is List<Object>) return [for (final item in value) toPlain(item)];
  if (value is BigInt && value.isValidInt) return value.toInt();
  return value;
}

/// Structural equality. A mapping's order does not matter; an integer never equals a float.
bool valuesEqual(Object? a, Object? b) {
  if (a is Map<String, Object?>) {
    if (b is! Map<String, Object?> || a.length != b.length) return false;
    for (final entry in a.entries) {
      if (!b.containsKey(entry.key) ||
          !valuesEqual(entry.value, b[entry.key])) {
        return false;
      }
    }
    return true;
  }
  if (a is List<Object?>) {
    if (b is! List<Object?> || a.length != b.length) return false;
    for (var at = 0; at < a.length; at += 1) {
      if (!valuesEqual(a[at], b[at])) return false;
    }
    return true;
  }
  if (a is BigInt) return b is BigInt && a == b;
  if (a is double) return b is double && a == b;
  if (a is String) return b is String && a == b;
  if (a is bool) return b is bool && a == b;
  return a == null && b == null;
}

/// A value from `jsonDecode`'s result, for callers that hold plain JSON: an `int` becomes an integer.
///
/// On the web `jsonDecode` cannot tell `30` from `30.0`, so a whole number there is an integer.
Object fromPlain(Object? plain) {
  if (plain is Map) {
    final out = <String, Object>{};
    for (final entry in plain.entries) {
      out[entry.key as String] = fromPlain(entry.value);
    }
    return out;
  }
  if (plain is List) return [for (final item in plain) fromPlain(item)];
  if (plain is int) return BigInt.from(plain);
  if (plain == null) throw ArgumentError('a null is not a value');
  return plain;
}
