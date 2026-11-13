import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/settings/oo_config.dart';

/// A socket whose failure the test decides: nothing happens until it says.
class SilentChannel implements AssistantChannel {
  final _incoming = StreamController<String>();
  final sent = <String>[];

  void fail() => _incoming.addError(StateError('refused'));
  void ready() => _incoming.add(jsonEncode({'type': 'ready', 'entries': []}));

  @override
  Stream<String> get incoming => _incoming.stream;

  @override
  void send(String frame) => sent.add(frame);

  @override
  Future<void> close() async {
    if (!_incoming.isClosed) await _incoming.close();
  }
}

Map<String, Object?> profile({bool moneypenny = false}) => {
  'person_id': 'ana',
  'name': 'ana',
  'display_name': 'Ana Ortiz',
  'groups': const <String>[],
  'expires_at': 0,
  'operations': [
    {
      'operation_id': 'G',
      'actions': [
        'mission.read',
        'mission.create',
        'audit.read',
        'agent.read',
        'pool.read',
      ],
    },
    {
      'operation_id': 'L',
      'actions': ['mission.read', 'mission.create'],
    },
  ],
  'stations': const <Object>[],
  'moneypenny': moneypenny,
};

/// A gateway whose every read answers, counting them by path.
MockClient gateway(
  Map<String, int> counts, {
  bool moneypenny = false,
  int Function(String path)? status,
}) => MockClient((request) async {
  final path = request.url.path;
  counts.update(path, (count) => count + 1, ifAbsent: () => 1);
  const json = {'content-type': 'application/json'};
  final code = status?.call(path) ?? 200;
  if (path == '/v1/auth/login') {
    return http.Response(jsonEncode({'token': 'token'}), 201, headers: json);
  }
  if (path == '/v1/user/profile') {
    return http.Response(
      jsonEncode(profile(moneypenny: moneypenny)),
      code,
      headers: json,
    );
  }
  if (path == '/v1/stations') {
    return http.Response(
      jsonEncode({'headquarters_station': null, 'stations': []}),
      code,
      headers: json,
    );
  }
  return http.Response('[]', code, headers: json);
});

Widget app(MissionBoard board) =>
    OuterOfficeApp(board: board, desktopWindows: false);

/// Pumps [app] until the sign-in's reads are done.
Future<void> settle(WidgetTester tester) async {
  await tester.pump(const Duration(milliseconds: 10));
  await tester.pump(const Duration(milliseconds: 10));
}

void main() {
  group('Moneypenny off on this deployment', () {
    testWidgets(
      'no card, no icon, no chat; Settings says so and who can change it',
      (tester) async {
        tester.view.physicalSize = const Size(1400, 1000);
        tester.view.devicePixelRatio = 1;
        addTearDown(tester.view.reset);
        var opened = 0;
        final board = MissionBoard(
          apiOrigin: Uri.parse('https://oo.example'),
          pollInterval: null,
          client: gateway({}),
          assistantChannel: () {
            opened++;
            return SilentChannel();
          },
        );
        await tester.pumpWidget(app(board));
        await board.login('ana', 'secret');
        await tester.pumpAndSettle();
        expect(find.text('MONEYPENNY'), findsNothing);
        expect(
          find.textContaining('What would you like to know'),
          findsNothing,
        );
        expect(find.byTooltip('Ask Moneypenny (Ctrl+Alt+I)'), findsNothing);
        expect(find.byType(AssistantPanel), findsNothing);
        expect(find.text('Ask Moneypenny…'), findsNothing);
        expect(opened, 0, reason: 'nothing may open her socket');
        await tester.tap(find.byTooltip('Your account and settings'));
        await tester.pumpAndSettle();
        expect(find.text(moneypennyOffNote), findsOneWidget);
        expect(moneypennyOffNote, contains('off on this deployment'));
        expect(moneypennyOffNote, contains('Universal Exports'));
        await tester.pumpWidget(const SizedBox());
        board.dispose();
      },
    );

    testWidgets('turned off while her socket is down, it does not come back', (
      tester,
    ) async {
      var served = true;
      var opened = 0;
      SilentChannel? last;
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: MockClient((request) async {
          const json = {'content-type': 'application/json'};
          if (request.url.path == '/v1/auth/login') {
            return http.Response(
              jsonEncode({'token': 'token'}),
              201,
              headers: json,
            );
          }
          if (request.url.path == '/v1/user/profile') {
            return http.Response(
              jsonEncode(profile(moneypenny: served)),
              200,
              headers: json,
            );
          }
          return http.Response('[]', 200, headers: json);
        }),
        assistantChannel: () {
          opened++;
          return last = SilentChannel();
        },
      );
      await tester.pumpWidget(app(board));
      await board.login('ana', 'secret');
      await settle(tester);
      await tester.pump(const Duration(seconds: 1));
      expect(opened, greaterThan(0));
      last!.ready();
      await tester.pump();
      last!.fail();
      await tester.pump();
      served = false;
      await board.loadUserSession();
      await tester.pump();
      final before = opened;
      await tester.pump(const Duration(hours: 1));
      expect(opened, before);
      await tester.pumpWidget(const SizedBox());
      board.dispose();
    });

    testWidgets('served, Settings says nothing about her being off', (
      tester,
    ) async {
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: gateway({}, moneypenny: true),
        assistantChannel: SilentChannel.new,
      );
      await tester.pumpWidget(app(board));
      await board.login('ana', 'secret');
      await tester.pumpAndSettle();
      expect(find.text('MONEYPENNY'), findsOneWidget);
      await tester.tap(find.byTooltip('Your account and settings'));
      await tester.pumpAndSettle();
      expect(find.text(moneypennyOffNote), findsNothing);
      await tester.pumpWidget(const SizedBox());
      // Her connect timer ends with the app.
      await tester.pump(const Duration(seconds: 1));
      board.dispose();
    });
  });

  group('Moneypenny on, her socket decides what the app says', () {
    late SilentChannel channel;
    late MissionBoard board;

    Future<void> open(WidgetTester tester) async {
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      channel = SilentChannel();
      board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: gateway({}, moneypenny: true),
        assistantChannel: () => channel,
      );
      await tester.pumpWidget(app(board));
      await board.login('ana', 'secret');
      await tester.pump();
      await tester.pump();
    }

    Future<void> close(WidgetTester tester) async {
      await tester.pumpWidget(const SizedBox());
      await tester.pump(const Duration(seconds: 1));
      board.dispose();
    }

    testWidgets('no greeting until the socket says she is there', (
      tester,
    ) async {
      await open(tester);
      expect(find.textContaining('What would you like to know'), findsNothing);
      expect(find.text('Connecting to Moneypenny…'), findsOneWidget);
      channel.ready();
      await tester.pump();
      await tester.pump();
      expect(
        find.textContaining('What would you like to know'),
        findsOneWidget,
      );
      await close(tester);
    });

    testWidgets('a failed socket says so and offers to try again', (
      tester,
    ) async {
      await open(tester);
      channel.fail();
      await tester.pump();
      await tester.pump();
      expect(find.text('The connection to Moneypenny failed.'), findsOneWidget);
      expect(find.textContaining('What would you like to know'), findsNothing);
      expect(tester.takeException(), isNull);
      final retry = find.widgetWithText(TextButton, 'Try again');
      expect(retry, findsOneWidget);
      final first = channel;
      channel = SilentChannel();
      await tester.tap(retry);
      await tester.pump();
      expect(channel, isNot(same(first)));
      expect(channel.sent, hasLength(1), reason: 'the retry says hello again');
      expect(find.text('Connecting to Moneypenny…'), findsOneWidget);
      await close(tester);
    });

    testWidgets(
      'a socket that stays silent ends in a retry, not an endless Connecting',
      (tester) async {
        await open(tester);
        await tester.pump(ChatSettings.defaultConnectTimeout);
        await tester.pump();
        expect(find.text('Connecting to Moneypenny…'), findsNothing);
        expect(find.text('Moneypenny did not answer.'), findsOneWidget);
        expect(find.widgetWithText(TextButton, 'Try again'), findsOneWidget);
        await close(tester);
      },
    );
  });

  group('the chat panel', () {
    testWidgets('a socket that closes shows why and a retry, not Connecting', (
      tester,
    ) async {
      final channel = SilentChannel();
      final controller = AssistantController(
        token: 't',
        connect: () => channel,
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: AssistantPanel(controller: controller)),
        ),
      );
      expect(find.text('Connecting…'), findsOneWidget);
      await channel.close();
      await tester.pump();
      await tester.pump();
      expect(find.text('Connecting…'), findsNothing);
      expect(find.text('Reconnecting…'), findsOneWidget);
      expect(find.text('The connection to Moneypenny closed.'), findsOneWidget);
      expect(find.widgetWithText(TextButton, 'Try again'), findsOneWidget);
      controller.dispose();
    });
  });

  test('a real socket that cannot connect leaves nothing uncaught', () async {
    final uncaught = <Object>[];
    await runZonedGuarded(() async {
      // Nothing listens on port 1.
      final controller = AssistantController(
        token: 't',
        connect: () => connectAssistant(Uri.parse('http://127.0.0.1:1')),
      );
      final failed = Completer<void>();
      controller.addListener(() {
        if (controller.problem != null && !failed.isCompleted) {
          failed.complete();
        }
      });
      controller.open();
      await failed.future.timeout(const Duration(seconds: 20));
      expect(controller.connected, isFalse);
      expect(controller.connecting, isFalse);
      expect(controller.problem, 'The connection to Moneypenny failed.');
      // `ready` fails on the same turn as the stream's error, or just after.
      await Future<void>.delayed(const Duration(milliseconds: 100));
      controller.dispose();
    }, (error, _) => uncaught.add(error));
    expect(uncaught, isEmpty);
  });
}
