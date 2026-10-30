import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/access/access_api.dart';
import 'package:outer_office/access/role_editor.dart';
import 'package:outer_office/main.dart';

import 'shell_navigation.dart';

const _chiefActions = [
  'access.manage',
  'access.read',
  'assistant.memory.admin',
  'mission.read',
  'pool.manage',
];

Map<String, Object?> _view({
  String revision = 'r1',
  List<String> delegable = const ['mission.read', 'pool.read'],
}) => {
  'revision': revision,
  'writable': true,
  'roles': [
    {
      'name': 'chief',
      'built_in': true,
      'editable': false,
      'actions': _chiefActions,
    },
    {
      'name': 'operator',
      'built_in': true,
      'editable': true,
      'actions': ['mission.read'],
    },
  ],
  'rules': [
    {'subject': 'group:ops', 'scope': 'operation:G', 'role': 'operator'},
  ],
  'delegable': {
    'actions': delegable,
    'default': true,
    'weakenings': <String>[],
  },
  'chief': {
    'stored': ['account:boss'],
    'host': ['account:root'],
  },
};

/// A board signed in with `global` actions on the policy and the accounts, and `operations`
/// as (id, actions); the gateway answers with `respond`.
MissionBoard board(
  List<http.Request> requests,
  List<String> global,
  http.Response Function(http.Request) respond, {
  List<(String, List<String>)> operations = const [],
  List<String> roster = const [],
}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      requests.add(request);
      final path = request.url.path;
      if (path.contains('/access') || path.startsWith('/v1/user/accounts')) {
        return respond(request);
      }
      return http.Response('[]', 200);
    }),
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
    roster: roster,
    globalActions: global,
  );
  return board;
}

Future<void> openAccess(WidgetTester tester, MissionBoard board) async {
  tester.view.physicalSize = const Size(1400, 1200);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await openPage(tester, 'Access');
  await tester.pumpAndSettle();
}

http.Response json(Object? body, [int status = 200]) =>
    http.Response(jsonEncode(body), status);

void main() {
  test('the rail offers Access to whoever may read or manage something', () {
    final controller = board([], [
      'access.read',
      'access.manage',
      'account.manage',
    ], (_) => json({}));
    final auditor = board([], ['access.read'], (_) => json({}));
    final delegate = board(
      [],
      [],
      (_) => json({}),
      operations: [
        ('X', ['mission.read', 'access.read']),
      ],
    );
    final nobody = board(
      [],
      [],
      (_) => json({}),
      operations: [
        ('X', ['mission.read']),
      ],
      roster: ['roster.read'],
    );
    expect(controller.canOpenAccess, isTrue);
    expect(auditor.canOpenAccess, isTrue);
    expect(delegate.canOpenAccess, isTrue);
    expect(nobody.canOpenAccess, isFalse);
    for (final each in [controller, auditor, delegate, nobody]) {
      each.dispose();
    }
  });

  testWidgets(
    'a controller sees Access and may edit; an auditor sees it read-only',
    (tester) async {
      final controller = board([], [
        'access.read',
        'access.manage',
      ], (_) => json(_view()));
      await openAccess(tester, controller);
      expect(find.text('Roles'), findsOneWidget);
      expect(find.text('built in'), findsWidgets);
      expect(find.text('account:root (from unex.cfg, fixed)'), findsOneWidget);
      expect(find.text('New role'), findsOneWidget);
      expect(find.byTooltip('Edit operator'), findsOneWidget);
      expect(
        find.byTooltip('Edit chief'),
        findsNothing,
        reason: 'chief is fixed',
      );
    },
  );

  testWidgets('an auditor reads and edits nothing', (tester) async {
    final auditor = board([], ['access.read'], (_) => json(_view()));
    await openAccess(tester, auditor);
    expect(find.text('Read only'), findsOneWidget);
    expect(find.text('New role'), findsNothing);
    expect(find.byTooltip('Edit operator'), findsNothing);
    expect(find.byTooltip('Delete operator'), findsNothing);
  });

  testWidgets(
    'the role editor greys what the person does not hold, with the reason',
    (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: RoleEditor(
              universe: _chiefActions,
              held: const {'access.manage', 'mission.read', 'pool.manage'},
              operationScope: false,
              cap: const {},
            ),
          ),
        ),
      );
      SwitchListTile tile(String action) =>
          tester.widget(find.widgetWithText(SwitchListTile, action));
      expect(tile('assistant.memory.admin').onChanged, isNull);
      expect(tile('pool.manage').onChanged, isNotNull);
      expect(
        find.byTooltip('You do not hold this action, so you cannot give it.'),
        findsWidgets,
      );
    },
  );

  testWidgets("a delegate's Operation tab offers only actions within the cap", (
    tester,
  ) async {
    final requests = <http.Request>[];
    final delegate = board(
      requests,
      [],
      (_) => json({
        ..._view(),
        'roles': <Object>[],
        'delegable': ['mission.read', 'pool.read'],
      }),
      operations: [
        (
          'X',
          [
            'mission.read',
            'access.read',
            'access.manage',
            'approval.class',
            'pool.read',
          ],
        ),
      ],
    );
    expect(
      actionBlock(
        'approval.class',
        held: {'approval.class', 'mission.read'},
        operationScope: true,
        cap: {'mission.read'},
      ),
      contains('delegation cap'),
    );
    expect(
      actionBlock(
        'station.configure',
        held: {'station.configure'},
        operationScope: true,
        cap: {'station.configure'},
      ),
      contains('only Operation actions'),
    );
    expect(
      actionBlock(
        'mission.read',
        held: {'mission.read'},
        operationScope: true,
        cap: {'mission.read'},
      ),
      isNull,
    );
    await openAccess(tester, delegate);
    await tester.tap(find.text('Access').first);
    await tester.pumpAndSettle();
    expect(requests.any((r) => r.url.path == '/v1/user/access'), isFalse);
  });

  testWidgets('a stale save shows the reload prompt and saves nothing', (
    tester,
  ) async {
    final requests = <http.Request>[];
    final controller = board(
      requests,
      ['access.read', 'access.manage'],
      (request) => request.method == 'GET'
          ? json(_view())
          : json({
              'error': 'the access policy changed since you read it; reload it',
            }, 409),
    );
    await openAccess(tester, controller);
    await tester.tap(find.byTooltip('Delete operator'));
    await tester.pumpAndSettle();
    expect(find.text('Reload'), findsOneWidget);
    final write = requests.firstWhere((r) => r.method == 'DELETE');
    expect(write.headers['if-match'], '"r1"');
    await tester.tap(find.text('Reload'));
    await tester.pumpAndSettle();
    expect(find.text('Reload'), findsNothing);
  });

  testWidgets(
    'a reset password is shown once and is gone when the dialog closes',
    (tester) async {
      final requests = <http.Request>[];
      final controller = board(requests, ['account.manage'], (request) {
        if (request.method == 'POST') {
          return json({'secret': 'n3w-s3cret'});
        }
        return json([
          {
            'name': 'ann',
            'groups': ['ops'],
            'disabled': false,
          },
        ]);
      });
      await openAccess(tester, controller);
      expect(find.text('ann'), findsOneWidget);
      await tester.tap(find.text('Reset password'));
      await tester.pumpAndSettle();
      expect(find.text('n3w-s3cret'), findsOneWidget);
      expect(find.text('Copy'), findsOneWidget);
      await tester.tap(find.text('Done'));
      await tester.pumpAndSettle();
      expect(find.text('n3w-s3cret'), findsNothing);
      expect(find.textContaining('n3w-s3cret'), findsNothing);
      expect(
        find.byTooltip('Groups of ann'),
        findsNothing,
        reason: 'groups need access.manage',
      );
    },
  );
}
