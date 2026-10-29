import 'package:material_ui/material_ui.dart';

import 'oo_config.dart';
import 'oo_config_file.dart';

/// The operator menu's connection setting on desktop: Universal Exports' address, written
/// back to `OO.cfg` as `[unex] url` in the format the file declares, keeping its `#!` line.
/// It applies at the next start.
class ConnectionSettings extends StatefulWidget {
  ConnectionSettings({
    super.key,
    String? Function()? read,
    Future<void> Function(String text)? write,
    String? path,
  }) : read = read ?? readDesktopConfig,
       write = write ?? writeDesktopConfig,
       path = path ?? desktopConfigPath();

  /// The current `OO.cfg` text, `null` when there is none.
  final String? Function() read;

  /// Replaces the `OO.cfg` text.
  final Future<void> Function(String text) write;

  /// Where `OO.cfg` is, shown to the person.
  final String? path;

  @override
  State<ConnectionSettings> createState() => _ConnectionSettingsState();
}

class _ConnectionSettingsState extends State<ConnectionSettings> {
  final _url = TextEditingController();
  String? _message;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    try {
      final text = widget.read();
      final url = text == null ? null : OoConfig.parse(text).unexUrl;
      _url.text = url?.toString() ?? '';
    } on Object catch (error) {
      _message = '$error';
    }
  }

  @override
  void dispose() {
    _url.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final url = Uri.tryParse(_url.text.trim());
    if (url == null || !url.hasScheme || url.host.isEmpty) {
      setState(
        () => _message = 'Enter an origin such as https://unex.example.',
      );
      return;
    }
    setState(() => _saving = true);
    String message;
    try {
      await widget.write(withUnexUrl(widget.read() ?? '', url));
      message = 'Saved. Outer Office uses it from its next start.';
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
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const Text('Universal Exports'),
      TextField(
        controller: _url,
        decoration: InputDecoration(
          labelText: 'Address',
          helperText: widget.path == null
              ? null
              : 'Kept in ${widget.path}; comments in a TOML or JSON file are not kept',
        ),
      ),
      Align(
        alignment: Alignment.centerRight,
        child: TextButton(
          onPressed: _saving ? null : _save,
          child: const Text('Save address'),
        ),
      ),
      if (_message case final message?) Text(message),
    ],
  );
}
