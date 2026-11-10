import 'dart:async';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../main.dart';
import 'oo_freshness.dart';

/// Tells [onActivity] about every key, pointer and scroll input in the window, and passes
/// every one on untouched.
class OoActivity extends StatefulWidget {
  const OoActivity({required this.onActivity, required this.child, super.key});

  final VoidCallback onActivity;
  final Widget child;

  @override
  State<OoActivity> createState() => _OoActivityState();
}

class _OoActivityState extends State<OoActivity> {
  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_key);
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_key);
    super.dispose();
  }

  bool _key(KeyEvent _) {
    widget.onActivity();
    return false;
  }

  @override
  Widget build(BuildContext context) => Listener(
    behavior: HitTestBehavior.translucent,
    onPointerDown: (_) => widget.onActivity(),
    onPointerMove: (_) => widget.onActivity(),
    onPointerHover: (_) => widget.onActivity(),
    onPointerSignal: (_) => widget.onActivity(),
    child: widget.child,
  );
}

/// Keeps a sign-in alive while the app is used: input marks it active, and when a renewal is
/// due with none, a "Stay signed in?" question with a countdown to the sign-out. It sits above
/// the Navigator, so the route underneath is untouched by it and by the sign-out.
class StaySignedIn extends StatelessWidget {
  const StaySignedIn({required this.board, required this.child, super.key});

  final MissionBoard board;
  final Widget child;

  @override
  Widget build(BuildContext context) => OoActivity(
    onActivity: board.markActive,
    child: AnimatedBuilder(
      animation: board,
      builder: (context, _) {
        final ends = board.idleEnds;
        return Stack(
          fit: StackFit.expand,
          children: [
            child,
            if (ends != null) ...[
              const ModalBarrier(dismissible: false, color: Color(0x99000000)),
              Center(
                child: _StayDialog(ends: ends, onStay: board.staySignedIn),
              ),
            ],
          ],
        );
      },
    ),
  );
}

class _StayDialog extends StatefulWidget {
  const _StayDialog({required this.ends, required this.onStay});

  final DateTime ends;
  final VoidCallback onStay;

  @override
  State<_StayDialog> createState() => _StayDialogState();
}

class _StayDialogState extends State<_StayDialog> {
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final left = widget.ends.difference(ooClock());
    final seconds = left.isNegative ? 0 : (left.inMilliseconds / 1000).ceil();
    final clock =
        '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
    return Semantics(
      scopesRoute: true,
      namesRoute: true,
      explicitChildNodes: true,
      label: 'Stay signed in?',
      child: AlertDialog(
        title: const Text('Stay signed in?'),
        content: Semantics(
          liveRegion: true,
          child: Text('You will be signed out in $clock unless you stay.'),
        ),
        actions: [
          FilledButton(
            autofocus: true,
            onPressed: widget.onStay,
            child: const Text('Stay signed in'),
          ),
        ],
      ),
    );
  }
}
