import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/widgets/oo_principals.dart';

import 'shell_navigation.dart';
import 'widget_test.dart' show userProfileResponse;

const _sid = 'S-1-5-21-1-2-3-1000';

http.Response _json(Object body) => http.Response(
  jsonEncode(body),
  200,
  headers: {'content-type': 'application/json'},
);

/// A gateway that has seen Ada sign in (by her directory SID) and nobody else.
http.Response _answer(http.Request request, List<http.Request> asked) {
  asked.add(request);
  if (request.url.path == '/v1/user/audit') {
    return _json([
      for (final (id, actor) in [
        ('a1', _sid),
        ('a2', 'ghost-1'),
        ('a3', '$_sid via key k1'),
      ])
        {
          'id': id,
          'actor': actor,
          'action': 'moneypenny.page.opened',
          'operation': 'G',
          'target': 'page-1',
          'at': 1900000000,
        },
    ]);
  }
  if (request.url.path == '/v1/user/principals' && request.method == 'POST') {
    final ids = (jsonDecode(request.body) as Map)['ids'] as List;
    return _json({
      'people': [
        if (ids.contains(_sid))
          {
            'id': _sid,
            'name': 'ada',
            'label': 'Ada Lovelace',
            'groups': <String>[],
            'last_seen': 1.0,
          },
      ],
      'groups': <String>[],
    });
  }
  return http.Response('{}', 404);
}

MissionBoard _board(List<http.Request> asked) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async => _answer(request, asked)),
  );
  board.accessToken = 'token';
  board.loading = false;
  board.userRun = UserRunSummary.fromJson(
    jsonDecode(userProfileResponse(actions: ['mission.read', 'audit.read']))
        as Map<String, dynamic>,
  );
  return board;
}

void main() {
  testWidgets(
    'the audit names a person who signed in and keeps an unknown id',
    (tester) async {
      tester.view
        ..physicalSize = const Size(1400, 1000)
        ..devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final asked = <http.Request>[];
      final board = _board(asked);
      addTearDown(board.dispose);
      await tester.pumpWidget(
        OuterOfficeApp(board: board, desktopWindows: false),
      );
      await openPage(tester, 'Audit');
      await tester.pumpAndSettle();

      expect(find.textContaining('Ada Lovelace'), findsNWidgets(2));
      expect(
        find.text('ghost-1'),
        findsOneWidget,
        reason: 'never seen: the id',
      );
      expect(
        find.textContaining(_sid),
        findsNothing,
        reason: 'the SID is not printed',
      );
      expect(
        asked.where((r) => r.method == 'POST').length,
        1,
        reason: 'one batch for every id on the page',
      );
      expect(find.textContaining('via key k1'), findsOneWidget);
    },
  );

  testWidgets('a badge shows the id muted for a person never seen', (
    tester,
  ) async {
    final people = OoPrincipals(
      lookup: (ids) async => [
        if (ids.contains('1000'))
          const OoPerson(id: '1000', label: 'Ada Lovelace'),
      ],
      search: (_) async => const OoPrincipalHits(),
    );
    addTearDown(people.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: OoPrincipalsScope(
          principals: people,
          child: const Column(
            children: [
              OoPrincipalBadge('person:1000'),
              OoPrincipalBadge('person:2000'),
              OoPrincipalBadge('group:domain admins'),
              OoPrincipalBadge('station:hq'),
            ],
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Ada Lovelace'), findsOneWidget);
    expect(find.text('2000'), findsOneWidget);
    expect(find.text('group:domain admins'), findsOneWidget);
    expect(find.text('station:hq'), findsOneWidget);
  });

  testWidgets('the picker finds a person and a group by name', (tester) async {
    final people = OoPrincipals(
      lookup: (_) async => const [],
      search: (text) async => OoPrincipalHits(
        people: [
          if ('ada lovelace'.contains(text.toLowerCase()))
            const OoPerson(id: _sid, label: 'Ada Lovelace'),
        ],
        groups: [
          if ('domain admins'.contains(text.toLowerCase())) 'domain admins',
        ],
      ),
    );
    addTearDown(people.dispose);
    final subject = TextEditingController();
    addTearDown(subject.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: OoPrincipalPicker(controller: subject, principals: people),
        ),
      ),
    );
    await tester.enterText(
      find.byKey(const ValueKey('principal-picker')),
      'love',
    );
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Ada Lovelace'));
    await tester.pumpAndSettle();
    expect(subject.text, 'person:$_sid');

    subject.text = '';
    await tester.enterText(
      find.byKey(const ValueKey('principal-picker')),
      'domain',
    );
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    await tester.tap(find.text('domain admins').first);
    await tester.pumpAndSettle();
    expect(subject.text, 'group:domain admins');

    // A subject written out is taken as it is.
    await tester.enterText(
      find.byKey(const ValueKey('principal-picker')),
      'account:svc',
    );
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.byKey(const ValueKey('principal-options')), findsNothing);
  });
}
