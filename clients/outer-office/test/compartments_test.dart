import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/forms/form_host.dart'
    show flutterMaterialLocalizations;
import 'package:outer_office/compartments/compartments_page.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/oo_shell.dart';

import 'shell_navigation.dart';

final _forms = jsonDecode(
  File('assets/record_forms.json').readAsStringSync(),
) as Map<String, dynamic>;

Map<String, Object?> _weakened({
  List<Map<String, Object?>> covers = const [
    {'entry': 'operation:G', 'propagate': true},
  ],
}) => {
  'id': 'KESTREL',
  'description': 'Release work',
  'actions': ['mission.status', 'product.read'],
  'limits': {'max_fanout': 'none'},
  'weakenings': [
    'compartment KESTREL: limits.max_fanout none is looser than the default 100',
  ],
  'covers': covers,
  'read_in': [
    {'entry': 'group:release', 'propagate': true},
  ],
  'revision': 'r1',
  'writable': true,
};

http.Response _json(Object? body, [int status = 200]) => http.Response(
  jsonEncode(body),
  status,
  headers: {'content-type': 'application/json'},
);

class _Gateway {
  _Gateway(this.views);

  List<Map<String, Object?>> views;
  final requests = <http.Request>[];
  http.Response? refusal;

  Future<http.Response> answer(http.Request request) async {
    final path = request.url.path;
    if (path == '/v1/user/compartment-form') {
      return _json(_forms['compartments']);
    }
    if (path.startsWith('/v1/user/compartments')) {
      requests.add(request);
      if (request.method != 'GET') {
        return refusal ?? _json({'id': 'KESTREL', 'weakenings': <String>[]});
      }
      return _json(views);
    }
    return http.Response('[]', 200);
  }
}

MissionBoard _board(
  _Gateway gateway, {
  List<(String, List<String>)> operations = const [],
  List<String> global = const ['compartment.manage'],
}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient(gateway.answer),
  );
  board.accessToken = 'token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'p-1',
    name: 'p-1',
    displayName: null,
    groups: const [],
    expiresAt: 1e12,
    operations: [
      for (final (id, actions) in operations)
        UserOperationAccess(operationId: id, actions: actions),
    ],
    globalActions: global,
  );
  return board;
}

Future<void> _open(WidgetTester tester, MissionBoard board) async {
  tester.view.physicalSize = const Size(1400, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(
    MaterialApp(
      localizationsDelegates: const [flutterMaterialLocalizations],
      home: Scaffold(
        body: CompartmentsPage(
          board: board,
          route: const OoRoute(OoPage.compartments),
          onRoute: (_) {},
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'the card shows what it grants, its entries and a weakening chip',
    (tester) async {
      await _open(tester, _board(_Gateway([_weakened()])));
      expect(find.text('KESTREL'), findsOneWidget);
      expect(find.text('product.read'), findsOneWidget);
      expect(find.text('operation:G'), findsOneWidget);
      expect(find.text('group:release'), findsOneWidget);
      expect(
        find.byKey(const Key('compartment-weakening-KESTREL')),
        findsOneWidget,
      );
      expect(
        find.byTooltip(
          'compartment KESTREL: limits.max_fanout none is looser than the default 100',
        ),
        findsOneWidget,
      );
    },
  );

  testWidgets('a plain record shows no weakening chip', (tester) async {
    await _open(
      tester,
      _board(
        _Gateway([
          {..._weakened(), 'weakenings': <String>[]},
        ]),
      ),
    );
    expect(
      find.byKey(const Key('compartment-weakening-KESTREL')),
      findsNothing,
    );
  });

  testWidgets('the cover picker offers only the Operations that consent', (
    tester,
  ) async {
    final gateway = _Gateway([_weakened(covers: const [])]);
    await _open(
      tester,
      _board(
        gateway,
        operations: [
          ('G', ['mission.read', 'compartment.cover']),
          ('H', ['mission.read', 'compartment.cover']),
          ('L', ['mission.read']),
        ],
      ),
    );
    await tester.tap(find.text('Cover…'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('compartment-cover-G')), findsOneWidget);
    expect(find.byKey(const Key('compartment-cover-H')), findsOneWidget);
    expect(find.byKey(const Key('compartment-cover-L')), findsNothing);
    await tester.tap(find.byKey(const Key('compartment-cover-H')));
    await tester.pumpAndSettle();
    final put = gateway.requests.singleWhere(
      (request) => request.method == 'PUT',
    );
    expect(
      Uri.decodeComponent(put.url.path),
      '/v1/user/compartments/KESTREL/covers/operation:H',
    );
    expect(jsonDecode(put.body), {'propagate': true});
  });

  testWidgets(
    'a person is read in through the dialog, and the gateway can refuse',
    (tester) async {
      final gateway = _Gateway([_weakened()])
        ..refusal = _json({'error': 'request is not authorized'}, 403);
      await _open(tester, _board(gateway));
      await tester.tap(find.text('Read in…'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('compartment-read-in-name')),
        'ai6',
      );
      await tester.tap(find.byKey(const Key('compartment-read-in-save')));
      await tester.pumpAndSettle();
      final put = gateway.requests.singleWhere(
        (request) => request.method == 'PUT',
      );
      expect(
        Uri.decodeComponent(put.url.path),
        '/v1/user/compartments/KESTREL/read-in/operation:ai6',
      );
      expect(find.text('request is not authorized'), findsOneWidget);
    },
  );

  testWidgets('a record is made through its form, not a text box', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1400, 2400);
    final gateway = _Gateway(const []);
    await _open(tester, _board(gateway));
    tester.view.physicalSize = const Size(1400, 2400);
    await tester.tap(find.byKey(const Key('compartment-new')));
    await tester.pumpAndSettle();
    expect(find.text('What it grants'), findsWidgets);
    await tester.enterText(find.byKey(const Key('compartment-key')), 'LAPWING');
    await tester.tap(find.text('Read briefs'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('compartment-save')));
    await tester.pumpAndSettle();
    final put = gateway.requests.singleWhere(
      (request) => request.method == 'PUT',
    );
    expect(put.url.path, '/v1/user/compartments/LAPWING');
    final body = jsonDecode(put.body) as Map<String, dynamic>;
    expect(body['actions'], contains('mission.read'));
    expect(body.containsKey('covers'), isFalse);
    expect(body.containsKey('read_in'), isFalse);
  });

  testWidgets('removing an entry names it and keeps the rest', (tester) async {
    final gateway = _Gateway([_weakened()]);
    await _open(tester, _board(gateway));
    await tester.tap(find.byTooltip('Remove operation:G'));
    await tester.pumpAndSettle();
    final delete = gateway.requests.singleWhere(
      (request) => request.method == 'DELETE',
    );
    expect(
      Uri.decodeComponent(delete.url.path),
      '/v1/user/compartments/KESTREL/covers/operation:G',
    );
  });

  testWidgets(
    'an Operation shows the compartments that open it to the person',
    (tester) async {
      final board = _board(
        _Gateway(const []),
        global: const [],
        operations: const [],
      );
      board.userSession = UserSessionSummary(
        personId: 'p-1',
        name: 'p-1',
        displayName: null,
        groups: const [],
        expiresAt: 1e12,
        operations: const [
          UserOperationAccess(
            operationId: 'L',
            actions: ['mission.read'],
            compartments: ['LAPWING'],
          ),
        ],
      );
      expect(board.canOpenCompartments, isTrue);
      tester.view.physicalSize = const Size(1400, 1200);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(board.dispose);
      await tester.pumpWidget(
        OuterOfficeApp(board: board, desktopWindows: false),
      );
      await openPage(tester, 'Operations');
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('compartment-chip-LAPWING')), findsOneWidget);
    },
  );

  test('the route round-trips through its fixture', () {
    final route = OoRoute.fromJson({
      'page': 'compartments',
      'compartment_id': 'KESTREL',
    });
    expect(route['compartment_id'], 'KESTREL');
    expect(
      route.toUri().toString(),
      'app:/compartments?compartment_id=KESTREL',
    );
    expect(
      () => OoRoute.fromJson({'page': 'compartments', 'tab': 'roles'}),
      throwsFormatException,
    );
  });

  testWidgets('the rail offers Compartments, and only with a reason to', (
    tester,
  ) async {
    final open = _board(_Gateway(const []));
    final none = _board(_Gateway(const []), global: const []);
    expect(open.canOpenCompartments, isTrue);
    expect(none.canOpenCompartments, isFalse);
    final readIn = _board(
      _Gateway(const []),
      global: const [],
      operations: [
        ('L', ['mission.read']),
      ],
    );
    expect(readIn.canOpenCompartments, isFalse);
    open.dispose();
    none.dispose();
    readIn.dispose();
    final board = _board(_Gateway(const []));
    tester.view.physicalSize = const Size(1400, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await openPage(tester, 'Compartments');
    await tester.pumpAndSettle();
    expect(find.text('New compartment'), findsOneWidget);
  });
}
