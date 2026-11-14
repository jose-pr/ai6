import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/chat_layout.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/missions_board.dart';
import 'package:outer_office/shell/operation_names.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'chat_layout_test.dart' show chord, connect, signIn;
import 'polish_test.dart' show FakeChannel, atSize, signedIn;
import 'shell_navigation.dart';

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    OperationNames.use(const {});
  });

  group('the Missions page', () {
    MissionBoard board({List<String> actions = const ['mission.read']}) {
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: MockClient((request) async => http.Response('[]', 404)),
      );
      board.accessToken = 't';
      board.loading = false;
      board.userSession = UserSessionSummary.fromJson({
        'person_id': 'ana',
        'name': 'ana',
        'groups': <String>[],
        'expires_at': 0,
        'operations': [
          {'operation_id': 'G', 'name': 'Guatiao', 'actions': actions},
          {'operation_id': 'L', 'name': 'LineupMux', 'actions': actions},
          {'operation_id': 'Q', 'actions': actions},
        ],
      });
      return board;
    }

    Future<void> pumpEmpty(
      WidgetTester tester, {
      ValueChanged<String>? onDraft,
      String? operation,
    }) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: EmptyBoard(
              operations: const ['G', 'L', 'Q'],
              onDraft: onDraft,
              operation: operation,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
    }

    testWidgets(
      'an Operation is read by its name, by its ID when it has none',
      (tester) async {
        final b = board();
        addTearDown(b.dispose);
        expect(OperationNames.of('G'), 'Guatiao');
        expect(OperationNames.of('L'), 'LineupMux');
        expect(OperationNames.of('Q'), 'Q');
        expect(OperationNames.of('unknown'), 'unknown');
      },
    );

    testWidgets('the empty queue offers New Mission, a draft with Moneypenny, '
        'for the picked Operation', (tester) async {
      final b = board();
      addTearDown(b.dispose);
      final drafted = <String>[];
      await pumpEmpty(tester, onDraft: drafted.add, operation: 'L');
      expect(find.text('No Missions yet'), findsOneWidget);
      expect(find.text('New Mission'), findsOneWidget);
      // The picker starts on the Operation the page is filtered to, by its name.
      expect(find.widgetWithText(TextField, 'LineupMux'), findsOneWidget);
      expect(find.text('Draft with Moneypenny'), findsNothing);
      await tester.tap(find.text('New Mission'));
      expect(drafted, ['L']);

      await tester.tap(find.byKey(const Key('new-mission-operation')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Guatiao').last);
      await tester.pumpAndSettle();
      await tester.tap(find.text('New Mission'));
      expect(drafted, ['L', 'G']);
    });

    testWidgets('without Moneypenny there is no button, and a person who '
        'may not start one is told who can', (tester) async {
      final b = board();
      addTearDown(b.dispose);
      await pumpEmpty(tester);
      expect(find.text('New Mission'), findsNothing);

      await tester.pumpWidget(
        const MaterialApp(home: Scaffold(body: EmptyBoard())),
      );
      expect(find.textContaining('a controller grants'), findsOneWidget);
      expect(find.text('New Mission'), findsNothing);
    });

    testWidgets('a state with no Mission is no filter, a picked one stays', (
      tester,
    ) async {
      final run = const RunSummary(
        operationId: 'G',
        missionId: 'm1',
        runId: 'r1',
        state: 'working',
        sequence: 1,
      );
      Future<void> header(List<String> selected) => tester.pumpWidget(
        MaterialApp(
          home: Material(
            child: BoardHeader(
              missions: groupMissions([run]),
              selectedStates: selected,
              onSelectStates: (_) {},
              sort: const SizedBox.shrink(),
            ),
          ),
        ),
      );
      await header(const []);
      expect(find.textContaining('All'), findsOneWidget);
      expect(find.textContaining('working'), findsOneWidget);
      expect(find.textContaining('queued'), findsNothing);
      expect(find.textContaining('failed'), findsNothing);
      expect(find.byType(ChoiceChip), findsNWidgets(2));
      await header(const ['failed']);
      expect(find.textContaining('failed'), findsOneWidget);
    });
  });

  group('Station settings', () {
    Future<MissionBoard> boardWith(
      WidgetTester tester, {
      required bool execution,
    }) async {
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: MockClient((request) async {
          if (request.url.path.endsWith('/settings')) {
            return http.Response(
              jsonEncode({
                'station_id': request.url.pathSegments[3],
                'max_concurrent_missions': 0,
                'min_free_memory_mb': 256,
                'max_cpu_percent': 90,
                'configured': false,
                'execution_enabled': execution,
              }),
              200,
              headers: {'content-type': 'application/json'},
            );
          }
          return http.Response('[]', 200);
        }),
      );
      board.accessToken = 'operator-token';
      board.loading = false;
      board.userSession = UserSessionSummary.fromJson({
        'person_id': 'ana',
        'name': 'ana',
        'groups': <String>[],
        'expires_at': 0,
        'operations': <Object>[],
        'stations': [
          {
            'station_id': 'control',
            'actions': ['station.read', 'station.configure'],
          },
          {
            'station_id': 'worker',
            'actions': ['station.read', 'station.configure'],
          },
        ],
      });
      board.stationBoard = StationBoardSummary(
        headquartersStation: 'control',
        stations: [
          for (final (id, roles) in [
            ('control', ['control']),
            ('worker', ['control', 'execution']),
          ])
            StationSummary(
              stationId: id,
              healthy: true,
              headquarters: id == 'control',
              seen: 0,
              roles: roles,
              running: 0,
              capacity: execution ? 4 : 0,
              operations: const [],
            ),
        ],
      );
      addTearDown(board.dispose);
      return board;
    }

    Future<void> open(
      WidgetTester tester,
      MissionBoard board,
      String uri,
    ) async {
      await atSize(tester, const Size(1400, 1000));
      tester.binding.platformDispatcher.defaultRouteNameTestValue = uri;
      addTearDown(
        tester.binding.platformDispatcher.clearDefaultRouteNameTestValue,
      );
      await tester.pumpWidget(
        OuterOfficeApp(board: board, desktopWindows: false),
      );
      await tester.pumpAndSettle();
    }

    testWidgets('a Station that cannot run Missions says so instead of a field '
        'at 0', (tester) async {
      final board = await boardWith(tester, execution: false);
      await open(tester, board, '/stations?tab=settings&station_id=control');
      expect(
        find.byKey(const Key('station-execution-unavailable')),
        findsOneWidget,
      );
      expect(
        find.textContaining('Execution unavailable on this Station'),
        findsOneWidget,
      );
      expect(find.byKey(const Key('station-capacity')), findsNothing);
    });

    testWidgets('with no Station named, Settings opens on one that runs '
        'Missions, and a named one stays', (tester) async {
      final board = await boardWith(tester, execution: true);
      await open(tester, board, '/stations?tab=settings');
      expect(
        tester
            .widget<DropdownButtonFormField<String>>(
              find.byType(DropdownButtonFormField<String>),
            )
            .initialValue,
        'worker',
      );
      expect(find.byKey(const Key('station-capacity')), findsOneWidget);
    });

    testWidgets('a Station card opens Settings on that Station', (
      tester,
    ) async {
      final board = await boardWith(tester, execution: true);
      await open(tester, board, '/stations');
      await tester.tap(find.byKey(const ValueKey('station-settings-control')));
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<DropdownButtonFormField<String>>(
              find.byType(DropdownButtonFormField<String>),
            )
            .initialValue,
        'control',
      );
    });
  });

  group('the rail while the chat is expanded', () {
    testWidgets('shows the destination and keeps the thread', (tester) async {
      await atSize(tester, const Size(1400, 900));
      final channel = FakeChannel();
      final board = signedIn(channel);
      addTearDown(board.dispose);
      final chat = await signIn(tester, board, channel);
      await chord(tester, LogicalKeyboardKey.keyI, shift: true);
      await connect(tester, channel);
      channel.answer({
        'type': 'entry',
        'entry': {
          'role': 'person',
          'text': 'Which Missions are blocked?',
          'at': 1.0,
        },
      });
      await tester.pumpAndSettle();
      expect(chat.layout, ChatLayout.expanded);

      await openPage(tester, 'Runs');
      expect(chat.layout, ChatLayout.docked);
      expect(find.text('Which Missions are blocked?'), findsOneWidget);
      expect(chat.controller.entries.map((entry) => entry.text), [
        'Which Missions are blocked?',
      ]);
    });
  });
}
