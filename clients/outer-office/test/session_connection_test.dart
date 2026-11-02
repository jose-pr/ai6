import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:guatiao/wire.dart' as wire;
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/terminal/session_connection.dart';

void main() {
  test(
    'Session connection replays bytes, renews leases, and sends input',
    () async {
      final frames = await _readProtocolFrames();
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final socketWaiters = List.generate(5, (_) => Completer<_SocketAttach>());
      final attachRequests = <Map<String, Object?>>[];
      final inputReceived = Completer<Map<String, Object?>>();
      final resizeReceived = Completer<Map<String, Object?>>();

      server.listen((request) async {
        final index = socketWaiters.indexWhere((waiter) => !waiter.isCompleted);
        final socket = await WebSocketTransformer.upgrade(
          request,
          protocolSelector: (protocols) =>
              protocols.contains('ai6.guatiao') ? 'ai6.guatiao' : null,
        );
        final accepted = _SocketAttach(
          path: request.uri.path,
          after: request.uri.queryParameters['after'],
          cookie: request.headers.value(HttpHeaders.cookieHeader),
          origin: request.headers.value('origin'),
          forwardedProto: request.headers.value('x-forwarded-proto'),
          protocol: request.headers.value('sec-websocket-protocol'),
        );
        socketWaiters[index].complete(accepted);

        socket.add(frames['terminal_schema']!);
        socket.add(frames['resize_schema']!);
        if (index == 0) {
          socket.add(frames['lifecycle_schema']!);
          socket.add(frames['lifecycle_value']!);
          socket.add(frames['terminal_value']!);
          await Future<void>.delayed(const Duration(milliseconds: 100));
          await socket.close();
          return;
        }

        if (index == 2) {
          final receiver = wire.Receiver()
            ..accept(frames['terminal_schema']!)
            ..accept(frames['resize_schema']!);
          socket.listen((message) {
            if (message is! List<int>) return;
            final event = receiver.accept(message);
            final value = event.value;
            if (value == null) return;
            try {
              final raw = value.toDart();
              if (raw is! Map) return;
              final frame = Map<String, Object?>.from(raw);
              if (event.channel == 2 && !inputReceived.isCompleted) {
                inputReceived.complete(frame);
              } else if (event.channel == 3 && !resizeReceived.isCompleted) {
                resizeReceived.complete(frame);
              }
              if (inputReceived.isCompleted && resizeReceived.isCompleted) {
                unawaited(socket.close(1008, 'attachment credential expired'));
              }
            } finally {
              value.close();
            }
          }, onDone: receiver.close);
        }
      });

      final origin = Uri.parse('http://127.0.0.1:${server.port}');
      var grantCount = 0;
      final board = MissionBoard(
        apiOrigin: origin,
        pollInterval: null,
        client: MockClient((request) async {
          if (request.url.path == '/v1/user/attach-grants') {
            final body = jsonDecode(request.body) as Map<String, dynamic>;
            attachRequests.add(body.cast<String, Object?>());
            grantCount++;
            return http.Response(
              jsonEncode({'token': 'grant-$grantCount'}),
              201,
            );
          }
          expect(request.url.path, '/v1/attach');
          return http.Response(
            '',
            204,
            headers: {
              'set-cookie':
                  'ai6_attach=ticket-$grantCount; Path=/v1/socket; HttpOnly',
            },
          );
        }),
      );
      board.accessToken = 'user-token';
      const session = SessionSummary(
        operationId: 'operation-a',
        missionId: 'mission-17',
        sessionId: 'provider-run-3',
        state: 'working',
        sequence: 3,
      );
      board.sessions = [session];
      final connection = SessionConnection(board: board, session: session);
      final output = Completer<List<int>>();
      final outputSubscription = connection.output.listen((bytes) {
        if (!output.isCompleted) output.complete(List<int>.from(bytes));
      });

      addTearDown(() async {
        connection.dispose();
        await outputSubscription.cancel();
        board.dispose();
        await server.close(force: true);
      });

      connection.start();
      final first = await socketWaiters[0].future.timeout(
        const Duration(seconds: 5),
      );
      await _waitUntilConnected(connection);
      expect(first.path, '/v1/socket/provider-run-3');
      expect(first.after, '0');
      expect(first.cookie, 'ai6_attach=ticket-1');
      expect(first.origin, origin.origin);
      expect(first.forwardedProto, 'https');
      expect(first.protocol, 'ai6.guatiao');
      expect(attachRequests.single['mode'], 'read');

      expect(await output.future.timeout(const Duration(seconds: 5)), [
        255,
        0,
        128,
      ]);
      expect(board.sessions.single.state, 'working');
      expect(board.sessions.single.sequence, 4);
      connection.sendInput([9]);
      connection.sendResize(80, 24);

      final replay = await socketWaiters[1].future.timeout(
        const Duration(seconds: 5),
      );
      await _waitUntilConnected(connection);
      expect(replay.after, '5');
      expect(replay.cookie, 'ai6_attach=ticket-2');
      expect(attachRequests[1]['mode'], 'read');

      await connection.setWriteLease(true);
      final writer = await socketWaiters[2].future.timeout(
        const Duration(seconds: 5),
      );
      await _waitUntilConnected(connection);
      expect(connection.writeLeaseHeld, isTrue);
      expect(writer.after, '5');
      expect(writer.cookie, 'ai6_attach=ticket-3');
      expect(attachRequests[2]['mode'], 'write');

      connection.sendInput([0, 255, 10]);
      connection.sendResize(120, 40);
      final input = await inputReceived.future.timeout(
        const Duration(seconds: 5),
      );
      final resize = await resizeReceived.future.timeout(
        const Duration(seconds: 5),
      );
      expect(input, {
        'session_id': 'provider-run-3',
        'sequence': 1,
        'direction': 'input',
        'bytes': Uint8List.fromList([0, 255, 10]),
      });
      expect(resize, {
        'session_id': 'provider-run-3',
        'sequence': 2,
        'columns': 120,
        'rows': 40,
      });

      final renewed = await socketWaiters[3].future.timeout(
        const Duration(seconds: 5),
      );
      await _waitUntilConnected(connection);
      expect(renewed.after, '5');
      expect(renewed.cookie, 'ai6_attach=ticket-4');
      expect(attachRequests[3]['mode'], 'write');

      await connection.setWriteLease(false);
      final reader = await socketWaiters[4].future.timeout(
        const Duration(seconds: 5),
      );
      await _waitUntilConnected(connection);
      expect(connection.writeLeaseHeld, isFalse);
      expect(reader.after, '5');
      expect(reader.cookie, 'ai6_attach=ticket-5');
      expect(attachRequests[4]['mode'], 'read');
    },
  );

  test('A reconnect replays from what the terminal received, not what the board saw', () async {
    final frames = await _readProtocolFrames();
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final attaches = List.generate(2, (_) => Completer<String?>());
    server.listen((request) async {
      final index = attaches.indexWhere((attach) => !attach.isCompleted);
      final socket = await WebSocketTransformer.upgrade(
        request,
        protocolSelector: (protocols) =>
            protocols.contains('ai6.guatiao') ? 'ai6.guatiao' : null,
      );
      attaches[index].complete(request.uri.queryParameters['after']);
      socket.add(frames['terminal_schema']!);
      if (index == 0) {
        socket.add(frames['terminal_value']!);
        await Future<void>.delayed(const Duration(milliseconds: 100));
        await socket.close();
      }
    });
    final board = _board(server);
    const session = SessionSummary(
      operationId: 'operation-a',
      missionId: 'mission-17',
      sessionId: 'provider-run-3',
      state: 'working',
      sequence: 40,
    );
    board.sessions = [session];
    final connection = SessionConnection(board: board, session: session);
    final received = connection.output.first;
    addTearDown(() async {
      connection.dispose();
      board.dispose();
      await server.close(force: true);
    });

    connection.start();
    expect(
      await attaches[0].future.timeout(const Duration(seconds: 60)),
      '0',
      reason:
          'the board has seen 40 events; this terminal has shown none of them',
    );
    await received.timeout(const Duration(seconds: 60));
    expect(
      await attaches[1].future.timeout(const Duration(seconds: 60)),
      '5',
      reason: 'the last frame this terminal received, whatever the board moved on to',
    );
  });

  test('A gap the gateway reports is shown once, where it happened', () async {
    final frames = await _readProtocolFrames();
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(
        request,
        protocolSelector: (protocols) =>
            protocols.contains('ai6.guatiao') ? 'ai6.guatiao' : null,
      );
      socket.add(frames['gap_schema']!);
      socket.add(frames['gap_value']!);
      socket.add(frames['terminal_schema']!);
      socket.add(frames['terminal_value']!);
      // Again, as a live frame after a position it no longer applies to.
      socket.add(frames['gap_value']!);
    });
    final board = _board(server);
    const session = SessionSummary(
      operationId: 'operation-a',
      missionId: 'mission-17',
      sessionId: 'provider-run-3',
      state: 'working',
      sequence: 0,
    );
    board.sessions = [session];
    final connection = SessionConnection(board: board, session: session);
    final shown = <List<int>>[];
    final subscription = connection.output.listen(shown.add);
    addTearDown(() async {
      connection.dispose();
      await subscription.cancel();
      board.dispose();
      await server.close(force: true);
    });

    connection.start();
    final deadline = DateTime.now().add(const Duration(seconds: 60));
    while (shown.length < 2 && DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    expect(shown, hasLength(2));
    expect(utf8.decode(shown[0]), contains('output is missing here'));
    expect(shown[1], [255, 0, 128]);
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(shown, hasLength(2), reason: 'a gap behind the position is no news');
  });
}

MissionBoard _board(HttpServer server) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('http://127.0.0.1:${server.port}'),
    pollInterval: null,
    client: MockClient((request) async {
      if (request.url.path == '/v1/user/attach-grants') {
        return http.Response(jsonEncode({'token': 'grant'}), 201);
      }
      return http.Response(
        '',
        204,
        headers: {'set-cookie': 'ai6_attach=ticket; Path=/v1/socket; HttpOnly'},
      );
    }),
  );
  board.accessToken = 'user-token';
  return board;
}

Future<void> _waitUntilConnected(SessionConnection connection) async {
  if (connection.isConnected) return;
  final connected = Completer<void>();
  void listen() {
    if (connection.isConnected && !connected.isCompleted) connected.complete();
  }

  connection.addListener(listen);
  try {
    listen();
    await connected.future.timeout(const Duration(seconds: 5));
  } finally {
    connection.removeListener(listen);
  }
}

Future<Map<String, Uint8List>> _readProtocolFrames() async {
  final file = File('../../crates/ai6-diptel/tests/vectors/frames.txt');
  final result = <String, Uint8List>{};
  for (final line in await file.readAsLines()) {
    final separator = line.indexOf('=');
    if (separator < 0) continue;
    final name = line.substring(0, separator);
    final hex = line.substring(separator + 1);
    result[name] = Uint8List.fromList([
      for (var i = 0; i < hex.length; i += 2)
        int.parse(hex.substring(i, i + 2), radix: 16),
    ]);
  }
  return result;
}

class _SocketAttach {
  const _SocketAttach({
    required this.path,
    required this.after,
    required this.cookie,
    required this.origin,
    required this.forwardedProto,
    required this.protocol,
  });

  final String path;
  final String? after;
  final String? cookie;
  final String? origin;
  final String? forwardedProto;
  final String? protocol;
}
