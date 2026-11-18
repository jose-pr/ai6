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
import 'package:outer_office/forms/form_host.dart'
    show flutterMaterialLocalizations;

const _json = {'content-type': 'application/json'};

const _idPrefs = '6f1e4a6a-9c55-4a0c-9d5c-2f8a0b6c1e11';
const _idCall = '0c8b7a35-5a4b-4d7a-8f50-9d2c6e1f7a22';

/// A memory as the gateway holds it, and shows it: a Report of kind `memory`.
Map<String, dynamic> memory(
  String id,
  String title,
  String text, {
  String scope = 'person:operator-007',
  String summary = '',
  int revision = 1,
  Map<String, dynamic> filer = const {
    'type': 'person',
    'person': 'operator-007',
  },
  bool exposed = false,
  String state = 'standing',
  List<String> compartments = const [],
  Map<String, dynamic>? proposes,
}) => {
  'id': id,
  'scope': scope,
  'kind': 'memory',
  'state': state,
  'sight': 'content',
  'title': title,
  'summary': summary,
  'topics': <String>[],
  'text': text,
  'revision': revision,
  'created': 1760000000.0,
  'updated': 1760000000.0,
  'updated_by': 'person:operator-007',
  'filer': filer,
  'exposed': exposed,
  'compartments': compartments,
  'proposes': proposes,
};

/// Universal Exports' Report routes over the memories it holds by ID, recording every request;
/// [refuse] answers a request instead when it returns a response.
class ReportsGateway {
  ReportsGateway(List<Map<String, dynamic>> held)
    : entries = {for (final entry in held) entry['id'] as String: entry};

  final Map<String, Map<String, dynamic>> entries;
  final requests = <http.Request>[];
  http.Response? Function(http.Request request)? refuse;

  static const _prefix = '/v1/reports';

  http.Response _ok(Object body, [int status = 200]) =>
      http.Response(jsonEncode(body), status, headers: _json);

  http.Response answer(http.Request request) {
    requests.add(request);
    if (refuse?.call(request) case final refused?) return refused;
    final path = request.url.path;
    final query = request.url.queryParameters;
    if (path == _prefix && request.method == 'GET') {
      final words = query['words']?.toLowerCase();
      final found = entries.values.where((entry) {
        if (entry['scope'] != query['scope']) return false;
        if (query['state'] != null) return entry['state'] == query['state'];
        if (entry['state'] != 'standing') return false;
        return words == null ||
            '${entry['title']} ${entry['summary']} ${entry['text']}'
                .toLowerCase()
                .contains(words);
      }).toList();
      return _ok({
        'reports': [
          for (final entry in found)
            query['words'] == null ? entry : ({...entry}..remove('text')),
        ],
        'total': found.length,
      });
    }
    if (path == _prefix && request.method == 'POST') {
      final body = jsonDecode(request.body) as Map<String, dynamic>;
      final made = memory(
        'new-entry',
        body['title'] as String,
        body['text'] as String,
        scope: body['scope'] as String,
        summary: body['summary'] as String? ?? '',
      );
      entries['new-entry'] = made;
      return _ok(made, 201);
    }
    final rest = path.substring(_prefix.length + 1).split('/');
    final id = rest.first;
    final held = entries[id];
    if (held == null) {
      return _ok({'error': 'No such report.'}, 404);
    }
    if (rest.length == 2 && rest.last == 'assessment') {
      return _ok({'id': id, 'state': 'standing'});
    }
    switch (request.method) {
      case 'GET':
        return _ok(held);
      case 'PUT':
        final body = jsonDecode(request.body) as Map<String, dynamic>;
        entries[id] = {
          ...held,
          'title': body['title'],
          'summary': body['summary'] ?? '',
          'text': body['text'],
          'revision': (held['revision'] as int) + 1,
        };
        return _ok(entries[id]!);
      case 'DELETE':
        entries.remove(id);
        return _ok({'forgotten': id});
    }
    return http.Response('', 405);
  }

  List<http.Request> sent(String method) =>
      requests.where((request) => request.method == method).toList();
}

MissionBoard signedIn(ReportsGateway gateway) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      if (request.url.path.startsWith('/v1/reports')) {
        return gateway.answer(request);
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
      localizationsDelegates: const [flutterMaterialLocalizations],
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
    final gateway = ReportsGateway([
      memory(
        _idPrefs,
        'Answers',
        'Tables.\n',
        revision: 3,
        summary: 'How they like answers',
      ),
      memory(_idCall, 'on-call', 'Ana.\n', summary: 'Who is on call'),
    ]);
    await openMemory(tester, signedIn(gateway));

    final listed = gateway.requests.single.url;
    expect(listed.path, '/v1/reports');
    expect(listed.queryParameters, {
      'kind': 'memory',
      'scope': 'person:operator-007',
      'words': '',
    });
    expect(find.text('Answers'), findsOneWidget);
    expect(find.textContaining('Who is on call'), findsOneWidget);

    await tester.tap(find.text('Answers'));
    await tester.pumpAndSettle();
    expect(gateway.requests.last.url.path, '/v1/reports/$_idPrefs');
    final field = find.byKey(const Key('memory-text'));
    expect(tester.widget<TextField>(field).controller!.text, 'Tables.\n');

    await tester.enterText(field, 'Charts, not tables.\n');
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final put = gateway.sent('PUT').single;
    expect(put.url.path, '/v1/reports/$_idPrefs');
    expect(jsonDecode(put.body), {
      'title': 'Answers',
      'summary': 'How they like answers',
      'text': 'Charts, not tables.\n',
      'revision': 3,
      'topics': <String>[],
    });
    expect(find.byKey(const Key('memory-text')), findsNothing);
    expect(find.text('Saved Answers.'), findsOneWidget);
  });

  testWidgets('a new entry is filed by a form, with no revision', (
    tester,
  ) async {
    final gateway = ReportsGateway([]);
    await openMemory(tester, signedIn(gateway));
    expect(find.text('She remembers nothing about you yet.'), findsOneWidget);

    await tester.tap(find.text('New entry'));
    await tester.pumpAndSettle();
    expect(gateway.sent('GET').length, 1, reason: 'nothing to read yet');
    expect(find.byTooltip('Forget this entry'), findsNothing);

    await tester.enterText(find.byKey(const Key('memory-title')), 'people');
    await tester.enterText(find.byKey(const Key('memory-summary')), 'Ana');
    await tester.enterText(
      find.byKey(const Key('memory-text')),
      'Ana leads ops.\n',
    );
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    final post = gateway.sent('POST').single;
    expect(post.url.path, '/v1/reports');
    expect(jsonDecode(post.body), {
      'scope': 'person:operator-007',
      'kind': 'memory',
      'title': 'people',
      'summary': 'Ana',
      'text': 'Ana leads ops.\n',
    });
    expect(find.text('people'), findsOneWidget);
  });

  testWidgets('searching asks the gateway and says when nothing matches', (
    tester,
  ) async {
    final gateway = ReportsGateway([
      memory(_idPrefs, 'Answers', 'Tables.\n'),
      memory(_idCall, 'On call', 'Ana.\n'),
    ]);
    await openMemory(tester, signedIn(gateway));
    await tester.enterText(find.byKey(const Key('memory-search')), 'tables');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(gateway.requests.last.url.queryParameters['words'], 'tables');
    expect(find.text('Answers'), findsOneWidget);
    expect(find.text('On call'), findsNothing);

    await tester.enterText(find.byKey(const Key('memory-search')), 'zebra');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(find.text('Nothing kept here matches that.'), findsOneWidget);
  });

  testWidgets('its source and its mark are shown', (tester) async {
    final gateway = ReportsGateway([
      memory(
        _idPrefs,
        'From a Mission',
        'Text.\n',
        filer: {'type': 'agent', 'agent': '007', 'run': 'r', 'mission': 'G-1'},
        exposed: true,
      ),
      memory(
        _idCall,
        'From her',
        'Text.\n',
        filer: {'type': 'assistant', 'person': 'operator-007'},
      ),
    ]);
    await openMemory(tester, signedIn(gateway));
    expect(
      find.textContaining('Written after reading outside text'),
      findsOneWidget,
    );

    await tester.tap(find.text('From a Mission'));
    await tester.pumpAndSettle();
    expect(
      tester.widget<Text>(find.byKey(const Key('memory-source'))).data,
      contains('Written by agent:007, after reading outside text'),
    );
    expect(
      tester.widget<Text>(find.byKey(const Key('memory-source'))).data,
      contains('Saving your edit clears the mark.'),
    );
  });

  testWidgets('an entry is forgotten only once the person confirms', (
    tester,
  ) async {
    final gateway = ReportsGateway([
      memory(_idPrefs, 'Prefs', 'Tables.\n'),
      memory(_idCall, 'On call', 'Ana.\n'),
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
    expect(gateway.sent('DELETE').single.url.path, '/v1/reports/$_idPrefs');
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
    final gateway = ReportsGateway([memory(_idPrefs, 'Prefs', 'Tables.\n')]);
    const changed =
        'The report changed since it was read, so nothing was saved: it is at revision 2. '
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
    const none = 'This gateway keeps no Registry, so it holds no reports.';
    final empty = ReportsGateway([])
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
        localizationsDelegates: const [flutterMaterialLocalizations],
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
        localizationsDelegates: const [flutterMaterialLocalizations],
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
      final gateway = ReportsGateway([
        memory(
          _idPrefs,
          'How G deploys',
          'Through the gate.\n',
          scope: 'operation:G',
        ),
        memory(
          _idCall,
          'Who to call',
          'Ana.\n',
          scope: 'global',
          compartments: ['KESTREL'],
        ),
      ]);
      final board = signedIn(gateway);
      await openMemory(tester, board, shelf: MemoryShelf.operation('G'));

      // The Operations to choose from are the ones the person reads.
      expect(find.byKey(const Key('memory-shelf-operation:G')), findsOneWidget);
      expect(find.byKey(const Key('memory-shelf-operation:L')), findsNothing);
      expect(find.text('Memory: Operation G'), findsOneWidget);
      expect(
        gateway.requests.first.url.queryParameters['scope'],
        'operation:G',
      );
      expect(find.text('How G deploys'), findsOneWidget);

      await tester.tap(find.byKey(const Key('memory-shelf-global')));
      await tester.pumpAndSettle();
      expect(find.text('Who to call'), findsOneWidget);

      // An entry opens by its ID, and a save goes to the same route with its revision.
      await tester.tap(find.text('Who to call'));
      await tester.pumpAndSettle();
      expect(gateway.requests.last.url.path, '/v1/reports/$_idCall');
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('memory-compartments')))
            .controller!
            .text,
        'KESTREL',
      );
      await tester.enterText(find.byKey(const Key('memory-text')), 'Bo.\n');
      gateway.requests.clear();
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      final put = gateway.sent('PUT').single;
      expect(put.url.path, '/v1/reports/$_idCall');
      expect(jsonDecode(put.body), {
        'title': 'Who to call',
        'text': 'Bo.\n',
        'revision': 1,
        'topics': <String>[],
      });
    },
  );

  testWidgets('what Agents proposed waits for a decision, its mark shown', (
    tester,
  ) async {
    final gateway = ReportsGateway([
      memory(
        'proposal-1',
        'How G deploys',
        'Push to main.',
        scope: 'operation:G',
        state: 'open',
        filer: {'type': 'agent', 'agent': '007', 'run': 'r', 'mission': 'G-1'},
        exposed: true,
      ),
    ]);
    final board = signedIn(gateway);
    await openMemory(tester, board, shelf: MemoryShelf.operation('G'));
    expect(find.text('Waiting for you'), findsOneWidget);
    expect(find.textContaining('agent:007'), findsOneWidget);
    expect(find.textContaining('after reading outside text'), findsOneWidget);
    expect(
      gateway.requests.any(
        (request) => request.url.queryParameters['state'] == 'open',
      ),
      isTrue,
    );
    await tester.tap(find.byTooltip('Approve How G deploys'));
    await tester.pumpAndSettle();
    final decided = gateway.sent('PUT').single;
    expect(decided.url.path, '/v1/reports/proposal-1/assessment');
    expect(jsonDecode(decided.body)['assessment'], 'accepted');
    expect(find.text('Approved: How G deploys.'), findsOneWidget);
  });
}
