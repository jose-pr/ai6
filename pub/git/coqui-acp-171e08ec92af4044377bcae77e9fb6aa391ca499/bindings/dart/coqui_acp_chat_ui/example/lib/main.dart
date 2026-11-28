import 'dart:async';

import 'package:coqui_acp/coqui_acp.dart';
import 'package:coqui_acp_chat/coqui_acp_chat.dart';
import 'package:coqui_acp_chat_ui/coqui_acp_chat_ui.dart';
import 'package:material_ui/material_ui.dart';

void main() => runApp(const ExampleApp());

/// A conversation with a scripted agent: updates go into a `ChatSession` as an agent would send them, the transcript draws it, and
/// the density and the colours change the look. It is not an agent: what is typed starts the script again.
class ExampleApp extends StatefulWidget {
  const ExampleApp({super.key});

  @override
  State<ExampleApp> createState() => _ExampleAppState();
}

class _ExampleAppState extends State<ExampleApp> {
  ChatDensity _density = ChatDensity.normal;
  Brightness _brightness = Brightness.dark;

  @override
  Widget build(BuildContext context) {
    final scheme = ColorScheme.fromSeed(seedColor: const Color(0xFFD9A066), brightness: _brightness);
    return MaterialApp(
      title: 'coqui_acp_chat_ui',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        useMaterial3: true,
        colorScheme: scheme,
        extensions: [ChatLook.fromScheme(scheme, density: _density)],
      ),
      home: ExamplePage(
        density: _density,
        brightness: _brightness,
        onDensity: (density) => setState(() => _density = density),
        onBrightness: (brightness) => setState(() => _brightness = brightness),
      ),
    );
  }
}

class ExamplePage extends StatefulWidget {
  const ExamplePage({super.key, required this.density, required this.brightness, required this.onDensity, required this.onBrightness});

  final ChatDensity density;
  final Brightness brightness;
  final ValueChanged<ChatDensity> onDensity;
  final ValueChanged<Brightness> onBrightness;

  @override
  State<ExamplePage> createState() => _ExamplePageState();
}

class _ExamplePageState extends State<ExamplePage> {
  final _input = TextEditingController();
  late ChatSession _session;
  final List<Timer> _timers = [];
  bool _asking = false;

  @override
  void initState() {
    super.initState();
    _start('Fix the addNumbers bug in src/calc.js');
  }

  void _update(Map<String, dynamic> raw) => _session.apply(AcpSessionUpdate('s1', raw));

  /// What an agent sends over a few seconds: its reading, a plan, an edit that needs a person's word, a command and the answer.
  void _start(String prompt) {
    for (final timer in _timers) {
      timer.cancel();
    }
    _timers.clear();
    _session = ChatSession(id: 'demo', agent: 'Scripted agent')..addListener(() => setState(() {}));
    _asking = false;
    _update({'sessionUpdate': 'user_message_chunk', 'content': {'type': 'text', 'text': prompt}});
    final steps = <int, Map<String, dynamic>>{
      300: {'sessionUpdate': 'agent_thought_chunk', 'content': {'type': 'text', 'text': 'The function subtracts; the name says it adds.'}},
      600: {
        'sessionUpdate': 'plan',
        'entries': [
          {'content': 'Read addNumbers in src/calc.js', 'priority': 'medium', 'status': 'in_progress'},
          {'content': 'Change subtraction to addition', 'priority': 'medium', 'status': 'pending'},
          {'content': 'Check the result', 'priority': 'medium', 'status': 'pending'},
        ],
      },
      900: {
        'sessionUpdate': 'tool_call',
        'toolCallId': 'read',
        'title': 'Read src/calc.js',
        'kind': 'read',
        'status': 'completed',
        'locations': [{'path': 'src/calc.js', 'line': 2}],
      },
      1500: {
        'sessionUpdate': 'plan',
        'entries': [
          {'content': 'Read addNumbers in src/calc.js', 'priority': 'medium', 'status': 'completed'},
          {'content': 'Change subtraction to addition', 'priority': 'medium', 'status': 'in_progress'},
          {'content': 'Check the result', 'priority': 'medium', 'status': 'pending'},
        ],
      },
      1800: {
        'sessionUpdate': 'tool_call',
        'toolCallId': 'edit',
        'title': 'Edit src/calc.js',
        'kind': 'edit',
        'status': 'completed',
        'content': [
          {'type': 'diff', 'path': 'src/calc.js', 'oldText': 'export const addNumbers = (a, b) => a - b;\n', 'newText': 'export const addNumbers = (a, b) => a + b;\n'},
        ],
      },
      2400: {
        'sessionUpdate': 'tool_call',
        'toolCallId': 'run',
        'title': 'node -e "console.log(addNumbers(2, 3))"',
        'kind': 'execute',
        'status': 'failed',
        'content': [
          {'type': 'content', 'content': {'type': 'text', 'text': 'ReferenceError: addNumbers is not defined'}},
        ],
      },
      3000: {
        'sessionUpdate': 'tool_call',
        'toolCallId': 'run2',
        'title': 'node test/calc.test.js',
        'kind': 'execute',
        'status': 'completed',
      },
      3300: {
        'sessionUpdate': 'plan',
        'entries': [
          {'content': 'Read addNumbers in src/calc.js', 'priority': 'medium', 'status': 'completed'},
          {'content': 'Change subtraction to addition', 'priority': 'medium', 'status': 'completed'},
          {'content': 'Check the result', 'priority': 'medium', 'status': 'completed'},
        ],
      },
      3500: {'sessionUpdate': 'agent_message_chunk', 'content': {'type': 'text', 'text': 'I fixed `addNumbers` in `src/calc.js`: it now returns `a + b` instead of `a - b`. The test passes.'}},
    };
    steps.forEach((ms, raw) => _timers.add(Timer(Duration(milliseconds: ms), () => mounted ? _update(raw) : null)));
    _timers.add(Timer(const Duration(milliseconds: 1700), () {
      if (mounted) setState(() => _asking = true);
    }));
    _timers.add(Timer(const Duration(milliseconds: 5000), () {
      if (mounted) setState(() => _asking = false);
    }));
  }

  @override
  void dispose() {
    for (final timer in _timers) {
      timer.cancel();
    }
    _input.dispose();
    super.dispose();
  }

  void _send() {
    final text = _input.text.trim();
    if (text.isEmpty) return;
    _input.clear();
    _start(text);
  }

  @override
  Widget build(BuildContext context) {
    final look = context.chatLook;
    return Scaffold(
      appBar: AppBar(
        title: const Text('coqui_acp_chat_ui'),
        actions: [
          SegmentedButton<ChatDensity>(
            segments: [for (final density in ChatDensity.values) ButtonSegment(value: density, label: Text(density.label))],
            selected: {widget.density},
            onSelectionChanged: (chosen) => widget.onDensity(chosen.first),
          ),
          IconButton(
            tooltip: 'Light or dark',
            icon: Icon(widget.brightness == Brightness.dark ? Icons.light_mode_outlined : Icons.dark_mode_outlined),
            onPressed: () => widget.onBrightness(widget.brightness == Brightness.dark ? Brightness.light : Brightness.dark),
          ),
        ],
      ),
      body: Column(children: [
        Expanded(
          child: ChatTranscript(
            entries: [for (final message in _session.messages) ModelEntry(message)],
            terminals: _session.terminals,
            plan: _session.plan,
            working: _timers.any((timer) => timer.isActive),
            renderers: ChatRenderers(
              // A host's own kind: a notice that starts with "chart:" is drawn as a chart.
              entry: [
                (context, entry) => entry.role == MessageRole.notice && entry.text.startsWith('chart:') ? Text('A chart of ${entry.text.substring(6)}') : null,
              ],
            ),
          ),
        ),
        if (_asking)
          PermissionCard(
            agent: 'Scripted agent',
            title: 'Edit src/calc.js',
            detail: 'export const addNumbers = (a, b) => a + b;',
            options: const [
              AcpPermissionOption(optionId: 'allow', name: 'Yes', kind: 'allow_once'),
              AcpPermissionOption(optionId: 'deny', name: 'No', kind: 'reject_once'),
            ],
            onChoose: (_) => setState(() => _asking = false),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 12, 16),
          child: Center(
            child: ConstrainedBox(
              constraints: BoxConstraints(maxWidth: look.messageWidth + 100),
              child: Composer(controller: _input, onSend: _send, hint: 'Message the scripted agent: it starts the script again'),
            ),
          ),
        ),
      ]),
    );
  }
}
