import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/roster/found_libraries.dart';

const _digest =
    'sha256:abababababababababababababababababababababababababababababababab';
const _key =
    'ed25519:1212121212121212121212121212121212121212121212121212121212121212';

/// A board whose gateway reports one Station that found a signed library, and answers an
/// approval as held until it carries `confirm`.
MissionBoard _board(
  List<http.Request> requests, {
  required List<String> globalActions,
  int approvalStatus = 200,
}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      requests.add(request);
      final path = request.url.path;
      if (path == '/v1/user/stations/station-a/libraries') {
        return http.Response(
          jsonEncode({
            'host': 'station-a',
            'libraries': [
              {
                'file': 'ai6_extra.dll',
                'digest': _digest,
                'kinds': ['ai6_agent_handler'],
                'verdict': 'unpinned',
                'signed': true,
                'publisher': _key,
              },
            ],
          }),
          200,
        );
      }
      if (path.endsWith('/libraries/approve')) {
        if (approvalStatus != 200) {
          return http.Response(
            jsonEncode({'refused': 'station-a reports no such library'}),
            approvalStatus,
          );
        }
        final confirmed =
            (jsonDecode(request.body) as Map<String, dynamic>)['confirm'] ==
            true;
        return http.Response(
          jsonEncode(
            confirmed
                ? {
                    'revision': '13',
                    'weakened': ['publisher $_key alone admits 1 library'],
                  }
                : {
                    'applied': false,
                    'changes': [
                      'station-a will load ai6_extra.dll when its bytes are $_digest',
                    ],
                    'weakened': ['publisher $_key alone admits 1 library'],
                  },
          ),
          200,
        );
      }
      return http.Response('[]', 200);
    }),
  );
  board.accessToken = 'operator-token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'operator-007',
    name: 'operator-007',
    displayName: null,
    groups: const [],
    expiresAt: 1e12,
    operations: const [],
    globalActions: globalActions,
  );
  board.stationBoard = StationBoardSummary.fromJson({
    'headquarters_station': 'station-a',
    'stations': [
      {
        'station_id': 'station-a',
        'healthy': true,
        'headquarters': true,
        'seen': 1000,
        'roles': ['control', 'execution'],
        'running': 0,
        'capacity': 4,
        'operations': <String>[],
      },
    ],
  });
  return board;
}

Future<void> _show(
  WidgetTester tester,
  MissionBoard board, {
  VoidCallback? onApproved,
}) async {
  tester.view.physicalSize = const Size(900, 1200);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: FoundLibrariesPanel(
          board: board,
          onApproved: onApproved ?? () {},
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

List<Map<String, dynamic>> _approvals(List<http.Request> requests) => [
  for (final request in requests)
    if (request.url.path.endsWith('/libraries/approve'))
      jsonDecode(request.body) as Map<String, dynamic>,
];

void main() {
  testWidgets('a library a Station found is shown with what was read of it', (
    tester,
  ) async {
    final requests = <http.Request>[];
    await _show(tester, _board(requests, globalActions: const []));

    expect(find.text('Libraries found and not loaded'), findsOneWidget);
    expect(find.text('Station station-a'), findsOneWidget);
    expect(find.text('ai6_extra.dll'), findsOneWidget);
    expect(find.textContaining(_digest), findsOneWidget);
    expect(find.textContaining('signed by $_key'), findsOneWidget);
    expect(find.textContaining('plugin.manage'), findsOneWidget);
    // Without the action there is nothing to press.
    expect(find.text('Approve ai6_extra.dll'), findsNothing);
    expect(_approvals(requests), isEmpty);
  });

  testWidgets(
    'approving shows what will load first and stores only on confirm',
    (tester) async {
      final requests = <http.Request>[];
      var approved = 0;
      await _show(
        tester,
        _board(requests, globalActions: const ['plugin.manage']),
        onApproved: () => approved++,
      );

      await tester.tap(find.text('Approve ai6_extra.dll'));
      await tester.pumpAndSettle();
      // The dialog states the digest, kinds, publisher and what the gateway says it changes.
      expect(find.text('Let station-a run ai6_extra.dll?'), findsOneWidget);
      expect(find.text(_digest), findsOneWidget);
      expect(find.textContaining('Kinds: ai6_agent_handler'), findsOneWidget);
      expect(find.text('Publisher: $_key'), findsOneWidget);
      expect(
        find.textContaining('when its bytes are $_digest'),
        findsOneWidget,
      );
      expect(find.textContaining('alone admits 1 library'), findsOneWidget);
      expect(_approvals(requests), [
        {
          'file': 'ai6_extra.dll',
          'digest': _digest,
          'trust': 'pin',
          'confirm': false,
        },
      ]);

      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(
        _approvals(requests),
        hasLength(1),
        reason: 'nothing was confirmed',
      );
      expect(approved, 0);

      await tester.tap(find.text('Approve ai6_extra.dll'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Approve digest'));
      await tester.pumpAndSettle();
      expect(_approvals(requests).last, {
        'file': 'ai6_extra.dll',
        'digest': _digest,
        'trust': 'pin',
        'confirm': true,
      });
      expect(approved, 1);
    },
  );

  testWidgets('trusting the publisher asks for a publisher trust', (
    tester,
  ) async {
    final requests = <http.Request>[];
    await _show(
      tester,
      _board(requests, globalActions: const ['plugin.manage']),
    );

    await tester.tap(find.text('Trust publisher of ai6_extra.dll'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Trust publisher'));
    await tester.pumpAndSettle();
    expect(_approvals(requests).map((body) => body['trust']), [
      'publisher',
      'publisher',
    ]);
    expect(_approvals(requests).last['confirm'], true);
  });

  testWidgets(
    'a refusal is shown in the gateway\'s words and nothing is stored',
    (tester) async {
      final requests = <http.Request>[];
      var approved = 0;
      await _show(
        tester,
        _board(
          requests,
          globalActions: const ['plugin.manage'],
          approvalStatus: 422,
        ),
        onApproved: () => approved++,
      );

      await tester.tap(find.text('Approve ai6_extra.dll'));
      await tester.pumpAndSettle();
      expect(
        find.textContaining('Refused: station-a reports no such library'),
        findsOneWidget,
      );
      expect(find.text('Approve digest'), findsNothing);
      expect(approved, 0);
    },
  );
}
