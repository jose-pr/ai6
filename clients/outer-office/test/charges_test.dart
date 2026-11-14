import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/charges/attachment.dart';
import 'package:outer_office/charges/charges_page.dart';
import 'package:outer_office/charges/charges_model.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/mission_detail.dart';
import 'package:outer_office/shell/missions_board.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;
import 'package:outer_office/theme/oo_theme.dart';

/// A stand-in for the attach socket: what the app sent, and frames the test makes arrive.
class FakeAttach implements AttachChannel {
  final _frames = StreamController<String>();
  final sent = <Map<String, Object?>>[];
  var closed = false;

  @override
  Stream<String> get incoming => _frames.stream;

  @override
  void send(String frame) =>
      sent.add((jsonDecode(frame) as Map).cast<String, Object?>());

  @override
  Future<void> close() async => closed = true;

  void receive(Map<String, Object?> message) =>
      _frames.add(jsonEncode(message));

  /// A frame of the Run's history.
  void frame(
    int seq,
    String kind,
    Map<String, Object?> data, {
    String? client,
  }) => receive({
    'type': 'frame',
    'frame': {'seq': seq, 'at': 1.0, 'kind': kind, 'data': data},
  });

  /// The gateway attaching this socket's person.
  void attached() =>
      receive({'type': 'attached', 'client': 'c1', 'target': 'charge'});

  /// The socket ending without the Run closing.
  Future<void> drop() => _frames.close();

  Iterable<Map<String, Object?>> ofType(String type) =>
      sent.where((message) => message['type'] == type);
}

Map<String, Object?> _charge(
  String scope, {
  bool held = true,
  bool converse = true,
  bool interrupt = false,
  Map<String, Object?>? extra,
}) => {
  'scope': scope,
  'agent': '007',
  'route': {'handler': 'claude', 'model': 'opus', 'routes': 2},
  'configured': true,
  if (held) 'station': 'worker',
  'held': held,
  if (held) 'lease_expires': 4_000_000_000.0,
  'last_wake': 1_000.0,
  'last_turn': 'woke on a timer',
  'turns_last_hour': 3,
  'can_converse': converse,
  'can_interrupt': interrupt,
  ...?extra,
};

MissionBoard _board(List<Map<String, Object?>> charges) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      http.Response json(Object body) => http.Response(
        jsonEncode(body),
        200,
        headers: {'content-type': 'application/json'},
      );
      final path = request.url.path;
      if (path == '/v1/user/charges') return json(charges);
      if (path.startsWith('/v1/user/charges/')) {
        final scope = Uri.decodeComponent(path.substring(17));
        return json({
          ...charges.firstWhere((charge) => charge['scope'] == scope),
          'logbook': [
            {'at': 900.0, 'kind': 'started', 'text': 'Started a Run'},
            {'at': 1000.0, 'kind': 'turn', 'text': 'woke on a timer'},
          ],
        });
      }
      return json(const <Object>[]);
    }),
  );
  board.accessToken = 'user-token';
  board.loading = false;
  addTearDown(board.dispose);
  return board;
}

class _Host extends StatefulWidget {
  const _Host({required this.board, required this.connect, this.start});

  final MissionBoard board;
  final AttachChannel Function() connect;
  final OoRoute? start;

  @override
  State<_Host> createState() => _HostState();
}

class _HostState extends State<_Host> {
  late OoRoute route = widget.start ?? OoRoute(OoPage.charges);

  @override
  Widget build(BuildContext context) => ChargesPage(
    board: widget.board,
    route: route,
    onRoute: (next) => setState(() => route = next),
    connect: widget.connect,
  );
}

Future<void> _show(WidgetTester tester, Widget child) async {
  tester.view.physicalSize = const Size(1280, 1400);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    MaterialApp(
      theme: ooTheme(OoThemeId.values.first),
      home: Scaffold(body: child),
    ),
  );
  await tester.pumpAndSettle();
}

/// Opens Operation G's charge on the Live tab, attached to a fake socket.
Future<FakeAttach> _attach(
  WidgetTester tester, {
  bool interrupt = false,
  bool converse = true,
}) async {
  final sockets = <FakeAttach>[];
  final board = _board([
    _charge('operation:G', interrupt: interrupt, converse: converse),
  ]);
  await _show(
    tester,
    _Host(
      board: board,
      connect: () {
        sockets.add(FakeAttach());
        return sockets.last;
      },
    ),
  );
  await tester.tap(find.byKey(const ValueKey('charge-operation:G')));
  await tester.pumpAndSettle();
  return sockets.isEmpty ? FakeAttach() : sockets.last;
}

void main() {
  testWidgets(
    'the list shows each charge: state, Agent, where it runs, last wake',
    (tester) async {
      final board = _board([
        _charge('m', converse: false),
        _charge('operation:G'),
        _charge('station:worker', held: false),
      ]);
      await _show(tester, _Host(board: board, connect: FakeAttach.new));

      expect(find.text('M'), findsOneWidget);
      expect(find.text('Operation G'), findsOneWidget);
      expect(find.text('Station worker'), findsOneWidget);
      expect(find.text('running'), findsNWidgets(2));
      expect(find.text('stopped'), findsOneWidget);
      expect(
        find.text('007 · claude · opus (+1 more routes)'),
        findsNWidgets(3),
      );
      expect(find.text('Runs on Station worker'), findsNWidgets(2));
      expect(find.text('Not running: no Station holds it'), findsOneWidget);
      expect(find.text('woke on a timer'), findsNWidgets(3));
      expect(find.text('3 turns in the last hour'), findsNWidgets(3));
      // No word of Watch or Session in what a person reads.
      expect(find.textContaining('Watch'), findsNothing);
      expect(find.textContaining('Session'), findsNothing);
    },
  );

  testWidgets('history is replayed, then live frames follow, each kind drawn', (
    tester,
  ) async {
    final socket = await _attach(tester);
    expect(socket.sent.single, {
      'type': 'hello',
      'token': 'user-token',
      'charge': 'operation:G',
    });
    expect(find.byKey(const ValueKey('attach-prompt')), findsOneWidget);

    socket.attached();
    socket.frame(1, 'prompt', {'from': 'ana', 'text': 'status?'});
    socket.frame(2, 'event', {'kind': 'text', 'text': 'All is quiet.'});
    socket.frame(3, 'ready', {'busy': false, 'attached': 1, 'last': 3});
    await tester.pumpAndSettle();
    expect(find.text('From ana'), findsOneWidget);
    expect(find.text('status?'), findsOneWidget);
    expect(find.text('All is quiet.'), findsOneWidget);
    expect(find.text('Replay complete'), findsOneWidget);
    expect(find.text('Idle'), findsOneWidget);

    socket.frame(4, 'turn', {'state': 'begun', 'by': 'ana'});
    socket.frame(5, 'event', {
      'kind': 'tool',
      'text': 'Run a command',
      'tools': ['shell'],
    });
    socket.frame(6, 'attached', {'person': 'bo'});
    socket.frame(7, 'detached', {'person': 'bo'});
    socket.frame(8, 'interrupted', {'by': 'ana'});
    await tester.pumpAndSettle();
    expect(find.text('Turn begun by ana'), findsOneWidget);
    expect(find.text('Taking a turn'), findsOneWidget);
    expect(find.text('Run a command'), findsOneWidget);
    expect(find.text('Tools: shell'), findsOneWidget);
    expect(find.text('bo attached'), findsOneWidget);
    expect(find.text('bo detached'), findsOneWidget);
    expect(find.text('Interrupted by ana'), findsOneWidget);

    socket.frame(9, 'closed', {'reason': 'the charge was removed'});
    await tester.pumpAndSettle();
    expect(
      find.text('This Run closed: the charge was removed'),
      findsOneWidget,
    );
    expect(find.text('This Run has closed'), findsOneWidget);
    // Nothing is sent to a Run that has gone.
    await tester.enterText(find.byKey(const ValueKey('attach-prompt')), 'hi');
    expect(
      tester
          .widget<FilledButton>(find.byKey(const ValueKey('attach-send')))
          .onPressed,
      isNull,
    );
  });

  testWidgets('a prompt is sent as the person typed it', (tester) async {
    final socket = await _attach(tester);
    socket.attached();
    socket.frame(1, 'ready', {'busy': false});
    await tester.pumpAndSettle();

    await tester.enterText(
      find.byKey(const ValueKey('attach-prompt')),
      'what is G-1 doing?',
    );
    await tester.tap(find.byKey(const ValueKey('attach-send')));
    await tester.pumpAndSettle();
    expect(socket.ofType('prompt').single['text'], 'what is G-1 doing?');
    // The Station echoes it back stamped with who sent it.
    socket.frame(2, 'prompt', {'from': 'ana', 'text': 'what is G-1 doing?'});
    await tester.pumpAndSettle();
    expect(find.text('From ana'), findsOneWidget);
  });

  testWidgets(
    'the first answer to a permission wins and disables the buttons',
    (tester) async {
      final socket = await _attach(tester);
      socket.attached();
      socket.frame(1, 'permission', {
        'request': 7,
        'tool': 'Run a command',
        'detail': 'git push',
        'options': [
          {'id': 'allow', 'label': 'Allow once'},
          {'id': 'deny', 'label': 'Deny'},
        ],
      });
      socket.frame(2, 'ready', {'busy': true});
      await tester.pumpAndSettle();
      expect(find.text('Asks permission: Run a command'), findsOneWidget);
      expect(find.text('git push'), findsOneWidget);

      Finder button(String id) => find.byKey(ValueKey('permission-7-$id'));
      expect(
        tester.widget<OutlinedButton>(button('allow')).onPressed,
        isNotNull,
      );
      await tester.tap(button('allow'));
      await tester.pumpAndSettle();
      expect(socket.ofType('answer').single, {
        'type': 'answer',
        'request': 7,
        'option': 'allow',
      });

      // Another attached person answered first: theirs counts, and both buttons are off.
      socket.frame(3, 'answered', {'request': 7, 'by': 'bo', 'option': 'deny'});
      await tester.pumpAndSettle();
      expect(tester.widget<OutlinedButton>(button('allow')).onPressed, isNull);
      expect(tester.widget<OutlinedButton>(button('deny')).onPressed, isNull);
      expect(
        find.byKey(const ValueKey('permission-7-answered')),
        findsOneWidget,
      );
      expect(find.text('Answered by bo: deny'), findsOneWidget);
      // A later answer is never sent.
      await tester.tap(button('deny'), warnIfMissed: false);
      expect(socket.ofType('answer'), hasLength(1));
    },
  );

  testWidgets('Interrupt shows only to a person who holds the right', (
    tester,
  ) async {
    final without = await _attach(tester);
    without.attached();
    without.frame(1, 'ready', {'busy': true});
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('attach-interrupt')), findsNothing);
    await tester.pumpWidget(const SizedBox());

    final withRight = await _attach(tester, interrupt: true);
    withRight.attached();
    withRight.frame(1, 'ready', {'busy': true});
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('attach-interrupt')));
    await tester.pumpAndSettle();
    expect(withRight.ofType('interrupt'), hasLength(1));
  });

  testWidgets('a person without charge.converse is offered no controls', (
    tester,
  ) async {
    final socket = await _attach(tester, converse: false);
    expect(socket.sent, isEmpty, reason: 'nothing is attached to');
    expect(find.byKey(const ValueKey('attach-prompt')), findsNothing);
    expect(find.byKey(const ValueKey('attach-send')), findsNothing);
    expect(find.byKey(const ValueKey('attach-interrupt')), findsNothing);
    expect(find.textContaining('needs charge.converse'), findsOneWidget);
  });

  testWidgets('a dropped socket offers Reconnect from the last seq', (
    tester,
  ) async {
    final sockets = <FakeAttach>[];
    final board = _board([_charge('operation:G')]);
    await _show(
      tester,
      _Host(
        board: board,
        connect: () {
          sockets.add(FakeAttach());
          return sockets.last;
        },
      ),
    );
    await tester.tap(find.byKey(const ValueKey('charge-operation:G')));
    await tester.pumpAndSettle();
    final first = sockets.single;
    first.attached();
    first.frame(1, 'event', {'kind': 'text', 'text': 'one'});
    first.frame(2, 'event', {'kind': 'text', 'text': 'two'});
    first.frame(3, 'ready', {'busy': false});
    await tester.pumpAndSettle();

    await first.drop();
    await tester.pumpAndSettle();
    expect(find.text('Disconnected'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('attach-reconnect')));
    await tester.pumpAndSettle();

    final second = sockets.last;
    expect(sockets, hasLength(2));
    expect(second.sent.single['type'], 'hello');
    expect(second.sent.single['after'], 3);
    // The Run replays from there; what was already shown is not shown twice.
    second.attached();
    second.frame(3, 'ready', {'busy': false});
    second.frame(4, 'event', {'kind': 'text', 'text': 'three'});
    await tester.pumpAndSettle();
    expect(find.text('one'), findsOneWidget);
    expect(find.text('three'), findsOneWidget);
    expect(find.text('Replay complete'), findsOneWidget);
    expect(find.byKey(const ValueKey('attach-reconnect')), findsNothing);
  });

  testWidgets('a refusal shows the gateway\'s own words and is final', (
    tester,
  ) async {
    final socket = await _attach(tester);
    socket.receive({
      'type': 'error',
      'code': 'forbidden',
      'message': 'You may no longer attach to that Run.',
    });
    await socket.drop();
    await tester.pumpAndSettle();
    expect(find.text('You may no longer attach to that Run.'), findsOneWidget);
    expect(find.text('Not attached'), findsOneWidget);
    expect(find.byKey(const ValueKey('attach-reconnect')), findsNothing);
  });

  testWidgets('a command the Run refused is shown and the attachment stands', (
    tester,
  ) async {
    final socket = await _attach(tester);
    socket.attached();
    socket.frame(1, 'ready', {'busy': false});
    socket.frame(0, 'error', {'reason': 'the prompt is too long'});
    socket.receive({
      'type': 'error',
      'code': 'busy',
      'message': 'Too many requests wait for the Station.',
    });
    await tester.pumpAndSettle();
    expect(find.text('Refused: the prompt is too long'), findsOneWidget);
    expect(
      find.text('Too many requests wait for the Station.'),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('attach-prompt')), findsOneWidget);
  });

  testWidgets('leaving the view detaches', (tester) async {
    final socket = await _attach(tester);
    socket.attached();
    socket.frame(1, 'ready', {'busy': false});
    await tester.pumpAndSettle();
    await tester.pumpWidget(const SizedBox());
    await tester.pumpAndSettle();
    expect(socket.ofType('detach'), hasLength(1));
    expect(socket.closed, isTrue);
  });

  testWidgets('the logbook tab reads the charge\'s logbook, newest first', (
    tester,
  ) async {
    final board = _board([_charge('operation:G')]);
    await _show(
      tester,
      _Host(
        board: board,
        connect: FakeAttach.new,
        start: OoRoute(OoPage.charges)
            .withField('operation_id', 'G')
            .withField('tab', 'logbook'),
      ),
    );
    expect(find.text('Started a Run'), findsOneWidget);
    expect(find.text('woke on a timer'), findsNWidgets(2));
    await tester.tap(find.byKey(const ValueKey('charge-tab-decisions')));
    await tester.pumpAndSettle();
    expect(find.text('woke on a timer'), findsNWidgets(2));
    expect(find.text('Started a Run'), findsNothing);
  });

  group('a Mission\'s live Run', () {
    MissionBoard missionBoard(List<String> actions) {
      final board = _board(const []);
      board.userSession = UserSessionSummary(
        personId: 'ana',
        name: 'ana',
        displayName: null,
        groups: const [],
        expiresAt: 0,
        operations: [UserOperationAccess(operationId: 'G', actions: actions)],
      );
      return board;
    }

    Future<List<FakeAttach>> detail(
      WidgetTester tester,
      MissionBoard board,
    ) async {
      final sockets = <FakeAttach>[];
      await _show(
        tester,
        MissionDetail(
          board: board,
          mission: const MissionGroup(
            operationId: 'G',
            missionId: 'm1',
            runs: [
              RunSummary(
                operationId: 'G',
                missionId: 'm1',
                runId: 's1',
                state: 'working',
                sequence: 1,
                stationId: 'hq',
              ),
            ],
          ),
          pages: const [OoPage.missions],
          onRoute: (_) {},
          onOpenRun: (_) {},
          onClose: () {},
          attachConnect: () {
            sockets.add(FakeAttach());
            return sockets.last;
          },
        ),
      );
      return sockets;
    }

    testWidgets('is offered under mission.control and attaches when opened', (
      tester,
    ) async {
      final sockets = await detail(
        tester,
        missionBoard(['mission.read', 'mission.control']),
      );
      expect(find.text('Live Run'), findsOneWidget);
      expect(sockets, isEmpty, reason: 'nothing attaches until it is opened');
      await tester.tap(find.text('Live Run'));
      await tester.pumpAndSettle();
      expect(sockets.single.sent.single, {
        'type': 'hello',
        'token': 'user-token',
        'mission': {'operation': 'G', 'mission': 'm1'},
      });
      sockets.single.attached();
      sockets.single.frame(1, 'ready', {'busy': true});
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('attach-interrupt')), findsOneWidget);
    });

    testWidgets('is not offered without mission.control', (tester) async {
      final sockets = await detail(tester, missionBoard(['mission.read']));
      expect(find.text('Live Run'), findsNothing);
      expect(sockets, isEmpty);
    });
  });

  test('a charge is told as the gateway lists it', () {
    final charge = ChargeSummary.fromJson(
      _charge('operation:G', interrupt: true),
    );
    expect(charge.title, 'Operation G');
    expect(charge.operationId, 'G');
    expect(charge.state, 'running');
    expect(chargeRouteFields('station:worker'), {'station_id': 'worker'});
    expect(chargeRouteFields('m'), {'charge': 'm'});
  });
}
