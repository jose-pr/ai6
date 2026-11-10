/// The settings of a provider a record chose, drawn from the provider's own schema and form.
///
/// A record names a provider in one field (`format: provider:<kind>`, a choice of the providers
/// the hosts report) and holds that provider's settings in another (`settings-of:<field>`, or
/// `flat-settings-of:<field>` when they are written beside the provider's name). The record's
/// own form leaves the settings field out; this file finds those fields, takes them out of the
/// schema, and says what the settings form for a chosen provider is.
library;

import '../forms/form_host.dart';
import 'reported_providers.dart';

const providerFormat = 'provider:';
const _settingsFormat = 'settings-of:';
const _flatSettingsFormat = 'flat-settings-of:';

/// One field that holds the settings of a provider another field chose.
class SettingsSlot {
  const SettingsSlot({
    required this.parent,
    required this.field,
    required this.sibling,
    required this.kind,
    required this.title,
    required this.flat,
    this.section,
  });

  /// The path of the object that holds both fields; empty for the record itself.
  final List<String> parent;

  /// The settings field.
  final String field;

  /// The field that names the provider.
  final String sibling;

  /// The Guatiao kind the provider is chosen from.
  final String kind;
  final String title;

  /// Whether the settings are written beside the provider's name rather than under [field].
  final bool flat;

  /// Only the provider's settings marked with this `x-section` are the record's to set.
  final String? section;

  /// This slot at the real path `path` of an item or entry.
  SettingsSlot at(List<String> path) => SettingsSlot(
    parent: path,
    field: field,
    sibling: sibling,
    kind: kind,
    title: title,
    flat: flat,
    section: section,
  );

  String get id => [...parent, field].join('.');
  List<String> get siblingPath => [...parent, sibling];
}

/// The settings fields of `schema`, at any depth of nested groups.
List<SettingsSlot> settingsSlots(Map<String, Object?> schema) {
  final slots = <SettingsSlot>[];
  void walk(Map<Object?, Object?> node, List<String> parent) {
    // Every item of a list, and every value of a map, has the same shape: a slot below it
    // stands for one settings form per item or entry (`*`).
    for (final below in ['items', 'additionalProperties']) {
      final inner = node[below];
      if (inner is Map && inner.isNotEmpty) walk(inner, [...parent, '*']);
    }
    final properties = _properties(node);
    if (properties == null) return;
    for (final MapEntry(:key, :value) in properties.entries) {
      if (value is! Map) continue;
      final format = value['format'];
      if (format is String &&
          (format.startsWith(_settingsFormat) ||
              format.startsWith(_flatSettingsFormat))) {
        final flat = format.startsWith(_flatSettingsFormat);
        final rest = format.substring(
          flat ? _flatSettingsFormat.length : _settingsFormat.length,
        );
        final named = rest.split('#');
        final sibling = properties[named.first];
        final siblingFormat = sibling is Map ? sibling['format'] : null;
        if (siblingFormat is String &&
            siblingFormat.startsWith(providerFormat)) {
          slots.add(
            SettingsSlot(
              parent: parent,
              field: key as String,
              sibling: named.first,
              kind: siblingFormat.substring(providerFormat.length),
              title: value['title'] as String? ?? key,
              flat: flat,
              section: named.length > 1 ? named[1] : null,
            ),
          );
        }
        continue;
      }
      walk(value, [...parent, key as String]);
    }
  }

  walk(schema, const []);
  return slots;
}

Map<Object?, Object?>? _properties(Map<Object?, Object?> node) {
  final direct = node['properties'];
  if (direct is Map) return direct;
  final any = node['anyOf'];
  if (any is List) {
    for (final arm in any) {
      if (arm is Map && arm['properties'] is Map) {
        return arm['properties'] as Map;
      }
    }
  }
  return null;
}

/// A copy of `schema` without the settings fields of `slots`.
Map<String, Object?> withoutSettings(
  Map<String, Object?> schema,
  List<SettingsSlot> slots,
) {
  Object? copy(Object? node, List<String> path) {
    if (node is Map) {
      final out = <String, Object?>{};
      for (final MapEntry(:key, :value) in node.entries) {
        if (key == 'properties' && value is Map) {
          out[key as String] = {
            for (final MapEntry(key: name, value: member) in value.entries)
              if (!slots.any(
                (slot) => slot.field == name && _same(slot.parent, path),
              ))
                name: copy(member, [...path, name as String]),
          };
        } else if (key == 'required' && value is List) {
          out[key as String] = [
            for (final name in value)
              if (!slots.any(
                (slot) => slot.field == name && _same(slot.parent, path),
              ))
                name,
          ];
        } else if ((key == 'items' || key == 'additionalProperties') &&
            value is Map) {
          out[key as String] = copy(value, [...path, '*']);
        } else {
          out[key as String] = copy(value, path);
        }
      }
      return out;
    }
    if (node is List) return [for (final item in node) copy(item, path)];
    return node;
  }

  return copy(schema, const []) as Map<String, Object?>;
}

bool _same(List<String> a, List<String> b) =>
    a.length == b.length &&
    [for (var i = 0; i < a.length; i++) a[i] == b[i]].every((same) => same);

Object? _step(Object? at, String step) {
  if (at is Map) return at[step];
  if (at is List) {
    final index = int.tryParse(step);
    return index == null || index < 0 || index >= at.length ? null : at[index];
  }
  return null;
}

/// The object of `value` at `path` (a number steps into a list), or null when the path is not
/// there.
Map<String, Object?>? objectAt(Object? value, List<String> path) {
  Object? at = value;
  for (final step in path) {
    at = _step(at, step);
  }
  return at is Map ? at.cast<String, Object?>() : null;
}

/// The slots `template` stands for in `value`: one for each item of a list or entry of a map its
/// `*` steps pass through, with the real path in place of the `*`.
List<SettingsSlot> expandSlot(SettingsSlot template, Object? value) {
  var found = <List<String>>[[]];
  for (final step in template.parent) {
    final next = <List<String>>[];
    for (final path in found) {
      if (step != '*') {
        next.add([...path, step]);
        continue;
      }
      var at = value;
      for (final walked in path) {
        at = _step(at, walked);
      }
      if (at is List) {
        for (var i = 0; i < at.length; i++) {
          next.add([...path, '$i']);
        }
      } else if (at is Map) {
        for (final key in at.keys) {
          next.add([...path, key as String]);
        }
      }
    }
    found = next;
  }
  return [for (final path in found) template.at(path)];
}

/// The member keys `schema` declares at `path`.
Set<String> declaredAt(Map<String, Object?> schema, List<String> path) {
  Map<Object?, Object?>? node = schema;
  for (final step in path) {
    if (node == null) break;
    // A member by name, else the shape every item of a list or entry of a map shares.
    Object? member = _properties(node)?[step];
    member ??= node['items'] ?? node['additionalProperties'];
    node = member is Map ? member : null;
  }
  return {
    for (final key in (node == null ? null : _properties(node))?.keys ?? [])
      key as String,
  };
}

/// `schema` limited to the properties marked with `x-section == section`; whole when there is no
/// section.
Map<String, Object?> forSection(Map<String, Object?> schema, String? section) {
  if (section == null) return schema;
  final properties = schema['properties'];
  if (properties is! Map) return schema;
  final kept = {
    for (final MapEntry(:key, :value) in properties.entries)
      if (value is Map && value['x-section'] == section) key: value,
  };
  return {
    ...schema,
    'properties': kept,
    if (schema['required'] is List)
      'required': [
        for (final name in schema['required'] as List)
          if (kept.containsKey(name)) name,
      ],
  };
}

/// `form` without hints for fields `schema` no longer has.
Map<String, Object?>? formFor(
  Map<String, Object?> schema,
  Map<String, Object?>? form,
) {
  if (form == null) return null;
  final fields = form['fields'];
  final properties = schema['properties'];
  if (fields is! Map || properties is! Map) return form;
  return {
    ...form,
    'fields': {
      for (final MapEntry(:key, :value) in fields.entries)
        if (properties.containsKey(key)) key: value,
    },
  };
}

/// What `choicesFor` offers for a `provider:<kind>` format: the providers the hosts report that
/// serve the kind, by their names. The list is open when no host reports any, so a deployment
/// whose Stations have not published a catalog can still be configured by typing the ID.
FieldChoices? providerChoices(ProvidersReport providers, String format) {
  if (!format.startsWith(providerFormat)) return null;
  final found = providers.serving(format.substring(providerFormat.length));
  return FieldChoices(
    [for (final provider in found) provider.id],
    open: found.isEmpty,
    labels: {
      for (final provider in found)
        if (provider.name.isNotEmpty && provider.name != provider.id)
          provider.id: '${provider.name} (${provider.id})',
    },
  );
}

/// A copy of `value` shaped as `schema` says: a group that holds a bare word (a unit variant
/// such as `"local"`) starts empty in the form.
Object? shaped(Map<Object?, Object?> schema, Object? value) {
  final properties = _properties(schema);
  if (properties == null) return value;
  if (value is! Map) {
    if (value == null) return null;
    // A bare word where a group is drawn names a provider (`access = "local"`): it is the
    // group that takes a provider, chosen.
    if (value is String) {
      for (final MapEntry(:key, value: member) in properties.entries) {
        final inner = member is Map ? _properties(member) : null;
        if (inner != null && inner.containsKey('provider')) {
          return <String, Object?>{
            key as String: <String, Object?>{'provider': value},
          };
        }
      }
    }
    return <String, Object?>{};
  }
  return <String, Object?>{
    for (final MapEntry(:key, value: held) in value.entries)
      key as String: properties[key] is Map
          ? shaped(properties[key] as Map, held)
          : held,
  };
}
