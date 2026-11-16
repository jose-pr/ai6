import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';
import 'package:qr/qr.dart';

import '../main.dart';
import '../theme/oo_theme.dart';

/// What the gateway says of the signed-in person's second factor.
class FactorState {
  const FactorState({
    required this.configured,
    required this.enrolled,
    required this.enrolling,
    required this.mayEnrol,
  });

  final bool configured;
  final bool enrolled;
  final bool enrolling;
  final bool mayEnrol;

  factory FactorState.fromJson(Map<String, dynamic> json) => FactorState(
    configured: json['configured'] == true,
    enrolled: json['enrolled'] == true,
    enrolling: json['enrolling'] == true,
    mayEnrol: json['may_enrol'] == true,
  );
}

/// A change the gateway refused, in its words.
class FactorRefused implements Exception {
  const FactorRefused(this.reason);

  final String reason;

  @override
  String toString() => reason;
}

/// The signed-in person's second factor, through `/v1/user/factors`.
class FactorsApi {
  const FactorsApi(this.board);

  final MissionBoard board;

  FactorRefused _refused(http.Response response) {
    var reason = 'The gateway refused it (HTTP ${response.statusCode}).';
    try {
      final body = jsonDecode(response.body);
      if (body is Map && body['error'] is String) {
        reason = body['error'] as String;
      }
    } on FormatException {
      // The gateway's plain refusals have no body.
    }
    return FactorRefused(reason);
  }

  Future<FactorState> state() async {
    final response = await board.accessRequest('GET', '/v1/user/factors');
    if (response.statusCode != 200) throw _refused(response);
    return FactorState.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  /// Begins an enrolment: the provisioning link and the recovery codes, for the caller to show
  /// once and keep nowhere.
  Future<Begun> begin() async {
    final response = await board.accessRequest('POST', '/v1/user/factors/totp');
    if (response.statusCode != 200) throw _refused(response);
    final body = jsonDecode(response.body) as Map<String, dynamic>;
    return Begun(
      uri: body['uri'] as String,
      recoveryCodes: (body['recovery_codes'] as List<dynamic>).cast<String>(),
    );
  }

  Future<void> confirm(String code) async {
    final response = await board.accessRequest(
      'POST',
      '/v1/user/factors/totp/confirm',
      body: {'code': code},
    );
    if (response.statusCode != 200) throw _refused(response);
  }
}

/// What beginning an enrolment answers.
class Begun {
  const Begun({required this.uri, required this.recoveryCodes});

  final String uri;
  final List<String> recoveryCodes;

  /// The secret to type into an app that cannot read the QR code.
  String get secret => Uri.tryParse(uri)?.queryParameters['secret'] ?? '';
}

/// The Second factor section of the person's own profile: whether a one-time code app is set up,
/// and a dialog that sets one up. The link and the recovery codes are held only by that dialog.
class SecondFactorSection extends StatefulWidget {
  const SecondFactorSection({required this.board, super.key});

  final MissionBoard board;

  @override
  State<SecondFactorSection> createState() => _SecondFactorSectionState();
}

class _SecondFactorSectionState extends State<SecondFactorSection> {
  FactorState? _state;
  String? _error;

  FactorsApi get _api => FactorsApi(widget.board);

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final state = await _api.state();
      if (mounted) {
        setState(() {
          _state = state;
          _error = null;
        });
      }
    } on FactorRefused catch (error) {
      if (mounted) setState(() => _error = error.reason);
    } on Object {
      if (mounted) setState(() => _error = 'The second factor is unavailable.');
    }
  }

  Future<void> _setUp() async {
    final Begun begun;
    try {
      begun = await _api.begin();
    } on FactorRefused catch (error) {
      if (mounted) setState(() => _error = error.reason);
      return;
    }
    if (!mounted) return;
    await showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (context) => _EnrolDialog(api: _api, begun: begun),
    );
    await _load();
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final muted = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: tokens.muted);
    final state = _state;
    if (state != null && !state.configured) return const SizedBox.shrink();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Semantics(
          header: true,
          child: Text(
            'Second factor',
            style: Theme.of(context).textTheme.titleSmall,
          ),
        ),
        const SizedBox(height: 4),
        if (_error != null)
          Semantics(
            liveRegion: true,
            child: Text(_error!, style: TextStyle(color: tokens.danger)),
          )
        else if (state == null)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 8),
            child: LinearProgressIndicator(),
          )
        else ...[
          Text(
            state.enrolled
                ? 'A code from an authenticator app is set up.'
                : state.enrolling
                ? 'Waiting for the first code from your authenticator app.'
                : 'No authenticator app is set up.',
            style: muted,
          ),
          const SizedBox(height: 8),
          if (state.mayEnrol)
            Align(
              alignment: Alignment.centerLeft,
              child: OutlinedButton(
                onPressed: _setUp,
                child: Text(state.enrolled ? 'Replace' : 'Set up'),
              ),
            )
          else
            Text(
              'Your sign-in policy does not let this sign-in set up a second '
              'factor.',
              style: muted,
            ),
        ],
      ],
    );
  }
}

/// Shows the QR code, the secret and the recovery codes, and asks for the first code. Closing
/// it drops them.
class _EnrolDialog extends StatefulWidget {
  const _EnrolDialog({required this.api, required this.begun});

  final FactorsApi api;
  final Begun begun;

  @override
  State<_EnrolDialog> createState() => _EnrolDialogState();
}

class _EnrolDialogState extends State<_EnrolDialog> {
  final _code = TextEditingController();
  String? _error;
  bool _busy = false;

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  Future<void> _confirm() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.api.confirm(_code.text.trim());
      if (mounted) Navigator.pop(context);
    } on FactorRefused catch (error) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = error.reason;
        });
      }
    } on Object {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = 'Could not connect to Universal Exports.';
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final begun = widget.begun;
    return AlertDialog(
      title: const Text('Set up an authenticator app'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Scan the code with your app, or type the secret. Shown once, '
              'not stored.',
            ),
            const SizedBox(height: 12),
            Center(
              child: Semantics(
                label: 'QR code for your authenticator app',
                child: SizedBox.square(
                  dimension: 200,
                  child: CustomPaint(
                    key: const ValueKey('second-factor-qr'),
                    painter: QrPainter(begun.uri),
                  ),
                ),
              ),
            ),
            const SizedBox(height: 8),
            SelectableText(begun.secret),
            const SizedBox(height: 12),
            const Text('Recovery codes, each works once:'),
            const SizedBox(height: 4),
            for (final code in begun.recoveryCodes) SelectableText(code),
            const SizedBox(height: 12),
            TextField(
              controller: _code,
              keyboardType: TextInputType.number,
              autocorrect: false,
              enableSuggestions: false,
              decoration: const InputDecoration(labelText: 'Code from the app'),
            ),
            if (_error != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Semantics(
                  liveRegion: true,
                  child: Text(_error!, style: TextStyle(color: tokens.danger)),
                ),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Clipboard.setData(
            ClipboardData(text: begun.recoveryCodes.join('\n')),
          ),
          child: const Text('Copy recovery codes'),
        ),
        TextButton(
          onPressed: _busy ? null : () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: _busy ? null : _confirm,
          child: const Text('Confirm'),
        ),
      ],
    );
  }
}

/// Draws [data] as a QR code on a white square with a quiet zone, dark modules black, so any
/// camera reads it whatever the theme.
class QrPainter extends CustomPainter {
  QrPainter(String data)
    : _image = QrImage(
        QrCode.fromData(data: data, errorCorrectLevel: QrErrorCorrectLevel.M),
      );

  final QrImage _image;

  @override
  void paint(Canvas canvas, Size size) {
    const quiet = 2;
    final count = _image.moduleCount + 2 * quiet;
    final cell = size.shortestSide / count;
    canvas.drawRect(
      Offset.zero & Size.square(size.shortestSide),
      Paint()..color = const Color(0xFFFFFFFF),
    );
    final dark = Paint()..color = const Color(0xFF000000);
    for (var row = 0; row < _image.moduleCount; row++) {
      for (var col = 0; col < _image.moduleCount; col++) {
        if (_image.isDark(row, col)) {
          canvas.drawRect(
            Rect.fromLTWH(
              (col + quiet) * cell,
              (row + quiet) * cell,
              cell,
              cell,
            ),
            dark,
          );
        }
      }
    }
  }

  @override
  bool shouldRepaint(QrPainter old) => old._image != _image;
}
