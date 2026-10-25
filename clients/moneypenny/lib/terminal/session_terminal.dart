import 'dart:async';
import 'dart:convert';

import 'package:material_ui/material_ui.dart';
import 'package:xterm2/xterm.dart';

/// Decodes a terminal's UTF-8 byte stream without losing split characters.
Stream<String> decodeTerminalOutput(Stream<List<int>> bytes) =>
    const Utf8Decoder(allowMalformed: true).bind(bytes);

/// Renders one existing provider Session; it never starts or resumes a Session.
class SessionTerminal extends StatefulWidget {
  const SessionTerminal({
    required this.output,
    this.onInput,
    this.onResize,
    this.writeLeaseHeld = false,
    this.maxLines = 10000,
    super.key,
  });

  final Stream<List<int>> output;
  final ValueChanged<List<int>>? onInput;
  final void Function(int columns, int rows)? onResize;
  final bool writeLeaseHeld;
  final int maxLines;

  @override
  State<SessionTerminal> createState() => _SessionTerminalState();
}

class _SessionTerminalState extends State<SessionTerminal> {
  late final Terminal _terminal;
  StreamSubscription<String>? _output;
  String? _streamNotice;

  bool get _canWrite => widget.writeLeaseHeld && widget.onInput != null;

  @override
  void initState() {
    super.initState();
    _terminal = Terminal(
      maxLines: widget.maxLines,
      onOutput: (text) {
        if (_canWrite) widget.onInput!(utf8.encode(text));
      },
      onResize: (columns, rows, _, _) {
        if (_canWrite) widget.onResize?.call(columns, rows);
      },
    );
    _listen();
  }

  @override
  void didUpdateWidget(SessionTerminal oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.output != widget.output) {
      unawaited(_output?.cancel());
      _streamNotice = null;
      _listen();
    }
  }

  void _listen() {
    _output = decodeTerminalOutput(widget.output).listen(
      _terminal.write,
      onError: (Object _, StackTrace _) =>
          _showStreamNotice('Terminal stream disconnected.'),
      onDone: () {
        if (_streamNotice == null) _showStreamNotice('Terminal stream ended.');
      },
    );
  }

  void _showStreamNotice(String notice) {
    if (mounted && _streamNotice != notice) {
      setState(() => _streamNotice = notice);
    }
  }

  @override
  void dispose() {
    unawaited(_output?.cancel());
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Stack(
    fit: StackFit.expand,
    children: [
      TerminalView(
        _terminal,
        readOnly: !_canWrite,
        autofocus: false,
        textStyle: const TerminalStyle(fontFamily: 'monospace', fontSize: 14),
      ),
      if (_streamNotice case final notice?)
        Align(
          alignment: Alignment.topCenter,
          child: ColoredBox(
            color: Color(0xff48242b),
            child: Padding(
              padding: EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              child: Text(notice),
            ),
          ),
        ),
    ],
  );
}
