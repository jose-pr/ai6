import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/controls/mission_actions.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/mission_outcome.dart';
import 'package:outer_office/theme/oo_theme.dart';

/// A Mission of Operation G whose one attempt filed [report], with [verdict] on it; a verdict
/// given is kept in [given] and answered back.
Future<MissionBoard> _board(
  List<String> actions, {
  String? report = 'All **merged**.',
  Map<String, Object?>? verdict,
  List<Map<String, dynamic>>? given,
}) async {
  var standing = verdict;
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      final path = request.url.path;
      http.Response json(Object body, [int status = 200]) => http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json'},
      );
      return switch (path) {
        '/v1/auth/login' => json({'token': 'user-token'}, 201),
        '/v1/board' || '/v1/missions' => json(const <Object>[]),
        '/v1/stations' => json({'headquarters_station': null, 'stations': []}),
        '/v1/user/profile' => json({
          'person_id': 'operator-007',
          'name': 'operator',
          'display_name': null,
          'groups': [],
          'expires_at': 1_900_000_000,
          'operations': [
            {'operation_id': 'G', 'actions': actions},
          ],
          'stations': [],
        }),
        '/v1/operations/G/missions/m1' => json({
          'operation_id': 'G',
          'mission_id': 'm1',
          'handle': 'G-1',
          'sessions': const <Object>[],
          'attempts': [
            {
              'attempt': 1,
              'session_id': 's1',
              'station': 'hq',
              'status': 'ended',
              'final_report': ?report,
              'verdict': ?standing,
            },
          ],
        }),
        '/v1/user/operations/G/missions/m1/verdict' => () {
          final body = jsonDecode(request.body) as Map<String, dynamic>;
          given?.add(body);
          standing = {...body, 'by': 'operator-007', 'at': 1_800_000_000.0};
          return json({
            'operation_id': 'G',
            'mission_id': 'm1',
            'attempt': 1,
            'session_id': 's1',
            ...standing!,
          });
        }(),
        _ => json(const <Object>[]),
      };
    }),
  );
  await board.login('operator', 'secret');
  addTearDown(board.dispose);
  return board;
}

Future<void> _show(WidgetTester tester, Widget child) async {
  tester.view.physicalSize = const Size(1280, 1000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    MaterialApp(
      theme: ooTheme(OoThemeId.values.first),
      home: Scaffold(body: SingleChildScrollView(child: child)),
    ),
  );
  await tester.pumpAndSettle();
}

MissionOutcomeSection _section(MissionBoard board, String state) =>
    MissionOutcomeSection(
      board: board,
      operationId: 'G',
      missionId: 'm1',
      state: state,
    );

void main() {
  testWidgets('the final message is rendered and a reader cannot judge it', (
    tester,
  ) async {
    final board = await _board(['mission.read']);
    await _show(tester, _section(board, 'done'));
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is RichText &&
            widget.text.toPlainText().contains('All merged.'),
      ),
      findsOneWidget,
    );
    expect(find.text('Dispute it'), findsNothing);
  });

  testWidgets('a person with mission.verify gives a verdict with a note', (
    tester,
  ) async {
    final given = <Map<String, dynamic>>[];
    final board = await _board([
      'mission.read',
      'mission.verify',
    ], given: given);
    await _show(tester, _section(board, 'done'));
    await tester.tap(find.text('It failed verification'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'the tag was never pushed');
    await tester.tap(find.text('Record it'));
    await tester.pumpAndSettle();
    expect(given, [
      {'verdict': 'failed-verification', 'note': 'the tag was never pushed'},
    ]);
    expect(
      find.textContaining('Failed verification, by operator-007'),
      findsOneWidget,
    );
  });

  testWidgets('a held Mission says why and takes a verdict', (tester) async {
    final board = await _board(['mission.verify'], report: null);
    await _show(tester, _section(board, 'held'));
    expect(find.textContaining('Held:'), findsOneWidget);
    expect(find.text('No final message is on record.'), findsOneWidget);
    expect(find.text('Confirm it'), findsOneWidget);
  });

  testWidgets('a Mission still working has nothing to judge', (tester) async {
    final board = await _board(['mission.verify'], report: null);
    await _show(tester, _section(board, 'working'));
    expect(find.text('OUTCOME'), findsNothing);
  });

  test('a held Mission can be cancelled, not resumed or retried', () {
    final options = {
      for (final option in missionControls(state: 'held', permitted: true))
        option.action: option.unavailable,
    };
    expect(options['cancel'], isNull);
    expect(options['resume'], isNotNull);
    expect(options['retry'], contains('verdict'));
  });
}
