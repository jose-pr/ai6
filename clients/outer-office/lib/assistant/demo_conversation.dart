import 'dart:async';
import 'dart:convert';

import 'assistant_controller.dart';
import 'assistant_panel.dart' show moneypennyPrompts;

/// The label the chat shows over the preview's scripted conversation.
const demoConversationLabel = 'Sample conversation — demo';

/// Moneypenny in the preview (`OO_DEMO=true`): a scripted conversation that answers the
/// quick questions from canned replies, everything else with one fallback, and reaches no
/// network. Confirm, Cancel and form submissions change nothing; her page cards open their
/// pages, which only read.
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
        _entry('navigation', 'The blocked Sessions of Operation G', {
          'id': 'demo-nav-blocked',
          'route': {
            'page': 'sessions',
            'operation_id': 'G',
            'state': 'blocked',
          },
          'label': 'The blocked Sessions of Operation G',
        }),
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
        // Her missions tool read briefs, text from outside AI6, so her views are stamped.
        _entry('view', '', {
          'id': 'demo-view',
          'exposed': true,
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
        _entry('view', '', {
          'id': 'demo-view-g',
          'exposed': true,
          'view': _operationG,
        }),
        // The records she summarised; stamped like her views, so it waits for Open.
        _entry('navigation', 'The Missions of Operation G I summarised', {
          'id': 'demo-nav-g',
          'exposed': true,
          'route': {'page': 'missions', 'operation_id': 'G'},
          'label': 'The Missions of Operation G I summarised',
        }),
      ];
    }
    if (text == dark) {
      return [
        _entry('tools', 'mcp__moneypenny__stations'),
        _entry(
          'moneypenny',
          'One: **station-remote-02** stopped checking in. It serves Operation L; '
              'see [its card](app:/stations?station_id=station-remote-02).',
        ),
        _entry('navigation', 'station-remote-02 on the Stations page', {
          'id': 'demo-nav-dark',
          'route': {'page': 'stations', 'station_id': 'station-remote-02'},
          'label': 'station-remote-02 on the Stations page',
        }),
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

  /// Operation G as the sample's second view: its attempts as a schedule, where it runs as a
  /// graph, Missions by state and Sessions by Station as charts, and a drawing.
  static const Map<String, Object?> _operationG = {
    'version': 1,
    'title': 'Operation G',
    'blocks': [
      {
        'kind': 'schedule',
        'title': 'Attempts of mission-007',
        'rows': [
          {
            'label': 'attempt 1',
            'start': '2026-10-29T08:00:00Z',
            'end': '2026-10-29T08:40:00Z',
            'state': 'failed',
            'link': 'app:/missions?operation_id=G&mission_id=mission-007',
          },
          {
            'label': 'attempt 2',
            'start': '2026-10-29T08:45:00Z',
            'end': '2026-10-29T09:30:00Z',
            'state': 'working',
          },
        ],
      },
      {
        'kind': 'graph',
        'title': 'Where G runs',
        'nodes': [
          {'id': 'G', 'label': 'Operation G', 'kind': 'operation'},
          {
            'id': 'control',
            'label': 'station-control-01',
            'kind': 'station',
            'state': 'working',
            'link': 'app:/stations?station_id=station-control-01',
          },
          {'id': 'build', 'label': 'build-box', 'kind': 'theater'},
          {
            'id': 'mission',
            'label': 'mission-007',
            'kind': 'mission',
            'state': 'blocked',
          },
        ],
        'edges': [
          {'from': 'G', 'to': 'control'},
          {'from': 'control', 'to': 'build', 'label': 'ssh'},
          {'from': 'build', 'to': 'mission'},
        ],
      },
      {
        'kind': 'chart',
        'chart': 'bar',
        'stacked': true,
        'title': 'Missions by state',
        'x_label': 'Operation',
        'series': [
          {
            'name': 'done',
            'points': [
              {'x': 'G', 'y': 4},
              {'x': 'L', 'y': 1},
            ],
          },
          {
            'name': 'working',
            'points': [
              {'x': 'G', 'y': 1},
            ],
          },
          {
            'name': 'blocked',
            'points': [
              {'x': 'G', 'y': 1},
            ],
          },
        ],
      },
      {
        'kind': 'chart',
        'chart': 'pie',
        'title': 'Sessions by Station',
        'series': [
          {
            'name': 'Sessions',
            'points': [
              {'x': 'station-control-01', 'y': 2},
              {'x': 'station-remote-02', 'y': 1},
            ],
          },
        ],
      },
      {
        'kind': 'svg',
        'title': 'station-control-01 hands mission-007 to build-box',
        'svg':
            '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 80">'
            '<title>station-control-01 hands mission-007 to build-box</title>'
            '<rect x="10" y="20" width="80" height="40" rx="6" fill="none" '
            'stroke="currentColor"/>'
            '<text x="50" y="45" text-anchor="middle" fill="currentColor" '
            'font-size="12">control-01</text>'
            '<line x1="90" y1="40" x2="150" y2="40" stroke="currentColor" '
            'stroke-width="2"/>'
            '<rect x="150" y="20" width="80" height="40" rx="6" fill="none" '
            'stroke="currentColor"/>'
            '<text x="190" y="45" text-anchor="middle" fill="currentColor" '
            'font-size="12">build-box</text>'
            '</svg>',
      },
    ],
  };

  @override
  Future<void> close() async {
    for (final timer in _timers) {
      timer.cancel();
    }
    _timers.clear();
    await _incoming.close();
  }
}
