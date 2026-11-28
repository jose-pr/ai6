import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/roster/secret_values.dart';
import 'package:outer_office/theme/oo_theme.dart';

const _planted = 'planted-value-0451';

class _Gateway {
  final requests = <http.Request>[];

  /// The states the request route answers, in order; the last one repeats.
  List<Map<String, Object?>> answers = [
    {
      'state': 'done',
      'outcome': {
        'verb': 'put',
        'name': 'forge',
        'scope': 'operation:alpha',
        'revision': 3,
        'key_id': 'key-1',
        'stations': ['a', 'b'],
        'notes': <String>[],
      },
    },
  ];
  int asked = 0;
  int? refuseSendWith;

  Future<http.Response> handle(http.Request request) async {
    requests.add(request);
    http.Response reply(int status, Object? body) => http.Response(
      jsonEncode(body),
      status,
      headers: {'content-type': 'application/json'},
    );
    final path = request.url.path;
    if (path == '/v1/user/secrets') {
      return reply(200, {
        'values': [
          {
            'name': 'forge',
            'scope': 'operation:alpha',
            'revision': 2,
            'key_id': 'key-1',
          },
        ],
        'scopes': [
          {
            'scope': 'operation:alpha',
            'revision': 1,
            'key_id': 'key-1',
            'stations': ['a', 'b'],
            'rotating': false,
          },
        ],
      });
    }
    if (path == '/v1/user/secrets/requests' && request.method == 'POST') {
      final refusal = refuseSendWith;
      if (refusal != null) {
        return reply(refusal, {'error': 'scope global has no key yet'});
      }
      return reply(202, {'id': 'req1'});
    }
    if (path == '/v1/user/secrets/requests/req1') {
      final answer =
          answers[asked < answers.length ? asked : answers.length - 1];
      asked++;
      return reply(200, answer);
    }
    return reply(404, {'error': 'no'});
  }

  List<Map<String, dynamic>> get sent => [
    for (final request in requests)
      if (request.method == 'POST')
        jsonDecode(request.body) as Map<String, dynamic>,
  ];
}

Future<_Gateway> _open(
  WidgetTester tester, {
  List<String> actions = const ['config.read', 'config.manage'],
  List<UserOperationAccess> operations = const [],
}) async {
  final gateway = _Gateway();
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
    operations: operations,
    config: {'secrets': actions},
  );
  tester.view.physicalSize = const Size(800, 2000);
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
        body: SingleChildScrollView(
          child: SecretValuesPanel(
            board: board,
            pollInterval: const Duration(milliseconds: 10),
            initiallyExpanded: true,
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return gateway;
}

Future<void> _selectVerb(WidgetTester tester, String label) async {
  await tester.tap(find.byKey(const ValueKey('secret-verb')));
  await tester.pumpAndSettle();
  await tester.tap(find.text(label).last);
  await tester.pumpAndSettle();
}

Future<void> _putForge(WidgetTester tester) async {
  await tester.enterText(find.byKey(const ValueKey('secret-name')), 'forge');
  await tester.enterText(find.byKey(const ValueKey('secret-value')), _planted);
  await tester.pump();
  await tester.tap(find.byKey(const ValueKey('secret-send')));
  await tester.pumpAndSettle();
}

String _everyRenderedText(WidgetTester tester) => [
  for (final text in tester.widgetList<Text>(find.byType(Text)))
    text.data ?? text.textSpan?.toPlainText() ?? '',
  for (final field in tester.widgetList<EditableText>(
    find.byType(EditableText),
  ))
    field.controller.text,
].join('\n');

void main() {
  test(
    'the default scope follows the value, then the Operation, then global',
    () {
      const values = [
        SecretValueRow(
          name: 'forge',
          scope: 'compartment:red',
          revision: 1,
          keyId: 'k',
        ),
      ];
      expect(defaultSecretScope(), 'global');
      expect(defaultSecretScope(operationId: 'alpha'), 'operation:alpha');
      expect(
        defaultSecretScope(name: 'forge', operationId: 'alpha', values: values),
        'compartment:red',
      );
      expect(
        defaultSecretScope(name: 'other', operationId: 'alpha', values: values),
        'operation:alpha',
      );
    },
  );

  test('an outcome is read as words with no value in them', () {
    final answer = SecretAnswer.fromJson({
      'state': 'done',
      'outcome': {
        'verb': 'revoke',
        'scope': 'global',
        'stations': ['a'],
        'notes': ['Revoke its Registry credentials too.'],
      },
    });
    expect(answer.finished, isTrue);
    expect(answer.lines.first, 'Done: revoke in global.');
    expect(answer.lines.last, 'Revoke its Registry credentials too.');
    expect(SecretAnswer.fromJson({'state': 'pending'}).finished, isFalse);
  });

  testWidgets(
    'the value box is masked, cannot be copied, and is listed no value',
    (tester) async {
      await _open(tester);
      expect(find.text('forge'), findsOneWidget);
      expect(find.text('operation:alpha, revision 2'), findsOneWidget);
      final field = tester.widget<TextField>(
        find.byKey(const ValueKey('secret-value')),
      );
      expect(field.obscureText, isTrue);
      expect(field.enableInteractiveSelection, isFalse);
      expect(field.enableSuggestions, isFalse);
      expect(field.autocorrect, isFalse);
    },
  );

  testWidgets(
    'a put sends the right body, empties the box and shows the outcome',
    (tester) async {
      final gateway = await _open(
        tester,
        operations: const [
          UserOperationAccess(operationId: 'alpha', actions: ['config.manage']),
        ],
      );
      // An Operation chosen gives its scope to a new secret.
      await tester.tap(find.byKey(const ValueKey('secret-operation')));
      await tester.pumpAndSettle();
      await tester.tap(find.textContaining('alpha').last);
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('secret-name')),
        'fresh',
      );
      await tester.pump();
      expect(find.widgetWithText(TextField, 'operation:alpha'), findsOneWidget);
      await tester.enterText(
        find.byKey(const ValueKey('secret-stations')),
        'a, b',
      );
      await tester.enterText(
        find.byKey(const ValueKey('secret-value')),
        _planted,
      );
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('secret-send')));
      await tester.pumpAndSettle();

      expect(gateway.sent.single, {
        'verb': 'put',
        'name': 'fresh',
        'scope': 'operation:alpha',
        'value': _planted,
        'stations': ['a', 'b'],
      });
      expect(
        gateway.requests.map((r) => '${r.method} ${r.url.path}'),
        contains('GET /v1/user/secrets/requests/req1'),
      );
      for (final request in gateway.requests) {
        expect(request.headers['authorization'], 'Bearer token');
      }
      final box = tester.widget<TextField>(
        find.byKey(const ValueKey('secret-value')),
      );
      expect(box.controller!.text, isEmpty);
      expect(
        find.text('Done: put forge in operation:alpha, revision 3.'),
        findsOneWidget,
      );
      expect(find.text('Stations that hold the key: a, b.'), findsOneWidget);
      expect(_everyRenderedText(tester), isNot(contains(_planted)));
    },
  );

  testWidgets('a name that exists keeps its own scope', (tester) async {
    final gateway = await _open(tester);
    await tester.enterText(find.byKey(const ValueKey('secret-name')), 'forge');
    await tester.pump();
    await tester.enterText(find.byKey(const ValueKey('secret-value')), 'x');
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('secret-send')));
    await tester.pumpAndSettle();
    expect(gateway.sent.single['scope'], 'operation:alpha');
  });

  testWidgets('a pending request is read again until it is done', (
    tester,
  ) async {
    final gateway = await _open(tester);
    gateway.answers = [
      {'state': 'pending'},
      {'state': 'pending'},
      {
        'state': 'done',
        'outcome': {'verb': 'rotate', 'name': 'forge', 'revision': 4},
      },
    ];
    await _selectVerb(tester, 'Rotate a value');
    await _putForge(tester);
    expect(gateway.sent.single, {
      'verb': 'rotate',
      'name': 'forge',
      'value': _planted,
    });
    expect(gateway.asked, 3);
    expect(find.text('Done: rotate forge, revision 4.'), findsOneWidget);
  });

  testWidgets('grant, revoke and rotate-key send no value', (tester) async {
    final gateway = await _open(tester);
    for (final (label, verb) in [
      ('Grant a Station', 'grant'),
      ('Revoke a Station', 'revoke'),
    ]) {
      await _selectVerb(tester, label);
      expect(find.byKey(const ValueKey('secret-value')), findsNothing);
      await tester.enterText(
        find.byKey(const ValueKey('secret-scope')),
        'operation:alpha',
      );
      await tester.enterText(find.byKey(const ValueKey('secret-station')), 'b');
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('secret-send')));
      await tester.pumpAndSettle();
      expect(gateway.sent.last, {
        'verb': verb,
        'scope': 'operation:alpha',
        'station': 'b',
      });
    }
    await _selectVerb(tester, 'Rotate a scope key');
    await tester.enterText(
      find.byKey(const ValueKey('secret-scope')),
      'operation:alpha',
    );
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('secret-send')));
    await tester.pumpAndSettle();
    expect(gateway.sent.last, {
      'verb': 'rotate-key',
      'scope': 'operation:alpha',
    });
  });

  testWidgets(
    'a refusal and a failed request show their reasons, never the value',
    (tester) async {
      final gateway = await _open(tester);
      gateway.refuseSendWith = 422;
      await _putForge(tester);
      expect(find.text('scope global has no key yet'), findsOneWidget);
      expect(_everyRenderedText(tester), isNot(contains(_planted)));

      gateway.refuseSendWith = null;
      gateway.answers = [
        {'state': 'failed', 'error': 'no Station holds the key'},
      ];
      await _putForge(tester);
      expect(find.text('Failed: no Station holds the key'), findsOneWidget);
      expect(_everyRenderedText(tester), isNot(contains(_planted)));
    },
  );

  testWidgets('a reader sees the values and no form', (tester) async {
    await _open(tester, actions: const ['config.read']);
    expect(find.text('forge'), findsOneWidget);
    expect(find.byKey(const ValueKey('secret-send')), findsNothing);
    expect(find.byKey(const ValueKey('secret-value')), findsNothing);
  });
}
