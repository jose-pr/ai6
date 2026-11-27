import 'dart:async';

import 'package:flutter/services.dart' show TextInput;
import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../theme/oo_backdrop.dart';
import '../theme/oo_decor.dart';
import '../theme/oo_theme.dart';

/// The first page of a board with no sign-in: the wordmark and what the gateway accepts, a name
/// and password, its reverse proxy's word for the person, or both. The password goes only into
/// [MissionBoard.login] and the field is cleared after each try.
class SignInPage extends StatefulWidget {
  const SignInPage({required this.board, super.key});

  final MissionBoard board;

  /// The widest the panel gets.
  static const panelWidth = 420.0;

  @override
  State<SignInPage> createState() => _SignInPageState();
}

class _SignInPageState extends State<SignInPage> {
  final _name = TextEditingController();
  final _secret = TextEditingController();
  bool _submitting = false;
  bool _reveal = false;

  /// What the gateway accepts; the form until it says otherwise.
  SignInMethods _methods = SignInMethods.unknown;

  /// Ticks while the login limit holds the person off, so the wait counts down.
  Timer? _waiting;

  @override
  void initState() {
    super.initState();
    unawaited(_prepare());
    widget.board.addListener(_followWait);
    _followWait();
  }

  /// Runs once before anyone types. A sign-in that needs no typing belongs here, ahead of the
  /// form, which stays for when that does not end in a run.
  Future<void> _prepare() async {
    final methods = await widget.board.loadSignInMethods();
    if (mounted) setState(() => _methods = methods);
  }

  void _followWait() {
    if (widget.board.loginRetryAt == null) {
      _waiting?.cancel();
      _waiting = null;
    } else {
      _waiting ??= Timer.periodic(const Duration(seconds: 1), (_) {
        if (!mounted) return;
        // The wait is over: Continue comes back, and the board forgets the refusal.
        if (widget.board.loginWait == null) widget.board.clearLoginWait();
        setState(() {});
      });
    }
  }

  @override
  void dispose() {
    widget.board.removeListener(_followWait);
    _waiting?.cancel();
    _name.dispose();
    _secret.dispose();
    _code.dispose();
    super.dispose();
  }

  bool get _holding => widget.board.loginWait != null;

  final _code = TextEditingController();
  bool _recovery = false;

  bool get _canSendCode =>
      !_submitting && !_holding && _code.text.trim().isNotEmpty;

  Future<void> _sendCode() async {
    if (!_canSendCode) return;
    setState(() => _submitting = true);
    final code = _code.text.trim();
    _code.clear();
    await widget.board.loginCode(code, recovery: _recovery);
    _finishAutofill();
    if (mounted) setState(() => _submitting = false);
  }

  /// The step after the password: a code from the authenticator app, or a recovery code.
  List<Widget> _codeStep() {
    final tokens = OoTokens.of(context);
    final border = OutlineInputBorder(
      borderRadius: BorderRadius.circular(8),
      borderSide: BorderSide(color: tokens.border),
    );
    final canRecover = widget.board.pendingMethods.contains('recovery_code');
    return [
      Text(
        _recovery
            ? 'Enter one of your recovery codes. Each works once.'
            : 'Enter the code your authenticator app shows.',
        key: const ValueKey('sign-in-help'),
        style: TextStyle(color: tokens.muted),
      ),
      const SizedBox(height: 16),
      TextField(
        controller: _code,
        autocorrect: false,
        enableSuggestions: false,
        keyboardType: _recovery ? TextInputType.text : TextInputType.number,
        autofillHints: const [AutofillHints.oneTimeCode],
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) => _sendCode(),
        decoration: InputDecoration(
          labelText: _recovery ? 'Recovery code' : 'Code',
          filled: true,
          fillColor: tokens.raised,
          border: border,
          enabledBorder: border,
          focusedBorder: border.copyWith(
            borderSide: BorderSide(color: tokens.accent, width: 2),
          ),
        ),
      ),
      if (canRecover)
        Align(
          alignment: Alignment.centerLeft,
          child: TextButton(
            onPressed: () => setState(() {
              _recovery = !_recovery;
              _code.clear();
            }),
            child: Text(
              _recovery ? 'Use a code from the app' : 'Use a recovery code',
            ),
          ),
        ),
    ];
  }

  bool get _canSubmit =>
      !_submitting &&
      !_holding &&
      _name.text.trim().isNotEmpty &&
      _secret.text.isNotEmpty;

  Future<void> _signIn() async {
    if (!_canSubmit) return;
    setState(() => _submitting = true);
    final secret = _secret.text;
    _secret.clear();
    await widget.board.login(_name.text.trim(), secret);
    _finishAutofill();
    if (mounted) setState(() => _submitting = false);
  }

  /// Tells the browser and password managers the sign-in went through, so they offer to keep the
  /// account and password; nothing happens while the sign-in is refused or waits for a code.
  void _finishAutofill() {
    if (widget.board.accessToken != null) TextInput.finishAutofillContext();
  }

  Future<void> _signInViaProxy() async {
    if (_submitting || _holding) return;
    setState(() => _submitting = true);
    await widget.board.loginViaProxy();
    if (mounted) setState(() => _submitting = false);
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final theme = Theme.of(context);
    final error = _problem;
    final waiting = widget.board.pendingSignIn != null;
    return Scaffold(
      body: SizedBox.expand(
        child: OoBackdrop(
          alignment: Alignment.center,
          labelClear: SignInPage.panelWidth,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: SignInPage.panelWidth),
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(28),
              child: AutofillGroup(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const Center(
                      child: FittedBox(
                        fit: BoxFit.scaleDown,
                        child: OoWordmark(size: 34),
                      ),
                    ),
                    const SizedBox(height: 24),
                    Divider(color: tokens.border, height: 1),
                    const SizedBox(height: 24),
                    Semantics(
                      header: true,
                      child: Text(
                        'Sign in',
                        style: theme.textTheme.headlineSmall,
                      ),
                    ),
                    const SizedBox(height: 20),
                    if (waiting) ..._codeStep(),
                    if (!waiting && _methods.password) ..._form(),
                    if (!waiting && !_methods.password && _methods.proxy)
                      const Text(
                        'Continue with the account authenticated by your organization.',
                      ),
                    if (!waiting && !_methods.password && !_methods.proxy)
                      const Text(
                        'This gateway has no sign-in set up. An administrator can configure '
                        'one.',
                      ),
                    if (error != null) ...[
                      const SizedBox(height: 12),
                      Semantics(
                        liveRegion: true,
                        child: Text(
                          error,
                          key: const ValueKey('sign-in-error'),
                          style: TextStyle(color: tokens.danger),
                        ),
                      ),
                    ],
                    if (waiting) ...[
                      const SizedBox(height: 20),
                      FilledButton(
                        onPressed: _canSendCode ? _sendCode : null,
                        child: _submitting
                            ? const SizedBox.square(
                                dimension: 18,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                ),
                              )
                            : const Text('Continue'),
                      ),
                      const SizedBox(height: 8),
                      TextButton(
                        onPressed: _submitting
                            ? null
                            : () {
                                _code.clear();
                                setState(() => _recovery = false);
                                widget.board.cancelPendingSignIn();
                              },
                        child: const Text('Start again'),
                      ),
                    ],
                    if (!waiting && _methods.password) ...[
                      const SizedBox(height: 20),
                      FilledButton(
                        onPressed: _canSubmit ? _signIn : null,
                        child: _submitting
                            ? const SizedBox.square(
                                dimension: 18,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                ),
                              )
                            : const Text('Continue'),
                      ),
                    ],
                    if (!waiting && _methods.proxy) ...[
                      SizedBox(height: _methods.password ? 12 : 20),
                      _proxyButton(),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// No field takes focus by itself: Flutter's web engine leaves a screen reader's click on an
  /// already focused field unconnected, and what is typed there is lost. Tab reaches the first.
  List<Widget> _form() {
    final tokens = OoTokens.of(context);
    final border = OutlineInputBorder(
      borderRadius: BorderRadius.circular(8),
      borderSide: BorderSide(color: tokens.border),
    );
    InputDecoration field(String label, {Widget? suffix}) => InputDecoration(
      labelText: label,
      filled: true,
      fillColor: tokens.raised,
      border: border,
      enabledBorder: border,
      focusedBorder: border.copyWith(
        borderSide: BorderSide(color: tokens.accent, width: 2),
      ),
      suffixIcon: suffix,
    );
    return [
      Text(
        'Use your directory account or an AI6 access key.',
        key: const ValueKey('sign-in-help'),
        style: TextStyle(color: tokens.muted),
      ),
      const SizedBox(height: 16),
      TextField(
        controller: _name,
        keyboardType: TextInputType.emailAddress,
        autofillHints: const [AutofillHints.username, AutofillHints.email],
        textInputAction: TextInputAction.next,
        onChanged: (_) => setState(() {}),
        decoration: field('Account'),
      ),
      const SizedBox(height: 12),
      TextField(
        controller: _secret,
        obscureText: !_reveal,
        autofillHints: const [AutofillHints.password],
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) => _signIn(),
        decoration: field(
          'Password',
          suffix: IconButton(
            tooltip: _reveal ? 'Hide password' : 'Show password',
            icon: Icon(
              _reveal
                  ? Icons.visibility_off_outlined
                  : Icons.visibility_outlined,
            ),
            onPressed: () => setState(() => _reveal = !_reveal),
          ),
        ),
      ),
    ];
  }

  Widget _proxyButton() {
    final enabled = !_submitting && !_holding;
    const label = Text('Continue with organization sign-in');
    return _methods.password
        ? OutlinedButton(
            onPressed: enabled ? _signInViaProxy : null,
            child: label,
          )
        : FilledButton(
            onPressed: enabled ? _signInViaProxy : null,
            child: label,
          );
  }

  /// What to say over the form: how long the login limit holds the person off, while it does,
  /// else why the last try or the last run ended.
  String? get _problem {
    final wait = widget.board.loginWait;
    if (wait != null) {
      final seconds = wait.inSeconds + (wait.inMilliseconds % 1000 > 0 ? 1 : 0);
      return 'Too many sign-in tries. Try again in '
          '$seconds ${seconds == 1 ? 'second' : 'seconds'}.';
    }
    return widget.board.loginError;
  }
}
