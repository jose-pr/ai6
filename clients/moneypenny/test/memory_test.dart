import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:moneypenny/assistant/assistant_controller.dart';
import 'package:moneypenny/assistant/assistant_panel.dart';
import 'package:moneypenny/assistant/memory_page.dart';
import 'package:moneypenny/main.dart';

const _json = {'content-type': 'application/json'};

/// The fixture's stand-in for the SHA-256 Universal Exports gives: the app only hands it back.
String digestOf(String text) => 'digest:${base64.encode(utf8.encode(text))}';

/// Universal Exports' memory routes over [files] (path under `memory/` to text), recording
/// every request; [refuse] answers a request instead when it returns a response.
class MemoryGateway {
  MemoryGateway(this.files);

  final Map<String, String> files;
  final requests = <http.Request>[];
  http.Response? Function(http.Request request)? refuse;

  static const _prefix = '/v1/user/assistant/memory';

  http.Response answer(http.Request request) {
    requests.add(request);
    if (refuse?.call(request) case final refused?) return refused;
    final path = request.url.path;
    if (path == _prefix) {
      return http.Response(
        jsonEncode({
          'person': 'operator-007',
          'commit': 'c0ffee1234567890',
          'files': [
            for (final MapEntry(:key, :value) in files.entries)
              {'path': key, 'bytes': utf8.encode(value).length},
          ],
          'more': false,
        }),
        200,
        headers: _json,
      );
    }
    final file = Uri.decodeComponent(path.substring(_prefix.length + 1));
    switch (request.method) {
      case 'GET':
        final text = files[file];
        if (text == null) {
          return http.Response(
            jsonEncode({
              'error': 'memory/$file is not in the person\'s memory.',
            }),
            404,
            headers: _json,
          );
        }
        return http.Response(
          jsonEncode({
            'path': file,
            'text': text,
            'digest': digestOf(text),
            'commit': 'c0ffee1234567890',
          }),
          200,
          headers: _json,
        );
      case 'PUT':
        final body = jsonDecode(request.body) as Map<String, dynamic>;
        files[file] = body['content'] as String;
        return http.Response(
          jsonEncode({
            'path': file,
            'digest': digestOf(files[file]!),
            'commit': 'beef',
          }),
          200,
          headers: _json,
        );
      case 'DELETE':
        files.remove(file);
        return http.Response(
          jsonEncode({'path': file, 'commit': 'f00d'}),
          200,
          headers: _json,
        );
    }
    return http.Response('', 405);
  }

  List<http.Request> sent(String method) =>
      requests.where((request) => request.method == method).toList();
}

MissionBoard signedIn(MemoryGateway gateway) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://moneypenny.example'),
    pollInterval: null,
    client: MockClient((request) async {
      if (request.url.path.startsWith('/v1/user/assistant/memory')) {
        return gateway.answer(request);
      }
      return http.Response('[]', 200, headers: _json);
    }),
  );
  board.accessToken = 'operator-token';
  board.loading = false;
  return board;
}

Future<void> openMemory(WidgetTester tester, MissionBoard board) async {
  tester.view.physicalSize = const Size(800, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(
    MaterialApp(
      home: MemoryPage(key: ObjectKey(board), board: board),
    ),
  );
  await tester.pumpAndSettle();
}

/// A chat socket that stays silent.
class SilentChannel implements AssistantChannel {
  final _incoming = StreamController<String>();

  @override
  Stream<String> get incoming => _incoming.stream;

  @override
  void send(String frame) {}

  @override
  Future<void> close() => _incoming.close();
}

void main() {
  testWidgets(
    'lists her memory, opens a file and saves it from what was opened',
    (tester) async {
      final gateway = MemoryGateway({
        'MEMORY.md': '# Memory\n\n- prefs.md\n',
        'prefs.md': 'Tables.\n',
      });
      await openMemory(tester, signedIn(gateway));

      expect(gateway.requests.single.url.path, '/v1/user/assistant/memory');
      expect(find.text('MEMORY.md'), findsOneWidget);
      expect(find.text('prefs.md'), findsOneWidget);
      expect(find.text('8 bytes'), findsOneWidget);
      expect(find.text('As of commit c0ffee123456'), findsOneWidget);

      await tester.tap(find.text('prefs.md'));
      await tester.pumpAndSettle();
      expect(
        gateway.requests.last.url.path,
        '/v1/user/assistant/memory/prefs.md',
      );
      final field = find.byKey(const Key('memory-text'));
      expect(tester.widget<TextField>(field).controller!.text, 'Tables.\n');

      await tester.enterText(field, 'Charts, not tables.\n');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      final put = gateway.sent('PUT').single;
      expect(put.url.path, '/v1/user/assistant/memory/prefs.md');
      expect(jsonDecode(put.body), {
        'content': 'Charts, not tables.\n',
        'base': digestOf('Tables.\n'),
      });
      expect(find.byKey(const Key('memory-text')), findsNothing);
      expect(find.text('Saved prefs.md.'), findsOneWidget);
      expect(find.text('20 bytes'), findsOneWidget, reason: 'listed again');
    },
  );

  testWidgets('a new file is written without a base', (tester) async {
    final gateway = MemoryGateway({'MEMORY.md': '# Memory\n'});
    await openMemory(tester, signedIn(gateway));

    await tester.tap(find.text('New file'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('memory-new-path')),
      'people/ana.md',
    );
    await tester.tap(find.text('Create'));
    await tester.pumpAndSettle();
    expect(gateway.sent('GET').length, 1, reason: 'nothing to read yet');
    expect(find.byTooltip('Forget people/ana.md'), findsNothing);

    await tester.enterText(
      find.byKey(const Key('memory-text')),
      'Ana leads ops.\n',
    );
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final put = gateway.sent('PUT').single;
    expect(put.url.path, '/v1/user/assistant/memory/people/ana.md');
    expect(jsonDecode(put.body), {'content': 'Ana leads ops.\n'});
    expect(find.text('people/ana.md'), findsOneWidget);
  });

  testWidgets('a file is forgotten only once the person confirms', (
    tester,
  ) async {
    final gateway = MemoryGateway({
      'MEMORY.md': '# Memory\n',
      'prefs.md': 'Tables.\n',
    });
    await openMemory(tester, signedIn(gateway));

    await tester.tap(find.byTooltip('Forget prefs.md'));
    await tester.pumpAndSettle();
    expect(find.text('Forget prefs.md?'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(gateway.sent('DELETE'), isEmpty);
    expect(find.text('prefs.md'), findsOneWidget);

    await tester.tap(find.byTooltip('Forget prefs.md'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Forget'));
    await tester.pumpAndSettle();
    expect(
      gateway.sent('DELETE').single.url.path,
      '/v1/user/assistant/memory/prefs.md',
    );
    expect(find.text('prefs.md'), findsNothing);
    expect(find.text('Forgotten: prefs.md.'), findsOneWidget);

    // From the open file too.
    await tester.tap(find.text('MEMORY.md'));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Forget MEMORY.md'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Forget'));
    await tester.pumpAndSettle();
    expect(gateway.sent('DELETE').length, 2);
    expect(find.text('She remembers nothing about you yet.'), findsOneWidget);
  });

  testWidgets('a refusal is shown in the gateway\'s own words', (tester) async {
    final gateway = MemoryGateway({'prefs.md': 'Tables.\n'});
    const changed =
        'Memory/prefs.md changed since it was opened, so nothing was saved: open it '
        'again and make the change from what it holds now.';
    gateway.refuse = (request) => request.method == 'PUT'
        ? http.Response(jsonEncode({'error': changed}), 409, headers: _json)
        : null;
    await openMemory(tester, signedIn(gateway));
    await tester.tap(find.text('prefs.md'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('memory-text')), 'Charts.\n');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(find.text(changed), findsOneWidget);
    expect(
      find.byKey(const Key('memory-text')),
      findsOneWidget,
      reason: 'the editor stays open with the text',
    );
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('memory-text')))
          .controller!
          .text,
      'Charts.\n',
    );
  });

  testWidgets('a memory the gateway cannot list says why', (tester) async {
    const none =
        'This deployment gives nobody a personal repository, so Moneypenny keeps no '
        'memory.';
    final empty = MemoryGateway({})
      ..refuse = (_) =>
          http.Response(jsonEncode({'error': none}), 404, headers: _json);
    await openMemory(tester, signedIn(empty));
    expect(find.text(none), findsOneWidget);
    expect(find.text('Her memory is unavailable.'), findsOneWidget);
  });

  testWidgets('the Ask Moneypenny panel\'s menu opens her memory', (
    tester,
  ) async {
    final controller = AssistantController(
      token: 'user-token',
      connect: SilentChannel.new,
    );
    addTearDown(controller.dispose);
    var opened = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AssistantPanel(
            controller: controller,
            onMemory: () => opened++,
          ),
        ),
      ),
    );
    await tester.tap(find.byTooltip('Moneypenny menu'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Her memory of you'));
    await tester.pumpAndSettle();
    expect(opened, 1);

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: AssistantPanel(controller: controller)),
      ),
    );
    expect(
      find.byTooltip('Moneypenny menu'),
      findsNothing,
      reason: 'no memory to open, no menu',
    );
  });
}
