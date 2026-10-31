import 'dart:async';

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
  /// form, which stays for when that does not end in a session.
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
    super.dispose();
  }

  bool get _holding => widget.board.loginWait != null;

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
    if (mounted) setState(() => _submitting = false);
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
    return Scaffold(
      body: SizedBox.expand(
        child: OoBackdrop(
          alignment: Alignment.center,
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
                    if (_methods.password) ..._form(),
                    if (!_methods.password && _methods.proxy)
                      const Text(
                        'Continue with the account authenticated by your organization.',
                      ),
                    if (!_methods.password && !_methods.proxy)
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
                    if (_methods.password) ...[
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
                    if (_methods.proxy) ...[
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
  List<Widget> _form() => [
    TextField(
      controller: _name,
      autocorrect: false,
      enableSuggestions: false,
      autofillHints: const [AutofillHints.username],
      textInputAction: TextInputAction.next,
      onChanged: (_) => setState(() {}),
      decoration: const InputDecoration(labelText: 'Account'),
    ),
    const SizedBox(height: 12),
    TextField(
      controller: _secret,
      obscureText: true,
      autocorrect: false,
      enableSuggestions: false,
      autofillHints: const [AutofillHints.password],
      onChanged: (_) => setState(() {}),
      onSubmitted: (_) => _signIn(),
      decoration: const InputDecoration(labelText: 'Password'),
    ),
  ];

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
  /// else why the last try or the last session ended.
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
