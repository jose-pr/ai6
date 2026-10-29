import 'dart:async';
import 'dart:convert';

import 'assistant_controller.dart';
import 'assistant_panel.dart' show moneypennyPrompts;

/// The label the chat shows over the preview's scripted conversation.
const demoConversationLabel = 'Sample conversation — demo';

/// Moneypenny in the preview (`OO_DEMO=true`): a scripted conversation that answers the
/// quick questions from canned replies, everything else with one fallback, and reaches no
/// network. Confirm, Cancel and form submissions change nothing.
///
/// Only the preview builds it; a signed-in app always talks to Universal Exports.
class DemoAssistantChannel implements AssistantChannel {
  DemoAssistantChannel({this.replyDelay = const Duration(milliseconds: 600)});

  /// How long she seems to think before a reply, so the busy state shows.
  final Duration replyDelay;

  final _incoming = StreamController<String>();
  final _timers = <Timer>[];
  var _clock = 0.0;

  @override
  Stream<String> get incoming => _incoming.stream;

  void _say(Map<String, Object?> frame) {
    if (!_incoming.isClosed) _incoming.add(jsonEncode(frame));
  }

  Map<String, Object?> _entry(
    String role,
    String text, [
    Map<String, Object?>? data,
  ]) => {'role': role, 'text': text, 'at': ++_clock, 'data': ?data};

  void _add(Map<String, Object?> entry) =>
      _say({'type': 'entry', 'entry': entry});

  @override
  void send(String frame) {
    final Map<Object?, Object?> message;
    try {
      message = jsonDecode(frame) as Map;
    } on Object {
      return;
    }
    switch (message['type']) {
      case 'hello' || 'new':
        _say({
          'type': 'ready',
          'conversation': 'demo',
          'busy': false,
          'entries': const [],
        });
      case 'message':
        final text = '${message['text'] ?? ''}'.trim();
        _add(_entry('person', text));
        _say({'type': 'busy', 'busy': true});
        _timers.add(
          Timer(replyDelay, () {
            for (final entry in _reply(text)) {
              _add(entry);
            }
            _say({'type': 'busy', 'busy': false});
          }),
        );
      case 'interrupt':
        for (final timer in _timers) {
          timer.cancel();
        }
        _timers.clear();
        _add(_entry('notice', 'Stopped.'));
        _say({'type': 'busy', 'busy': false});
      case 'confirm' || 'cancel' || 'submit':
        _add(_entry('notice', 'This is the demo: nothing was changed.'));
    }
  }

  /// Her answer to [text]: the canned reply of a quick question, else the fallback.
  List<Map<String, Object?>> _reply(String text) {
    final [attention, summary, dark] = moneypennyPrompts;
    if (text == attention) {
      return [
        _entry('tools', 'mcp__moneypenny__board'),
        _entry(
          'moneypenny',
          'Three things: Session **preview-session-008** of mission-007 is blocked '
              'on green ink, a brief for Operation G is held for review, and '
              '**station-remote-02** is dark. Its Missions wait for placement; I can '
              'raise station-control-01\'s capacity so they run there.',
        ),
        _entry('proposal', '', {
          'id': 'demo-proposal',
          'status': 'pending',
          'summary': 'Raise station-control-01 to 6 Missions',
          'request': {
            'method': 'PUT',
            'path': '/v1/stations/station-control-01/settings',
            'body': {'capacity': 6},
          },
          'needs': {'action': 'station.configure', 'on': 'station-control-01'},
        }),
      ];
    }
    if (text == summary) {
      return [
        _entry('tools', 'mcp__moneypenny__board, mcp__moneypenny__missions'),
        _entry(
          'moneypenny',
          'Operation G has one Mission on the board, mission-007, on its second '
              'attempt at station-control-01; one of its Sessions is working and one '
              'is blocked. Operation L\'s mission-021 is done.',
        ),
        _entry('view', '', {
          'id': 'demo-view',
          'view': {
            'version': 1,
            'title': 'The board by Station',
            'blocks': [
              {
                'kind': 'table',
                'columns': ['Station', 'Running', 'Capacity', 'Healthy'],
                'rows': [
                  ['station-control-01', 2, 4, true],
                  ['station-remote-02', 0, 4, false],
                ],
              },
              {
                'kind': 'chart',
                'chart': 'bar',
                'title': 'Sessions by state',
                'y_label': 'Sessions',
                'series': [
                  {
                    'name': 'Sessions',
                    'points': [
                      {'x': 'working', 'y': 1},
                      {'x': 'blocked', 'y': 1},
                      {'x': 'done', 'y': 1},
                    ],
                  },
                ],
              },
            ],
          },
        }),
      ];
    }
    if (text == dark) {
      return [
        _entry('tools', 'mcp__moneypenny__stations'),
        _entry(
          'moneypenny',
          'One: **station-remote-02** stopped checking in. It serves Operation L; '
              'see [the Stations](app:/stations/station-remote-02).',
        ),
      ];
    }
    return [
      _entry(
        'moneypenny',
        'This is the demo, so I only know the sample questions: '
            '${moneypennyPrompts.map((prompt) => '"$prompt"').join(', ')}. Signed in '
            'to Universal Exports I answer from AI6\'s records.',
      ),
    ];
  }

  @override
  Future<void> close() async {
    for (final timer in _timers) {
      timer.cancel();
    }
    _timers.clear();
    await _incoming.close();
  }
}
