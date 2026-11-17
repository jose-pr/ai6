import 'dart:async';

import 'package:material_ui/material_ui.dart';

import 'access_api.dart';

/// An account's certificate pins and the certificate fields that name it. Both live in the
/// gateway's database, so every gateway sharing it reads what is changed here.
class AccountPinsDialog extends StatefulWidget {
  const AccountPinsDialog({required this.api, required this.name, super.key});

  final AccessApi api;
  final String name;

  @override
  State<AccountPinsDialog> createState() => _AccountPinsDialogState();
}

class _AccountPinsDialogState extends State<AccountPinsDialog> {
  final _pin = TextEditingController();
  final _field = TextEditingController();
  final _value = TextEditingController();
  AccountPins? _held;
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_change(() => widget.api.pins(widget.name)));
  }

  @override
  void dispose() {
    _pin.dispose();
    _field.dispose();
    _value.dispose();
    super.dispose();
  }

  Future<void> _change(Future<AccountPins> Function() call) async {
    try {
      final held = await call();
      if (mounted) {
        setState(() {
          _held = held;
          _error = null;
        });
      }
    } on AccessRefused catch (error) {
      if (mounted) {
        setState(() => _error = error.reason);
      }
    }
  }

  Future<void> _addPin() async {
    final pin = _pin.text.trim();
    if (pin.isEmpty) {
      return;
    }
    await _change(() => widget.api.changePins(widget.name, add: [pin]));
    if (_error == null) {
      _pin.clear();
    }
  }

  Future<void> _addMatch() async {
    final field = _field.text.trim();
    final value = _value.text.trim();
    if (field.isEmpty || value.isEmpty) {
      return;
    }
    await _change(
      () => widget.api.changePins(
        widget.name,
        matchAdd: [PinMatch(field, value)],
      ),
    );
    if (_error == null) {
      _field.clear();
      _value.clear();
    }
  }

  @override
  Widget build(BuildContext context) {
    final held = _held;
    return AlertDialog(
      title: Text('Certificate pins of ${widget.name}'),
      content: SizedBox(
        width: 560,
        child: held == null && _error == null
            ? const Center(child: CircularProgressIndicator())
            : SingleChildScrollView(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    if (_error != null)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 8),
                        child: Text(_error!),
                      ),
                    if (held != null) ...[
                      Text(
                        'Pins (${held.pins.length} of ${held.maxPins}): the '
                        'SHA-256 of a certificate (sha256:) or of its public '
                        'key (spki-sha256:).',
                      ),
                      for (final pin in held.pins)
                        ListTile(
                          dense: true,
                          title: SelectableText(pin),
                          trailing: IconButton(
                            tooltip: 'Remove $pin',
                            icon: const Icon(Icons.delete_outline),
                            onPressed: () => _change(
                              () => widget.api.changePins(
                                widget.name,
                                remove: [pin],
                              ),
                            ),
                          ),
                        ),
                      TextField(
                        controller: _pin,
                        decoration: const InputDecoration(labelText: 'New pin'),
                        onSubmitted: (_) => _addPin(),
                      ),
                      Align(
                        alignment: Alignment.centerLeft,
                        child: TextButton(
                          onPressed: _addPin,
                          child: const Text('Add pin'),
                        ),
                      ),
                      const SizedBox(height: 12),
                      Text(
                        'Certificate fields that name this account '
                        '(${held.matches.length} of ${held.maxPins}).',
                      ),
                      for (final entry in held.matches)
                        ListTile(
                          dense: true,
                          title: Text('${entry.field} = ${entry.value}'),
                          trailing: IconButton(
                            tooltip: 'Remove ${entry.field} ${entry.value}',
                            icon: const Icon(Icons.delete_outline),
                            onPressed: () => _change(
                              () => widget.api.changePins(
                                widget.name,
                                matchRemove: [entry],
                              ),
                            ),
                          ),
                        ),
                      Row(
                        children: [
                          Expanded(
                            child: TextField(
                              controller: _field,
                              decoration: const InputDecoration(
                                labelText: 'Field, such as san.email',
                              ),
                            ),
                          ),
                          const SizedBox(width: 8),
                          Expanded(
                            child: TextField(
                              controller: _value,
                              decoration: const InputDecoration(
                                labelText: 'Value',
                              ),
                              onSubmitted: (_) => _addMatch(),
                            ),
                          ),
                        ],
                      ),
                      Align(
                        alignment: Alignment.centerLeft,
                        child: TextButton(
                          onPressed: _addMatch,
                          child: const Text('Add field'),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
      ),
      actions: [
        FilledButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Done'),
        ),
      ],
    );
  }
}
