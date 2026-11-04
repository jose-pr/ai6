import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/transcripts.dart';
import 'package:outer_office/theme/oo_theme.dart';

/// Claude Code's transcript of a short Session: a question, a tool call, its result, an
/// answer.
final _claude = [
  {
    'type': 'user',
    'message': {'role': 'user', 'content': 'Which Stations are busy?'},
  },
  {
    'type': 'assistant',
    'message': {
      'role': 'assistant',
      'content': [
        {
          'type': 'tool_use',
          'name': 'mcp__moneypenny__board',
          'input': {'operation': 'G'},
        },
      ],
    },
  },
  {
    'type': 'user',
    'message': {
      'role': 'user',
      'content': [
        {
          'type': 'tool_result',
          'content': [
            {'type': 'text', 'text': 'worker runs 4 of 4'},
          ],
        },
      ],
    },
  },
  {
    'type': 'assistant',
    'message': {
      'role': 'assistant',
      'content': [
        {'type': 'text', 'text': 'worker is full.'},
      ],
    },
  },
].map(jsonEncode).join('\n');

class _Reads implements TranscriptReads {
  @override
  Future<Map<String, Object?>> sessionContext(
    String operation,
    String session,
  ) async => {
    'session': session,
    'format': 'claude-code-jsonl',
    'source': 'harness',
    'content': _claude,
  };

  @override
  Future<Map<String, Object?>> conversationLog(String conversation) async => {
    'conversation': conversation,
    'person': 'eve',
    'entries': [
      {'kind': 'person', 'text': 'Which Stations are busy?', 'at': 10},
      {
        'kind': 'tool_call',
        'text': 'board',
        'at': 11,
        'data': {
          'arguments': {'operation': 'G'},
        },
        'turn': {
          'station': 'hq',
          'session': 'provider-1',
          'route': {'handler': 'claude', 'pool': 'team'},
          'account': {'pool': 'team', 'account': 'alice'},
        },
      },
      {
        'kind': 'moneypenny',
        'text': 'worker is full.',
        'at': 12,
        'turn': {
          'station': 'hq',
          'account': {'pool': 'team', 'account': 'alice'},
          'tokens': 321,
        },
      },
    ],
  };

  @override
  Future<Map<String, Object?>> conversationContext(String conversation) async =>
      {
        'conversation': conversation,
        'contexts': [
          {
            'session': 'provider-1',
            'format': 'ai6-events-jsonl',
            'source': 'events',
            'cut_from': 9000,
            'content': '{"kind":"text","text":"Thinking aloud."}\n',
          },
        ],
      };

  @override
  Future<List<Map<String, Object?>>> conversations({
    String? person,
    bool everyone = false,
  }) async => [];
}

Widget _app(Widget home) =>
    OoMaterialApp(themeId: 'light', backdrop: false, title: 'test', home: home);

void main() {
  test("Claude Code's transcript reads as turns, its tool result folded", () {
    final turns = contextTurns('claude-code-jsonl', _claude);
    expect(
      [for (final turn in turns) turn.who],
      ['person', 'tool', 'result', 'agent'],
    );
    expect(turns[1].text, contains('mcp__moneypenny__board'));
    expect(turns[2].folded, isTrue);
    expect(turns[2].text, 'worker runs 4 of 4');
    final codex = contextTurns(
      'codex-rollout-jsonl',
      [
        {
          'type': 'response_item',
          'payload': {
            'type': 'function_call',
            'name': 'exec',
            'arguments': '{}',
          },
        },
        {
          'type': 'response_item',
          'payload': {'type': 'function_call_output', 'output': 'ok'},
        },
      ].map(jsonEncode).join('\n'),
    );
    expect([for (final turn in codex) turn.who], ['tool', 'result']);
    final unknown = contextTurns('something-else', '{"a":1}\nnot json\n');
    expect(unknown.every((turn) => turn.folded), isTrue);
  });

  testWidgets("a Session's context shows its turns, a tool result folded", (
    tester,
  ) async {
    final saved = <String, String>{};
    await tester.pumpWidget(
      _app(
        Scaffold(
          body: SingleChildScrollView(
            child: ContextView(
              data: {
                'session': 'session-g',
                'format': 'claude-code-jsonl',
                'source': 'harness',
                'content': _claude,
              },
              saveText: (name, text) async {
                saved[name] = text;
                return null;
              },
            ),
          ),
        ),
      ),
    );
    expect(find.text('Which Stations are busy?'), findsOneWidget);
    expect(find.text('worker is full.'), findsOneWidget);
    expect(find.textContaining('Tool result · worker runs'), findsOneWidget);
    // Folded: its whole text shows only once opened.
    expect(find.text('worker runs 4 of 4'), findsNothing);
    await tester.tap(find.textContaining('Tool result · worker runs'));
    await tester.pumpAndSettle();
    expect(find.text('worker runs 4 of 4'), findsOneWidget);
    await tester.tap(find.text('Download'));
    await tester.pumpAndSettle();
    expect(saved['context-session-g.jsonl'], _claude);
  });

  testWidgets(
    'the conversation log shows each entry with its turn, and her contexts',
    (tester) async {
      tester.view.physicalSize = const Size(800, 1600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        _app(ConversationLogPage(reads: _Reads(), conversation: 'c-1')),
      );
      await tester.pumpAndSettle();
      expect(find.text('Which Stations are busy?'), findsOneWidget);
      expect(find.textContaining('account team/alice'), findsNWidgets(2));
      expect(find.textContaining('321 tokens'), findsOneWidget);
      expect(find.textContaining('tool_call'), findsOneWidget);
      await tester.tap(find.text('Context'));
      await tester.pumpAndSettle();
      expect(find.textContaining("The Station's event stream"), findsOneWidget);
      expect(find.textContaining('it was 9000 bytes'), findsOneWidget);
      expect(find.text('Thinking aloud.'), findsOneWidget);
    },
  );
}
