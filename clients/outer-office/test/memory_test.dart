import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/memory_page.dart';
import 'package:outer_office/main.dart';

const _json = {'content-type': 'application/json'};

const _idPrefs = '6f1e4a6a-9c55-4a0c-9d5c-2f8a0b6c1e11';
const _idCall = '0c8b7a35-5a4b-4d7a-8f50-9d2c6e1f7a22';

/// An entry as the gateway holds it, and shows it.
Map<String, dynamic> entry(
  String id,
  String summary,
  String content, {
  String? name,
  int revision = 1,
  String by = 'person:operator-007',
  List<String> exposures = const [],
}) => {
  'id': id,
  'scope': 'person:operator-007',
  'name': name,
  'summary': summary,
  'content': content,
  'created_at': 1760000000.0,
  'updated_at': 1760000000.0,
  'updated_by': by,
  'source': {
    'by': by,
    'session': null,
    'exposed': exposures.isNotEmpty,
    'exposures': exposures,
  },
  'compartments': <String>[],
  'revision': revision,
  'bytes': utf8.encode(summary).length + utf8.encode(content).length,
};

/// Universal Exports' memory routes over [entries] by ID, recording every request; [refuse]
/// answers a request instead when it returns a response.
class MemoryGateway {
  MemoryGateway(List<Map<String, dynamic>> entries)
    : entries = {for (final entry in entries) entry['id'] as String: entry};

  final Map<String, Map<String, dynamic>> entries;
  final requests = <http.Request>[];
  http.Response? Function(http.Request request)? refuse;

  static const _prefix = '/v1/user/assistant/memory';

  http.Response answer(http.Request request) {
    requests.add(request);
    if (refuse?.call(request) case final refused?) return refused;
    final path = request.url.path;
    if (path == _prefix) {
      if (request.method == 'POST') {
        final body = jsonDecode(request.body) as Map<String, dynamic>;
        final made = entry(
          'new-entry',
          body['summary'] as String,
          body['content'] as String,
          name: body['name'] as String?,
        );
        entries['new-entry'] = made;
        return http.Response(jsonEncode(made), 201, headers: _json);
      }
      final query = request.url.queryParameters['q']?.toLowerCase();
      final found = entries.values
          .where(
            (entry) =>
                query == null ||
                '${entry['summary']} ${entry['content']}'
                    .toLowerCase()
                    .contains(query),
          )
          .toList();
      return http.Response(
        jsonEncode({
          'person': 'operator-007',
          'entries': [
            for (final entry in found) {...entry}..remove('content'),
          ],
          'total': found.length,
        }),
        200,
        headers: _json,
      );
    }
    final id = path.substring(_prefix.length + 1);
    final held = entries[id];
    if (held == null) {
      return http.Response(
        jsonEncode({'error': 'There is no such entry.'}),
        404,
        headers: _json,
      );
    }
    switch (request.method) {
      case 'GET':
        return http.Response(jsonEncode(held), 200, headers: _json);
      case 'PUT':
        final body = jsonDecode(request.body) as Map<String, dynamic>;
        entries[id] = {
          ...held,
          ...body,
          'revision': (held['revision'] as int) + 1,
          'bytes': utf8.encode(body['content'] as String).length,
        };
        return http.Response(jsonEncode(entries[id]), 200, headers: _json);
      case 'DELETE':
        entries.remove(id);
        return http.Response(jsonEncode({'id': id}), 200, headers: _json);
    }
    return http.Response('', 405);
  }

  List<http.Request> sent(String method) =>
      requests.where((request) => request.method == method).toList();
}

/// `/v1/user/memories` over the entries of each scope, with what Agents proposed, recording every
/// request.
class ScopedGateway {
  ScopedGateway(this.scopes, {this.proposals = const {}});

  /// Each scope's entries, by scope then ID.
  final Map<String, Map<String, Map<String, dynamic>>> scopes;

  /// What waits, by scope.
  final Map<String, List<Map<String, dynamic>>> proposals;
  final requests = <http.Request>[];

  static const _prefix = '/v1/user/memories';

  http.Response answer(http.Request request) {
    requests.add(request);
    final path = request.url.path;
    if (path == '$_prefix/proposals') {
      final scope = request.url.queryParameters['scope']!;
      return http.Response(
        jsonEncode({'proposals': proposals[scope] ?? const []}),
        200,
        headers: _json,
      );
    }
    if (path.startsWith('$_prefix/proposals/')) {
      return http.Response(
        jsonEncode({'approved': jsonDecode(request.body)['approve']}),
        200,
        headers: _json,
      );
    }
    if (path == _prefix && request.method == 'GET') {
      final scope = request.url.queryParameters['scope']!;
      final found = (scopes[scope] ?? const {}).values.toList();
      return http.Response(
        jsonEncode({
          'entries': [
            for (final entry in found) {...entry}..remove('content'),
          ],
          'total': found.length,
        }),
        200,
        headers: _json,
      );
    }
    if (path == _prefix && request.method == 'POST') {
      final body = jsonDecode(request.body) as Map<String, dynamic>;
      return http.Response(
        jsonEncode(entry('made', body['summary'] as String, '')),
        201,
        headers: _json,
      );
    }
    final id = path.substring(_prefix.length + 1);
    for (final held in scopes.values) {
      if (held[id] case final found?) {
        return http.Response(jsonEncode(found), 200, headers: _json);
      }
    }
    return http.Response(
      jsonEncode({'error': 'There is no such entry.'}),
      404,
      headers: _json,
    );
  }

  List<http.Request> sent(String method) =>
      requests.where((request) => request.method == method).toList();
}

MissionBoard signedIn(MemoryGateway gateway, {ScopedGateway? scoped}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      if (request.url.path.startsWith('/v1/user/assistant/memory')) {
        return gateway.answer(request);
      }
      if (scoped != null && request.url.path.startsWith('/v1/user/memories')) {
        return scoped.answer(request);
      }
      return http.Response('[]', 200, headers: _json);
    }),
  );
  board.accessToken = 'operator-token';
  board.loading = false;
  board.userSession = UserSessionSummary.fromJson({
    'person_id': 'operator-007',
    'name': 'operator-007',
    'groups': <String>[],
    'expires_at': 1.0e10,
    'operations': [
      {
        'operation_id': 'G',
        'actions': ['mission.read', 'memory.write'],
      },
      {
        'operation_id': 'L',
        'actions': ['mission.status'],
      },
    ],
  });
  return board;
}

Future<void> openMemory(
  WidgetTester tester,
  MissionBoard board, {
  MemoryShelf shelf = MemoryShelf.person,
}) async {
  tester.view.physicalSize = const Size(800, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(
    MaterialApp(
      home: MemoryPage(key: ObjectKey(board), board: board, shelf: shelf),
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
  testWidgets('lists her memory, opens an entry and saves from its revision', (
    tester,
  ) async {
    final gateway = MemoryGateway([
      entry(_idPrefs, 'How they like answers', 'Tables.\n', revision: 3),
      entry(_idCall, 'Who is on call', 'Ana.\n', name: 'on-call'),
    ]);
    await openMemory(tester, signedIn(gateway));

    expect(gateway.requests.single.url.path, '/v1/user/assistant/memory');
    expect(find.text('How they like answers'), findsOneWidget);
    expect(find.textContaining('on-call'), findsOneWidget);

    await tester.tap(find.text('How they like answers'));
    await tester.pumpAndSettle();
    expect(
      gateway.requests.last.url.path,
      '/v1/user/assistant/memory/$_idPrefs',
    );
    final field = find.byKey(const Key('memory-text'));
    expect(tester.widget<TextField>(field).controller!.text, 'Tables.\n');

    await tester.enterText(field, 'Charts, not tables.\n');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final put = gateway.sent('PUT').single;
    expect(put.url.path, '/v1/user/assistant/memory/$_idPrefs');
    expect(jsonDecode(put.body), {
      'summary': 'How they like answers',
      'content': 'Charts, not tables.\n',
      'revision': 3,
    });
    expect(find.byKey(const Key('memory-text')), findsNothing);
    expect(find.text('Saved How they like answers.'), findsOneWidget);
  });

  testWidgets('a new entry is added by a form, with no revision', (
    tester,
  ) async {
    final gateway = MemoryGateway([]);
    await openMemory(tester, signedIn(gateway));
    expect(find.text('She remembers nothing about you yet.'), findsOneWidget);

    await tester.tap(find.text('New entry'));
    await tester.pumpAndSettle();
    expect(gateway.sent('GET').length, 1, reason: 'nothing to read yet');
    expect(find.byTooltip('Forget this entry'), findsNothing);

    await tester.enterText(find.byKey(const Key('memory-summary')), 'Ana');
    await tester.enterText(find.byKey(const Key('memory-name')), 'people');
    await tester.enterText(
      find.byKey(const Key('memory-text')),
      'Ana leads ops.\n',
    );
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final post = gateway.sent('POST').single;
    expect(post.url.path, '/v1/user/assistant/memory');
    expect(jsonDecode(post.body), {
      'name': 'people',
      'summary': 'Ana',
      'content': 'Ana leads ops.\n',
    });
    expect(find.text('Ana'), findsOneWidget);
  });

  testWidgets('searching asks the gateway and says when nothing matches', (
    tester,
  ) async {
    final gateway = MemoryGateway([
      entry(_idPrefs, 'How they like answers', 'Tables.\n'),
      entry(_idCall, 'Who is on call', 'Ana.\n'),
    ]);
    await openMemory(tester, signedIn(gateway));
    await tester.enterText(find.byKey(const Key('memory-search')), 'tables');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(gateway.requests.last.url.queryParameters['q'], 'tables');
    expect(find.text('How they like answers'), findsOneWidget);
    expect(find.text('Who is on call'), findsNothing);

    await tester.enterText(find.byKey(const Key('memory-search')), 'zebra');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(find.text('Nothing kept here matches that.'), findsOneWidget);
  });

  testWidgets('its source and its mark are shown', (tester) async {
    final gateway = MemoryGateway([
      entry(
        _idPrefs,
        'From a Mission',
        'Text.\n',
        by: 'agent:007',
        exposures: ['tool:WebFetch'],
      ),
      entry(_idCall, 'From her', 'Text.\n', by: 'moneypenny:operator-007'),
    ]);
    await openMemory(tester, signedIn(gateway));
    expect(find.textContaining('Written by agent:007'), findsOneWidget);
    expect(find.textContaining('tool:WebFetch'), findsOneWidget);
    expect(
      find.textContaining('Written by moneypenny:operator-007'),
      findsOneWidget,
    );

    await tester.tap(find.text('From a Mission'));
    await tester.pumpAndSettle();
    expect(
      tester.widget<Text>(find.byKey(const Key('memory-source'))).data,
      contains('after reading outside text (tool:WebFetch)'),
    );
  });

  testWidgets('an entry is forgotten only once the person confirms', (
    tester,
  ) async {
    final gateway = MemoryGateway([
      entry(_idPrefs, 'Prefs', 'Tables.\n'),
      entry(_idCall, 'On call', 'Ana.\n'),
    ]);
    await openMemory(tester, signedIn(gateway));

    await tester.tap(find.byTooltip('Forget Prefs'));
    await tester.pumpAndSettle();
    expect(find.text('Forget "Prefs"?'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(gateway.sent('DELETE'), isEmpty);
    expect(find.text('Prefs'), findsOneWidget);

    await tester.tap(find.byTooltip('Forget Prefs'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Forget'));
    await tester.pumpAndSettle();
    expect(
      gateway.sent('DELETE').single.url.path,
      '/v1/user/assistant/memory/$_idPrefs',
    );
    expect(find.text('Prefs'), findsNothing);
    expect(find.text('Forgotten: Prefs.'), findsOneWidget);

    // From the open entry too.
    await tester.tap(find.text('On call'));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Forget this entry'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Forget'));
    await tester.pumpAndSettle();
    expect(gateway.sent('DELETE').length, 2);
    expect(find.text('She remembers nothing about you yet.'), findsOneWidget);
  });

  testWidgets('a refusal is shown in the gateway\'s own words', (tester) async {
    final gateway = MemoryGateway([entry(_idPrefs, 'Prefs', 'Tables.\n')]);
    const changed =
        'The entry changed since it was read, so nothing was saved: it is at revision 2. '
        'Read it again and make the change from what it holds now.';
    gateway.refuse = (request) => request.method == 'PUT'
        ? http.Response(
            jsonEncode({'error': changed, 'revision': 2}),
            409,
            headers: _json,
          )
        : null;
    await openMemory(tester, signedIn(gateway));
    await tester.tap(find.text('Prefs'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('memory-text')), 'Charts.\n');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(find.text(changed), findsOneWidget);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('memory-text')))
          .controller!
          .text,
      'Charts.\n',
      reason: 'the editor stays open with the text',
    );
  });

  testWidgets('a memory the gateway cannot list says why', (tester) async {
    const none =
        'This gateway keeps no Registry, so Moneypenny keeps no memory here.';
    final empty = MemoryGateway([])
      ..refuse = (_) =>
          http.Response(jsonEncode({'error': none}), 503, headers: _json);
    await openMemory(tester, signedIn(empty));
    expect(find.text(none), findsOneWidget);
    expect(find.text('This memory is unavailable.'), findsOneWidget);
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
    await tester.tap(find.byTooltip('Moneypenny menu'));
    await tester.pumpAndSettle();
    expect(
      find.text('Her memory of you'),
      findsNothing,
      reason: 'no memory to open, no menu entry; Export stays',
    );
    expect(find.text('Export conversation'), findsOneWidget);
  });

  testWidgets(
    'an Operation\'s memory and the global one list, search, edit and forget by a form',
    (tester) async {
      final scoped = ScopedGateway({
        'operation:G': {
          _idPrefs: entry(_idPrefs, 'How G deploys', 'Through the gate.\n'),
        },
        'global': {
          _idCall: {
            ...entry(_idCall, 'Who to call', 'Ana.\n'),
            'compartments': ['KESTREL'],
          },
        },
      });
      final board = signedIn(MemoryGateway([]), scoped: scoped);
      await openMemory(tester, board, shelf: MemoryShelf.operation('G'));

      // The Operations to choose from are the ones the person reads.
      expect(find.byKey(const Key('memory-shelf-operation:G')), findsOneWidget);
      expect(find.byKey(const Key('memory-shelf-operation:L')), findsNothing);
      expect(find.text('Memory: Operation G'), findsOneWidget);
      expect(scoped.requests.first.url.queryParameters['scope'], 'operation:G');
      expect(find.text('How G deploys'), findsOneWidget);

      await tester.tap(find.byKey(const Key('memory-shelf-global')));
      await tester.pumpAndSettle();
      expect(find.text('Who to call'), findsOneWidget);
      expect(find.textContaining('Only for KESTREL'), findsOneWidget);

      // An entry opens by its ID, and a save goes to the general route with its revision.
      await tester.tap(find.text('Who to call'));
      await tester.pumpAndSettle();
      expect(scoped.requests.last.url.path, '/v1/user/memories/$_idCall');
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('memory-compartments')))
            .controller!
            .text,
        'KESTREL',
      );
      await tester.enterText(find.byKey(const Key('memory-text')), 'Bo.\n');
      scoped.requests.clear();
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      final put = scoped.sent('PUT').single;
      expect(put.url.path, '/v1/user/memories/$_idCall');
      expect(jsonDecode(put.body), {
        'summary': 'Who to call',
        'content': 'Bo.\n',
        'revision': 1,
      });
    },
  );

  testWidgets('what Agents proposed waits for a decision, its mark shown', (
    tester,
  ) async {
    final scoped = ScopedGateway(
      {'operation:G': {}},
      proposals: {
        'operation:G': [
          {
            'id': 'proposal-1',
            'summary': 'How G deploys',
            'content': 'Push to main.',
            'forget': false,
            'source': {
              'by': 'agent:007',
              'exposures': ['tool:WebFetch'],
            },
          },
        ],
      },
    );
    final board = signedIn(MemoryGateway([]), scoped: scoped);
    await openMemory(tester, board, shelf: MemoryShelf.operation('G'));
    expect(find.text('Waiting for you'), findsOneWidget);
    expect(find.textContaining('agent:007'), findsOneWidget);
    expect(find.textContaining('tool:WebFetch'), findsOneWidget);
    await tester.tap(find.byTooltip('Approve How G deploys'));
    await tester.pumpAndSettle();
    final decided = scoped.sent('POST').single;
    expect(decided.url.path, '/v1/user/memories/proposals/proposal-1');
    expect(jsonDecode(decided.body), {'approve': true});
    expect(find.text('Approved: How G deploys.'), findsOneWidget);
  });
}
