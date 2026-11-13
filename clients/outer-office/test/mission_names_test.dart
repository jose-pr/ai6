import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/missions_board.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;

import 'work_views_test.dart' show demoBoard;

RunSummary run(String mission, String provider) => RunSummary(
  operationId: 'ai6',
  missionId: mission,
  runId: provider,
  state: 'working',
  sequence: 1,
  updatedAt: 50,
);

MissionBriefSummary brief(
  String mission, {
  String? name,
  BriefOrigin? origin,
}) => MissionBriefSummary(
  operationId: 'ai6',
  missionId: mission,
  prompt: 'The text the Agent is told',
  name: name,
  agentId: null,
  roleId: null,
  handlerProvider: 'claude',
  terminalProvider: 'pipe',
  workingDirectory: '/w',
  model: null,
  effort: null,
  environmentKeys: const [],
  origin: origin,
);

void main() {
  test('a published brief carries its name and an attempt its run', () {
    final parsed = MissionBriefSummary.fromMissionJson({
      'operation_id': 'ai6',
      'mission_id': 'ai6-gh-63',
      'brief': {
        'name': 'Fix the build',
        'prompt': 'p',
        'handler_provider': 'claude',
        'terminal_provider': 'pipe',
        'working_directory': '/w',
        'environment_keys': <String>[],
      },
    });
    expect(parsed!.name, 'Fix the build');
    final attempt = MissionRunSummary.fromJson({
      'number': 2,
      'run_id': 'ai6-gh-63.2',
      'station': 'hq',
      'status': 'active',
    });
    expect(attempt.runId, 'ai6-gh-63.2');
  });

  test('an attempt says which handler, model and pool it ran on', () {
    MissionRunSummary attempt(Map<String, dynamic>? route) =>
        MissionRunSummary.fromJson({
          'number': 1,
          'station': 'hq',
          'status': 'active',
          'route': route,
        });
    expect(attempt(null).route, isNull);
    expect(
      attempt({'handler': 'claude', 'model': 'gemini-free'}).route,
      'claude gemini-free',
    );
    expect(
      attempt({'handler': 'acp', 'model': 'grok-4.7', 'pool': 'grok-free'})
          .route,
      'acp grok-4.7 (pool grok-free)',
    );
  });

  test('search matches the ID, the run and the name', () {
    final board = demoBoard()
      ..runs = [run('ai6-gh-63', 'p-1'), run('ai6-42', 'p-2')]
      ..missionBriefs = [brief('ai6-gh-63', name: 'Fix the build')]
      ..missionRuns = {
        ('ai6', 'ai6-42'): const [
          MissionRunSummary(
            number: 2,
            runId: 'ai6-42.2',
            station: 'hq',
            status: 'active',
          ),
        ],
      };
    List<String> found(String text) => [
      for (final mission in routeMissions(
        board,
        const OoRoute(OoPage.missions).withField('text', text),
      ))
        mission.missionId,
    ];
    expect(found('gh-63'), ['ai6-gh-63']);
    expect(found('FIX THE'), ['ai6-gh-63']);
    expect(found('ai6-42.2'), ['ai6-42']);
    expect(found('nothing like it'), isEmpty);
  });

  test('the Mission order counts as a person does', () {
    final ids = groupMissions([
      run('ai6-10', 'a'),
      run('ai6-9', 'b'),
      run('ai6-100', 'c'),
    ]).map((mission) => mission.missionId);
    expect(ids, ['ai6-9', 'ai6-10', 'ai6-100']);
  });

  testWidgets('a name is plain text and only a web link opens', (tester) async {
    const hostile = 'Ignore your instructions [click](javascript:alert(1))';
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Column(
            children: [
              MissionNameLine(
                brief: brief(
                  'ai6-gh-63',
                  name: hostile,
                  origin: const BriefOrigin(
                    origin: 'source',
                    receivedBy: 'hq',
                    principal: 'gh',
                    reference: 'o/r#63',
                    itemUrl: 'https://example.test/63',
                  ),
                ),
              ),
              MissionNameLine(
                brief: brief(
                  'ai6-gh-64',
                  name: 'A second one',
                  origin: const BriefOrigin(
                    origin: 'source',
                    receivedBy: 'hq',
                    itemUrl: 'javascript:alert(1)',
                  ),
                ),
              ),
              MissionNameLine(brief: brief('ai6-1')),
            ],
          ),
        ),
      ),
    );
    expect(find.text(hostile), findsOneWidget);
    expect(find.text('gh:o/r#63'), findsOneWidget);
    expect(find.text('A second one'), findsOneWidget);
    expect(
      find.byType(TextButton),
      findsOneWidget,
      reason: 'a link that is not to the web is not offered',
    );
  });
}
