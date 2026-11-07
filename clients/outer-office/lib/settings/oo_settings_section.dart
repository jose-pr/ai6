import 'package:material_ui/material_ui.dart';

import '../forms/form_host.dart';
import 'oo_config.dart';
import 'oo_schema.dart';

/// `[oo]` as a form, from its declaration. On desktop it writes the file back; on the web the file
/// beside the page is the deployment's, so the form shows it and changes nothing. It applies at the
/// next start, and its form opens with the section, since the form needs `guatiao_intake`.
class OoSettingsSection extends StatefulWidget {
  const OoSettingsSection({
    required this.read,
    this.write,
    this.path,
    super.key,
  });

  /// The current `ai6.cfg` text, `null` when there is none.
  final String? Function() read;

  /// Replaces the `ai6.cfg` text; `null` where it cannot be changed from here.
  final Future<void> Function(String text)? write;

  /// Where `ai6.cfg` is.
  final String? path;

  @override
  State<OoSettingsSection> createState() => _OoSettingsSectionState();
}

class _OoSettingsSectionState extends State<OoSettingsSection> {
  FormModel? _model;
  Map<String, String> _problems = const {};
  String? _message;
  bool _saving = false;

  void _open() {
    if (_model != null) return;
    try {
      final text = widget.read();
      final oo = text == null
          ? const <String, Object?>{}
          : decodeOoConfig(text, path: widget.path)['oo'];
      _model = ooFormModel(
        schema: OoSchema.schema,
        form: OoSchema.form,
        value: oo is Map ? oo.cast<String, Object?>() : const {},
      );
    } on Object catch (error) {
      _message = 'These settings cannot be shown: $error';
    }
  }

  @override
  void dispose() {
    _model?.close();
    super.dispose();
  }

  Future<void> _save(FormModel model) async {
    final write = widget.write;
    if (write == null) return;
    final problems = <String, String>{};
    final value = model.value(issues: problems);
    if (problems.isNotEmpty) {
      setState(() => _problems = problems);
      return;
    }
    setState(() {
      _problems = const {};
      _saving = true;
    });
    String message;
    try {
      final text = withOoSettings(
        widget.read() ?? '',
        OoSchema.withoutDefaults(value),
        path: widget.path,
      );
      OoConfig.parse(text, path: widget.path);
      await write(text);
      message = 'Saved. Outer Office uses it from its next start.';
    } on FormatException catch (error) {
      message = 'Not saved: ${error.message}';
    } on Object catch (error) {
      message = 'Not saved: $error';
    }
    if (!mounted) return;
    setState(() {
      _saving = false;
      _message = message;
    });
  }

  @override
  Widget build(BuildContext context) {
    final model = _model;
    final writable = widget.write != null;
    return ExpansionTile(
      tilePadding: EdgeInsets.zero,
      title: const Text('Outer Office settings'),
      subtitle: Text(
        writable
            ? 'Defaults of this app, kept in ai6.cfg.'
            : "Set by this deployment's ai6.cfg.",
      ),
      onExpansionChanged: (open) {
        if (open) setState(_open);
      },
      children: [
        if (model != null)
          OoFormView(
            key: ObjectKey(model),
            model: model,
            problems: _problems,
            enabled: writable && !_saving,
          ),
        if (_message case final message?)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Text(message),
          ),
        if (model != null && writable)
          Align(
            alignment: Alignment.centerRight,
            child: FilledButton(
              onPressed: _saving ? null : () => _save(model),
              child: const Text('Save settings'),
            ),
          ),
      ],
    );
  }
}
