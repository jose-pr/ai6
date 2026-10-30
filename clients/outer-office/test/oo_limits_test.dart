import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/settings/oo_config.dart';

void main() {
  test('every limit defaults to the value Outer Office used before', () {
    final limits = OoConfig.parse('').limits;
    expect(limits.refresh.sessions, const Duration(seconds: 3));
    expect(limits.refresh.overview, const Duration(seconds: 10));
    expect(limits.refresh.timeline, const Duration(seconds: 10));
    expect(limits.refresh.pools, const Duration(seconds: 15));
    expect(limits.controls.wait, const Duration(seconds: 10));
    expect(limits.controls.poll, const Duration(milliseconds: 500));
    expect(limits.pages.timeline, 50);
    expect(limits.pages.audit, 100);
    expect(limits.terminal.scrollbackLines, 10000);
    expect(limits.terminal.connectTimeout, const Duration(seconds: 10));
    expect(limits.terminal.retryDelay, const Duration(seconds: 1));
    expect(limits.terminal.retryMaxDelay, const Duration(seconds: 16));
    expect(limits.inputs.missionTitleChars, 512);
    expect(limits.inputs.missionPromptChars, 65536);
    expect(limits.inputs.reasonChars, 512);
  });

  test(
    'the tables set each value in any format, and 0 turns a refresh off',
    () {
      final limits = OoConfig.parse(
        '[refresh]\nsessions = 7\noverview = 0\ntimeline = 2.5\n'
        '[controls]\nwait = 0\npoll = 0.25\n'
        '[pages]\ntimeline = 20\naudit = 500\n'
        '[terminal]\nscrollback_lines = 200\nconnect_timeout = 3\n'
        'retry_delay = 0.5\nretry_max_delay = 4\n'
        '[limits]\nmission_title_chars = 80\nmission_prompt_chars = 0\n'
        'reason_chars = 120\n',
      ).limits;
      expect(limits.refresh.sessions, const Duration(seconds: 7));
      expect(limits.refresh.overview, isNull);
      expect(limits.refresh.timeline, const Duration(milliseconds: 2500));
      expect(limits.refresh.pools, const Duration(seconds: 15));
      expect(limits.controls.wait, Duration.zero);
      expect(limits.controls.poll, const Duration(milliseconds: 250));
      expect([limits.pages.timeline, limits.pages.audit], [20, 500]);
      expect(limits.terminal.scrollbackLines, 200);
      expect(limits.terminal.connectTimeout, const Duration(seconds: 3));
      expect(limits.terminal.retryDelay, const Duration(milliseconds: 500));
      expect(limits.terminal.retryMaxDelay, const Duration(seconds: 4));
      expect(limits.inputs.missionTitleChars, 80);
      expect(InputSettings.field(limits.inputs.missionPromptChars), isNull);
      expect(InputSettings.field(limits.inputs.reasonChars), 120);
      final json = OoConfig.parse('#!json\n{"refresh": {"pools": 9}}').limits;
      expect(json.refresh.pools, const Duration(seconds: 9));
    },
  );

  test('nonsense and unknown keys are refused, naming the key', () {
    for (final (text, message) in [
      ('[refresh]\nsessions = -1\n', 'refresh.sessions'),
      ('[refresh]\nsessions = "soon"\n', 'refresh.sessions'),
      ('[controls]\npoll = 0\n', 'controls.poll'),
      ('[pages]\ntimeline = 0\n', 'pages.timeline'),
      ('[pages]\naudit = 1.5\n', 'pages.audit'),
      ('[terminal]\nretry_delay = 0\n', 'terminal.retry_delay'),
      ('[limits]\nreason_chars = -2\n', 'limits.reason_chars'),
      ('[refresh]\nnever = 1\n', 'refresh.never'),
      ('[terminal]\nnope = 1\n', 'terminal.nope'),
    ]) {
      expect(
        () => OoConfig.parse(text),
        throwsA(
          isA<FormatException>().having(
            (error) => error.message,
            'message',
            contains(message),
          ),
        ),
        reason: text,
      );
    }
  });

  testWidgets('[refresh] sessions = 7 makes the board ask every 7 seconds', (
    tester,
  ) async {
    final config = OoConfig.parse('[refresh]\nsessions = 7\n');
    var boards = 0;
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      pollInterval: config.limits.refresh.sessions,
      limits: config.limits,
      client: MockClient((request) async {
        if (request.url.path == '/v1/board') boards++;
        return http.Response('[]', 200);
      }),
    );
    addTearDown(board.dispose);
    board.accessToken = 'operator-token';

    await tester.pump(const Duration(seconds: 6));
    expect(boards, 0, reason: 'nothing before the first 7 seconds');
    await tester.pump(const Duration(seconds: 1));
    expect(boards, 1);
    await tester.pump(const Duration(seconds: 7));
    expect(boards, 2);
  });

  testWidgets('a refresh of 0 starts no timer at all', (tester) async {
    final config = OoConfig.parse('[refresh]\nsessions = 0\n');
    var boards = 0;
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      pollInterval: config.limits.refresh.sessions,
      limits: config.limits,
      client: MockClient((request) async {
        if (request.url.path == '/v1/board') boards++;
        return http.Response('[]', 200);
      }),
    );
    addTearDown(board.dispose);
    board.accessToken = 'operator-token';
    await tester.pump(const Duration(minutes: 5));
    expect(boards, 0);
  });

  test('[pages] sets what the timeline and the audit log ask for', () async {
    final asked = <Uri>[];
    final config = OoConfig.parse('[pages]\ntimeline = 7\naudit = 9\n');
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      pollInterval: null,
      limits: config.limits,
      client: MockClient((request) async {
        asked.add(request.url);
        return http.Response(
          request.url.path.endsWith('/timeline')
              ? '{"items": [], "next_cursor": null}'
              : '[]',
          200,
          headers: {'content-type': 'application/json'},
        );
      }),
    );
    board.accessToken = 'operator-token';
    await board.loadTimeline(operationId: 'G');
    await board.loadAudit();
    expect(asked[0].queryParameters['limit'], '7');
    expect(asked[1].queryParameters['limit'], '9');
    await board.loadTimeline(operationId: 'G', limit: 3);
    expect(
      asked[2].queryParameters['limit'],
      '3',
      reason: 'a caller still wins',
    );
  });
}
