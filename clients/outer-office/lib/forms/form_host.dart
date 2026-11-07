// Flutter's own Material is what guatiao_flutter draws with; the app's theme is material_ui's.
// ignore: deprecated_member_use
import 'package:flutter/material.dart' as flutter;
import 'package:guatiao_flutter/guatiao_flutter.dart';
import 'package:material_ui/material_ui.dart';

export 'package:guatiao_flutter/guatiao_flutter.dart'
    show ChoicesFor, FieldChoices, FieldSchema, FormModel;

/// Flutter's Material localizations, which `guatiao_flutter`'s menus and fields look up: the
/// app's own are `material_ui`'s, a different type.
const flutterMaterialLocalizations =
    flutter.DefaultMaterialLocalizations.delegate;

/// The app's theme as Flutter's own Material theme, which `guatiao_flutter` draws with.
///
/// The app uses `material_ui`, whose `Theme` is a separate widget from `flutter/material.dart`'s,
/// so the form would otherwise draw in Flutter's default colours. Every colour is the app's
/// (`OoTokens` through its colour scheme).
flutter.ThemeData flutterThemeOf(BuildContext context) {
  final theme = Theme.of(context);
  final c = theme.colorScheme;
  final scheme = flutter.ColorScheme(
    brightness: c.brightness,
    primary: c.primary,
    onPrimary: c.onPrimary,
    primaryContainer: c.primaryContainer,
    onPrimaryContainer: c.onPrimaryContainer,
    secondary: c.secondary,
    onSecondary: c.onSecondary,
    secondaryContainer: c.secondaryContainer,
    onSecondaryContainer: c.onSecondaryContainer,
    tertiary: c.tertiary,
    onTertiary: c.onTertiary,
    tertiaryContainer: c.tertiaryContainer,
    onTertiaryContainer: c.onTertiaryContainer,
    error: c.error,
    onError: c.onError,
    errorContainer: c.errorContainer,
    onErrorContainer: c.onErrorContainer,
    surface: c.surface,
    onSurface: c.onSurface,
    surfaceDim: c.surfaceDim,
    surfaceBright: c.surfaceBright,
    surfaceContainerLowest: c.surfaceContainerLowest,
    surfaceContainerLow: c.surfaceContainerLow,
    surfaceContainer: c.surfaceContainer,
    surfaceContainerHigh: c.surfaceContainerHigh,
    surfaceContainerHighest: c.surfaceContainerHighest,
    onSurfaceVariant: c.onSurfaceVariant,
    outline: c.outline,
    outlineVariant: c.outlineVariant,
    shadow: c.shadow,
    scrim: c.scrim,
    inverseSurface: c.inverseSurface,
    onInverseSurface: c.onInverseSurface,
    inversePrimary: c.inversePrimary,
    surfaceTint: c.surfaceTint,
  );
  return flutter.ThemeData(
    useMaterial3: true,
    colorScheme: scheme,
    dividerColor: theme.dividerColor,
    canvasColor: theme.canvasColor,
    scaffoldBackgroundColor: theme.scaffoldBackgroundColor,
  );
}

/// [IntakeFormView] inside the app's look: Flutter's Material theme from the app's tokens, a
/// transparent Material for the fields to draw on. Menus open above the page, so the app must
/// carry [flutterMaterialLocalizations] ([OoMaterialApp] does).
class OoFormView extends StatelessWidget {
  const OoFormView({
    required this.model,
    this.choicesFor,
    this.problems = const {},
    this.enabled = true,
    this.unavailable = const {},
    this.unavailableChoices = const {},
    super.key,
  });

  final FormModel model;
  final ChoicesFor? choicesFor;
  final Map<String, String> problems;
  final bool enabled;

  /// A field the person's access does not allow, by path, with the reason shown on it.
  final Map<String, String> unavailable;

  /// A choice the person's access does not allow: path, then choice, to the reason.
  final Map<String, Map<String, String>> unavailableChoices;

  @override
  Widget build(BuildContext context) => flutter.Theme(
    data: flutterThemeOf(context),
    child: flutter.Material(
      type: flutter.MaterialType.transparency,
      child: IntakeFormView(
        model: model,
        choicesFor: choicesFor,
        problems: problems,
        enabled: enabled,
        unavailable: unavailable,
        unavailableChoices: unavailableChoices,
      ),
    ),
  );
}

/// A [FormModel] over what the gateway sent.
///
/// A lifted bound ("No limit") may be stored as `null`; the form shows the one word its schema
/// offers, so a save writes that word. A number whose schema `format` is `seconds` or `bytes`
/// shows that unit: the gateway's schemas say it by format, and the shared form reads a unit
/// only from the form.
FormModel ooFormModel({
  required Map<String, Object?> schema,
  Map<String, Object?>? form,
  Map<String, Object?> value = const {},
}) => FormModel(
  schema: schema,
  form: form == null ? null : _withUnits(FieldSchema(schema), form),
  value: _liftedBounds(FieldSchema(schema), value) as Map<String, Object?>,
);

Object? _liftedBounds(FieldSchema at, Object? value) {
  if (value == null) {
    final words = at.wordArm?.choices;
    return words != null && words.length == 1 ? words.single : null;
  }
  if (value is Map) {
    final entries = <String, Object?>{};
    for (final MapEntry(:key, value: held) in value.entries) {
      final field = at.member(key as String) ?? at.values;
      entries[key] = field == null ? held : _liftedBounds(field, held);
    }
    return entries;
  }
  if (value is List) {
    final items = at.items;
    return items == null
        ? value
        : [for (final item in value) _liftedBounds(items, item)];
  }
  return value;
}

/// A copy of `form` with a `unit` hint on each field the schema gives a unit by its format,
/// at any depth of groups, lists and maps of groups.
Map<String, Object?> _withUnits(FieldSchema at, Map<String, Object?> form) {
  final fields = <String, Object?>{
    for (final MapEntry(:key, :value)
        in ((form['fields'] as Map?) ?? const {}).entries)
      key as String: value is Map ? {...value.cast<String, Object?>()} : value,
  };
  for (final key in at.memberKeys) {
    final member = at.member(key);
    if (member == null) continue;
    final hint = (fields[key] ??= <String, Object?>{}) as Map<String, Object?>;
    final unit = switch (member.format) {
      'seconds' => 'seconds',
      'bytes' => 'bytes',
      _ => null,
    };
    if (unit != null && hint['unit'] == null) hint['unit'] = unit;
    final inner = member.hasMembers ? member : (member.items ?? member.values);
    if (inner != null && inner.hasMembers) {
      final nested = hint['form'];
      hint['form'] = _withUnits(
        inner,
        nested is Map ? nested.cast<String, Object?>() : <String, Object?>{},
      );
    }
  }
  return {...form, 'fields': fields};
}
