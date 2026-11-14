import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/mission_draft.dart';
import 'package:outer_office/shell/operation_names.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'polish_test.dart' show FakeChannel;

/// What a draft card reads of the board, as the test sets it.
class FakeReads extends ChangeNotifier implements MissionDraftReads {
  DraftPlacement? where = const DraftPlacement(
    best: 'worker · local · Agent 007',
    reasons: ['station-b: it has not checked in'],
  );
  final standings = <String, MissionStanding>{};
  String? report;
  int placements = 0;

  @override
  Future<DraftPlacement?> placement(String operationId) async {
    placements++;
    return where;
  }

  @override
  MissionStanding? standing(String operationId, String missionId) =>
      standings['$operationId/$missionId'];

  @override
  Future<String?> finalReport(String operationId, String missionId) async =>
      report;

  void stand(String mission, MissionStanding standing) {
    standings['G/$mission'] = standing;
    notifyListeners();
  }
}

Map<String, Object?> proposal({
  String status = 'pending',
  String? result,
  Map<String, Object?>? receipt,
  String opens = 'app:/missions?operation_id=G&state=queued',
}) => {
  'id': 'proposal-1',
  'tool': 'propose_mission',
  'summary': 'Queue a Mission in Operation G: Fix the build',
  'request': {
    'method': 'POST',
    'path': '/v1/user/operations/G/missions',
    'body': {'title': 'Fix the build', 'prompt': 'Make CI green.'},
  },
  'needs': {'action': 'mission.create', 'on': 'operation:G'},
  'status': status,
  'result': result,
  'receipt': ?receipt,
  'opens': opens,
};

class Harness {
  final confirmed = <String>[];
  final cancelled = <String>[];
  final revised = <(String, String)>[];
  final opened = <String>[];
}

Future<Harness> show(
  WidgetTester tester,
  FakeReads reads,
  Map<String, Object?> data,
) async {
  final harness = Harness();
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: MissionDraftCard(
            proposal: data,
            reads: reads,
            onConfirm: harness.confirmed.add,
            onCancel: harness.cancelled.add,
            onRevise: (id, text) => harness.revised.add((id, text)),
            onOpen: harness.opened.add,
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return harness;
}

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    OperationNames.use(const {});
  });

  group('a Mission draft in the conversation', () {
    testWidgets('shows the brief to edit, where it would run, and Confirm', (
      tester,
    ) async {
      final reads = FakeReads();
      addTearDown(reads.dispose);
      await show(tester, reads, proposal());
      expect(find.text('Mission draft · G'), findsOneWidget);
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('draft-title')))
            .controller!
            .text,
        'Fix the build',
      );
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('draft-brief')))
            .controller!
            .text,
        'Make CI green.',
      );
      expect(
        find.text('Would run on worker · local · Agent 007.'),
        findsOneWidget,
      );
      expect(find.text('station-b: it has not checked in'), findsOneWidget);
      expect(find.text('Confirm'), findsOneWidget);
      expect(reads.placements, 1);
    });

    testWidgets('names the Operation by its name', (tester) async {
      OperationNames.use(const {'G': 'Guatiao'});
      final reads = FakeReads();
      addTearDown(reads.dispose);
      await show(tester, reads, proposal());
      expect(find.text('Mission draft · Guatiao'), findsOneWidget);
    });

    testWidgets(
      'says nowhere can take it, and shows nothing without a preview',
      (tester) async {
        final reads = FakeReads()..where = const DraftPlacement();
        addTearDown(reads.dispose);
        await show(tester, reads, proposal());
        expect(find.text('Nowhere can take it now.'), findsOneWidget);
        reads.where = null;
        await tester.pumpWidget(const SizedBox());
        await show(tester, reads, proposal());
        expect(find.byKey(const Key('draft-placement')), findsNothing);
        expect(find.text('Confirm'), findsOneWidget);
      },
    );

    testWidgets('a double click confirms once', (tester) async {
      final reads = FakeReads();
      addTearDown(reads.dispose);
      final harness = await show(tester, reads, proposal());
      await tester.tap(find.byKey(const Key('draft-confirm')));
      await tester.tap(
        find.byKey(const Key('draft-confirm')),
        warnIfMissed: false,
      );
      await tester.pump();
      expect(harness.confirmed, ['proposal-1']);
      expect(
        tester
            .widget<FilledButton>(find.byKey(const Key('draft-confirm')))
            .onPressed,
        isNull,
      );
    });

    testWidgets('an edit is never confirmed: it asks for a new draft', (
      tester,
    ) async {
      final reads = FakeReads();
      addTearDown(reads.dispose);
      final harness = await show(tester, reads, proposal());
      await tester.enterText(find.byKey(const Key('draft-title')), 'Fix CI');
      await tester.pump();
      expect(find.byKey(const Key('draft-confirm')), findsNothing);
      expect(find.textContaining('not part of the proposal'), findsOneWidget);
      await tester.tap(find.byKey(const Key('draft-revise')));
      expect(harness.confirmed, isEmpty);
      expect(harness.revised, hasLength(1));
      expect(harness.revised.single.$1, 'proposal-1');
      expect(harness.revised.single.$2, contains('Title: Fix CI'));
      expect(harness.revised.single.$2, contains('Brief: Make CI green.'));
    });

    testWidgets('a stale confirmation shows expired and offers no Confirm', (
      tester,
    ) async {
      final reads = FakeReads();
      addTearDown(reads.dispose);
      await show(tester, reads, proposal(status: 'expired'));
      expect(find.text('Expired, nothing changed'), findsOneWidget);
      expect(find.text('Confirm'), findsNothing);
      expect(find.byKey(const Key('draft-open')), findsNothing);
    });

    testWidgets('a refusal says why and leaves no receipt', (tester) async {
      final reads = FakeReads();
      addTearDown(reads.dispose);
      await show(
        tester,
        reads,
        proposal(
          status: 'failed',
          result: 'Refused: the person may not mission.create on operation:G.',
        ),
      );
      expect(find.text('Refused'), findsOneWidget);
      expect(find.textContaining('may not mission.create'), findsOneWidget);
      expect(find.byKey(const Key('draft-open')), findsNothing);
    });

    testWidgets('queued, working, done: the receipt follows the board and '
        'reopens the Mission', (tester) async {
      final reads = FakeReads();
      addTearDown(reads.dispose);
      final harness = await show(
        tester,
        reads,
        proposal(
          status: 'done',
          result: 'AI6 answered 200',
          receipt: {
            'page': 'missions',
            'operation_id': 'G',
            'mission_id': 'm-9',
          },
          opens: 'app:/missions?operation_id=G&mission_id=m-9',
        ),
      );
      expect(find.text('Applied'), findsOneWidget);
      expect(find.text('Mission queued'), findsOneWidget);

      reads.stand('m-9', const MissionStanding('working'));
      await tester.pumpAndSettle();
      expect(find.text('Mission working'), findsOneWidget);

      reads
        ..report = 'CI is green again.'
        ..stand('m-9', const MissionStanding('done'));
      await tester.pumpAndSettle();
      expect(find.text('Mission done'), findsOneWidget);
      expect(find.byKey(const Key('draft-result')), findsOneWidget);
      expect(find.text('CI is green again.'), findsOneWidget);

      await tester.tap(find.byKey(const Key('draft-open')));
      expect(harness.opened, ['app:/missions?operation_id=G&mission_id=m-9']);
    });
  });

  group('the card follows the board', () {
    for (final (name, size) in const [
      ('desktop', Size(1200, 800)),
      ('phone', Size(390, 800)),
    ]) {
      testWidgets('a result written after the Mission settled appears on the '
          'next change of the board at $name width', (tester) async {
        tester.view.physicalSize = size;
        tester.view.devicePixelRatio = 1;
        addTearDown(tester.view.reset);
        final reads = FakeReads();
        addTearDown(reads.dispose);
        await show(
          tester,
          reads,
          proposal(
            status: 'done',
            result: 'AI6 answered 200',
            receipt: {
              'page': 'missions',
              'operation_id': 'G',
              'mission_id': 'm-9',
            },
          ),
        );
        reads.stand('m-9', const MissionStanding('done'));
        await tester.pumpAndSettle();
        expect(find.text('Mission done'), findsOneWidget);
        expect(find.byKey(const Key('draft-result')), findsNothing);

        reads
          ..report = 'The build is fixed.'
          ..notifyListeners();
        await tester.pumpAndSettle();
        expect(find.text('The build is fixed.'), findsOneWidget);
        expect(tester.takeException(), isNull);
      });
    }
  });

  group('in the chat panel', () {
    testWidgets('a reconnect shows the settled draft with its receipt, and a '
        'proposal frame settles the pending one', (tester) async {
      final channel = FakeChannel();
      final controller = AssistantController(
        token: 'user-token',
        connect: () => channel,
      );
      addTearDown(controller.dispose);
      final reads = FakeReads();
      addTearDown(reads.dispose);
      final opened = <String>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AssistantPanel(
              controller: controller,
              missions: reads,
              onOpen: opened.add,
            ),
          ),
        ),
      );
      channel.answer({
        'type': 'ready',
        'conversation': 'c-1',
        'entries': [
          {
            'role': 'proposal',
            'text': '',
            'at': 1.0,
            'data': proposal(status: 'pending'),
          },
        ],
      });
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('draft-confirm')), findsOneWidget);
      await tester.tap(find.byKey(const Key('draft-confirm')));
      await tester.pump();
      expect(channel.sent.last, {'type': 'confirm', 'proposal': 'proposal-1'});

      // The socket drops and comes back: the gateway's log holds the settled proposal.
      channel.answer({
        'type': 'ready',
        'conversation': 'c-1',
        'entries': [
          {
            'role': 'proposal',
            'text': '',
            'at': 1.0,
            'data': proposal(
              status: 'done',
              result: 'AI6 answered 200',
              receipt: {
                'page': 'missions',
                'operation_id': 'G',
                'mission_id': 'm-9',
              },
              opens: 'app:/missions?operation_id=G&mission_id=m-9',
            ),
          },
        ],
      });
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('draft-confirm')), findsNothing);
      expect(find.text('Mission queued'), findsOneWidget);
      await tester.tap(find.byKey(const Key('draft-open')));
      expect(opened, ['app:/missions?operation_id=G&mission_id=m-9']);
    });

    testWidgets('a draft again withdraws the proposal and sends the edits', (
      tester,
    ) async {
      final channel = FakeChannel();
      final controller = AssistantController(
        token: 'user-token',
        connect: () => channel,
      );
      addTearDown(controller.dispose);
      final reads = FakeReads();
      addTearDown(reads.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AssistantPanel(controller: controller, missions: reads),
          ),
        ),
      );
      channel.answer({
        'type': 'ready',
        'conversation': 'c-1',
        'entries': [
          {'role': 'proposal', 'text': '', 'at': 1.0, 'data': proposal()},
        ],
      });
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('draft-brief')), 'Fix CI.');
      await tester.pump();
      await tester.tap(find.byKey(const Key('draft-revise')));
      await tester.pump();
      expect(channel.sent.map((frame) => frame['type']), [
        'hello',
        'cancel',
        'message',
      ]);
      expect(channel.sent.last['text'], contains('Brief: Fix CI.'));
    });

    testWidgets('another proposal keeps the plain card', (tester) async {
      final channel = FakeChannel();
      final controller = AssistantController(
        token: 'user-token',
        connect: () => channel,
      );
      addTearDown(controller.dispose);
      final reads = FakeReads();
      addTearDown(reads.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AssistantPanel(controller: controller, missions: reads),
          ),
        ),
      );
      channel.answer({
        'type': 'ready',
        'conversation': 'c-1',
        'entries': [
          {
            'role': 'proposal',
            'text': '',
            'at': 1.0,
            'data': {...proposal(), 'tool': 'propose_mission_control'},
          },
        ],
      });
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('draft-title')), findsNothing);
      expect(find.text('Confirm'), findsOneWidget);
    });
  });
}
