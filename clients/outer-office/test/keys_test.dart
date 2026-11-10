import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/keys/keys_section.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/top_bar.dart' show UserChip;
import 'package:outer_office/theme/oo_theme.dart';

const _fakeSecret = 'oo_key_FAKE_not_a_real_secret';

Map<String, Object?> _key(
  String id,
  String name, {
  List<String> scopes = const ['config.manage@config:plugin'],
  double? expiresAt,
  double? lastUsedAt,
}) => {
  'id': id,
  'name': name,
  'scopes': scopes,
  'created_at': 1700000000.0,
  'expires_at': expiresAt,
  'last_used_at': lastUsedAt,
  'owner_id': 'p-1',
  'owner_name': 'ada',
};

class _Gateway {
  _Gateway(this.keys);

  final List<Map<String, Object?>> keys;
  final requests = <http.Request>[];

  /// Set to make the next create refuse in the gateway's words.
  String? refuseCreate;

  Future<http.Response> handle(http.Request request) async {
    requests.add(request);
    final path = request.url.path;
    http.Response reply(int status, Object? body) => http.Response(
      jsonEncode(body),
      status,
      headers: {'content-type': 'application/json'},
    );
    if (!path.startsWith('/v1/user/keys')) return reply(200, []);
    switch (request.method) {
      case 'GET':
        return reply(200, keys);
      case 'POST':
        final refusal = refuseCreate;
        if (refusal != null) return reply(400, {'error': refusal});
        final body = jsonDecode(request.body) as Map<String, dynamic>;
        final made = _key(
          'k-new',
          body['name'] as String,
          scopes: (body['scopes'] as List<dynamic>).cast<String>(),
        );
        keys.add(made);
        return reply(201, {'key': made, 'secret': _fakeSecret});
      case 'DELETE':
        keys.removeWhere((key) => key['id'] == path.split('/').last);
        return reply(200, {'revoked': true});
    }
    return reply(405, {'error': 'no'});
  }
}

Future<(MissionBoard, _Gateway)> _open(
  WidgetTester tester,
  List<Map<String, Object?>> keys, {
  List<String> global = const [],
}) async {
  final gateway = _Gateway(keys);
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient(gateway.handle),
  );
  board.accessToken = 'token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'p-1',
    name: 'ada',
    displayName: null,
    groups: const [],
    expiresAt: 1e12,
    operations: const [],
    globalActions: global,
  );
  tester.view.physicalSize = const Size(800, 1600);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(
    OoMaterialApp(
      themeId: 'light',
      backdrop: false,
      title: 'test',
      home: Scaffold(
        body: SingleChildScrollView(child: KeysSection(board: board)),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return (board, gateway);
}

Future<void> _createKey(WidgetTester tester) async {
  await tester.tap(find.text('Create key'));
  await tester.pumpAndSettle();
  await tester.enterText(find.byType(TextField).at(0), 'deploy');
  await tester.enterText(
    find.byType(TextField).at(1),
    'config.manage@config:plugin',
  );
  await tester.tap(find.widgetWithText(FilledButton, 'Create'));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('lists each key with its scopes and times, never a value', (
    tester,
  ) async {
    await _open(tester, [
      _key('k-1', 'ci', scopes: ['config.manage@config:plugin', 'a.b@c:d']),
      _key('k-2', 'old', expiresAt: 1800000000.0, lastUsedAt: 1700000500.0),
    ]);
    expect(find.text('ci'), findsOneWidget);
    expect(find.text('config.manage@config:plugin'), findsNWidgets(2));
    expect(find.text('a.b@c:d'), findsOneWidget);
    expect(find.textContaining('Created '), findsNWidgets(2));
    expect(find.text('Expires: never'), findsOneWidget);
    expect(find.textContaining('Expires '), findsOneWidget);
    expect(find.text('Last used: never used'), findsOneWidget);
    expect(find.textContaining('Last used '), findsNWidgets(2));
    expect(find.textContaining(_fakeSecret), findsNothing);
    expect(find.byTooltip('Revoke ci'), findsOneWidget);
    expect(find.text('All keys'), findsNothing);
  });

  testWidgets('no keys says so', (tester) async {
    await _open(tester, []);
    expect(find.text('No keys yet.'), findsOneWidget);
    expect(find.text('Create key'), findsOneWidget);
  });

  testWidgets('a failed read shows the gateway reason', (tester) async {
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      pollInterval: null,
      client: MockClient(
        (_) async => http.Response(jsonEncode({'error': 'Keys are off.'}), 503),
      ),
    )..accessToken = 'token';
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OoMaterialApp(
        themeId: 'light',
        backdrop: false,
        title: 'test',
        home: Scaffold(body: KeysSection(board: board)),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Keys are off.'), findsOneWidget);
  });

  testWidgets('a new key shows its secret once, in a dialog only', (
    tester,
  ) async {
    final copied = <String>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied.add((call.arguments as Map)['text'] as String);
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    final (_, gateway) = await _open(tester, []);
    await _createKey(tester);

    final post = gateway.requests.singleWhere((r) => r.method == 'POST');
    expect(post.headers['authorization'], 'Bearer token');
    expect(jsonDecode(post.body), {
      'name': 'deploy',
      'scopes': ['config.manage@config:plugin'],
    });
    expect(find.text('Your new key'), findsOneWidget);
    expect(find.textContaining('Shown once, not stored'), findsOneWidget);
    expect(find.text(_fakeSecret), findsOneWidget);

    await tester.tap(find.text('Copy'));
    await tester.pump();
    expect(copied, [_fakeSecret]);

    await tester.tap(find.text('Done'));
    await tester.pumpAndSettle();
    expect(find.text('Your new key'), findsNothing);
    expect(find.textContaining(_fakeSecret), findsNothing);
    expect(find.text('deploy'), findsOneWidget);
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is EditableText && widget.controller.text.contains('oo_key'),
      ),
      findsNothing,
    );
  });

  testWidgets('a lifetime is sent in seconds, and a bad one is refused here', (
    tester,
  ) async {
    final (_, gateway) = await _open(tester, []);
    await tester.tap(find.text('Create key'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).at(0), 'x');
    await tester.enterText(find.byType(TextField).at(1), 'a.b@c:d');
    await tester.enterText(find.byType(TextField).at(2), 'soon');
    await tester.tap(find.widgetWithText(FilledButton, 'Create'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Use 30d'), findsOneWidget);
    expect(gateway.requests.where((r) => r.method == 'POST'), isEmpty);

    await tester.enterText(find.byType(TextField).at(2), '2d');
    await tester.tap(find.widgetWithText(FilledButton, 'Create'));
    await tester.pumpAndSettle();
    final post = gateway.requests.singleWhere((r) => r.method == 'POST');
    expect((jsonDecode(post.body) as Map)['expires_in'], 172800);
    expect(parseKeyLifetime('never'), 0);
    expect(parseKeyLifetime('  '), isNull);
  });

  testWidgets("the gateway's refusal is shown in its words", (tester) async {
    final (_, gateway) = await _open(tester, []);
    gateway.refuseCreate = 'You do not hold config.manage at config:plugin.';
    await _createKey(tester);
    expect(
      find.text('You do not hold config.manage at config:plugin.'),
      findsOneWidget,
    );
    expect(find.text('Your new key'), findsNothing);
    expect(gateway.keys, isEmpty);
  });

  testWidgets('revoking asks first, then deletes and drops the row', (
    tester,
  ) async {
    final (_, gateway) = await _open(tester, [
      _key('k-1', 'ci'),
      _key('k-2', 'old'),
    ]);
    await tester.tap(find.byTooltip('Revoke ci'));
    await tester.pumpAndSettle();
    expect(find.text('Revoke ci?'), findsOneWidget);
    expect(gateway.requests.where((r) => r.method == 'DELETE'), isEmpty);

    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(find.text('ci'), findsOneWidget);
    expect(gateway.requests.where((r) => r.method == 'DELETE'), isEmpty);

    await tester.tap(find.byTooltip('Revoke ci'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Revoke'));
    await tester.pumpAndSettle();
    final delete = gateway.requests.singleWhere((r) => r.method == 'DELETE');
    expect(delete.url.path, '/v1/user/keys/k-1');
    expect(find.text('ci'), findsNothing);
    expect(find.text('old'), findsOneWidget);
  });

  testWidgets('an administrator may list every key', (tester) async {
    final (_, gateway) = await _open(
      tester,
      [_key('k-1', 'ci')],
      global: ['account.manage'],
    );
    await tester.tap(find.text('All keys'));
    await tester.pumpAndSettle();
    expect(
      gateway.requests.any(
        (r) => r.method == 'GET' && r.url.query == 'all=true',
      ),
      isTrue,
    );
    expect(find.text('Owner: ada'), findsOneWidget);
  });

  testWidgets("the operator account dialog holds the person's Keys", (
    tester,
  ) async {
    final (board, _) = await _open(tester, [_key('k-1', 'ci')]);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byType(UserChip));
    await tester.pumpAndSettle();
    expect(find.text('Keys'), findsOneWidget);
    expect(find.text('ci'), findsOneWidget);
  });
}
