import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/assistant_panel.dart';
import 'package:outer_office/assistant/assistant_view.dart';
import 'package:outer_office/assistant/entry_kind.dart';
import 'package:outer_office/assistant/navigation_card.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/oo_shell.dart' show OoPage;

Map<String, Object?> proposal({
  String status = 'pending',
  String? result,
  Map<String, Object?> extra = const {},
}) => {
  'id': 'proposal-1',
  'tool': 'propose_mission_control',
  'summary': 'retry Mission m-12 in Operation G',
  'request': {
    'method': 'POST',
    'path': '/v1/user/operations/G/missions/m-12/actions',
    'body': {'action': 'retry'},
  },
  'needs': {'action': 'mission.control', 'on': 'operation:G'},
  'status': status,
  'result': result,
  'read': [
    {
      'tool': 'missions',
      'ids': {'operation_id': 'G'},
    },
    {
      'tool': 'mission',
      'ids': {'operation_id': 'G', 'mission_id': 'm-12'},
    },
    {'tool': 'profile', 'ids': <String, Object?>{}},
  ],
  'opens': 'app:/missions?operation_id=G&mission_id=m-12',
  ...extra,
};

Future<void> show(
  WidgetTester tester,
  Map<String, Object?> data, {
  void Function(String id)? onConfirm,
  void Function(String link)? onOpen,
  Duration confirmingAfter = const Duration(seconds: 30),
}) async {
  tester.view.physicalSize = const Size(700, 1200);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: ProposalCard(
            proposal: data,
            onConfirm: onConfirm ?? (_) {},
            onCancel: (_) {},
            onOpen: onOpen,
            confirmingAfter: confirmingAfter,
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('a proposal says what she read, what it needs and the page it '
      'affects', (tester) async {
    final opened = <String>[];
    await show(tester, proposal(), onOpen: opened.add);
    expect(
      find.text('Proposal · needs mission.control on operation:G'),
      findsOneWidget,
    );
    expect(
      find.text('Based on: missions (G), mission (G/m-12), profile'),
      findsOneWidget,
    );
    expect(find.text('Affects: Missions › G › m-12'), findsOneWidget);
    expect(find.text('Nothing changes until you confirm.'), findsOneWidget);
    expect(find.byType(OutsideTextStrip), findsNothing);
    await tester.tap(find.byKey(const ValueKey('proposal-opens')));
    expect(opened, ['app:/missions?operation_id=G&mission_id=m-12']);
  });

  testWidgets('a roster proposal names its record the way the page does', (
    tester,
  ) async {
    await show(
      tester,
      proposal(extra: {'opens': 'app:/roster?kind=agent&key=009'}),
      onOpen: (_) {},
    );
    expect(find.text('Affects: Roster › agent 009'), findsOneWidget);
  });

  testWidgets('a proposal without a page or a way to open it shows no chip', (
    tester,
  ) async {
    await show(tester, proposal(extra: {'opens': null}), onOpen: (_) {});
    expect(find.byKey(const ValueKey('proposal-opens')), findsNothing);
    await show(tester, proposal());
    expect(find.byKey(const ValueKey('proposal-opens')), findsNothing);
  });

  testWidgets('an exposed proposal carries the strip naming what exposed it', (
    tester,
  ) async {
    await show(
      tester,
      proposal(
        extra: {
          'exposed': true,
          'exposed_by': ['tool:missions', 'personal:notes/a.md'],
        },
      ),
    );
    expect(find.text(OutsideTextStrip.message), findsOneWidget);
    expect(
      find.text('Read from: tool:missions, personal:notes/a.md'),
      findsOneWidget,
    );
  });

  testWidgets('each status has its word, and a refusal its reason', (
    tester,
  ) async {
    const words = {
      'pending': 'Waiting for you',
      'confirming': 'Applying',
      'done': 'Applied',
      'failed': 'Refused',
      'cancelled': 'Cancelled',
      'expired': 'Expired, nothing changed',
    };
    for (final MapEntry(:key, :value) in words.entries) {
      await show(
        tester,
        proposal(
          status: key,
          result: key == 'failed'
              ? 'AI6 answered 403: no mission.control'
              : null,
        ),
      );
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('proposal-status')),
          matching: find.text(value),
        ),
        findsOneWidget,
        reason: key,
      );
      expect(
        find.text('AI6 answered 403: no mission.control'),
        key == 'failed' ? findsOneWidget : findsNothing,
      );
      expect(
        find.widgetWithText(FilledButton, 'Confirm'),
        key == 'pending' ? findsOneWidget : findsNothing,
      );
    }
  });

  testWidgets('after done the page chip is Open', (tester) async {
    await show(tester, proposal(status: 'done'), onOpen: (_) {});
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('proposal-opens')),
        matching: find.text('Open'),
      ),
      findsOneWidget,
    );
    expect(find.textContaining('Affects:'), findsNothing);
  });

  testWidgets('Confirm answers once: it is disabled after the first press', (
    tester,
  ) async {
    final confirmed = <String>[];
    await show(tester, proposal(), onConfirm: confirmed.add);
    await tester.tap(find.widgetWithText(FilledButton, 'Confirm'));
    await tester.pump();
    expect(confirmed, ['proposal-1']);
    final button = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, 'Confirm'),
    );
    expect(button.onPressed, isNull);
    await tester.tap(
      find.widgetWithText(FilledButton, 'Confirm'),
      warnIfMissed: false,
    );
    expect(confirmed, hasLength(1));
  });

  testWidgets('a proposal applying for thirty seconds is an unknown outcome, '
      'with no second Confirm', (tester) async {
    await show(tester, proposal(status: 'confirming'));
    expect(find.text('Applying'), findsOneWidget);
    await tester.pump(const Duration(seconds: 29));
    expect(find.text('Applying'), findsOneWidget);
    await tester.pump(const Duration(seconds: 2));
    expect(find.text('Applying'), findsNothing);
    expect(find.text('Outcome unknown, check the page'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Confirm'), findsNothing);
    // Told later that it applied, the card says so.
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProposalCard(
            proposal: proposal(status: 'done'),
            onConfirm: (_) {},
            onCancel: (_) {},
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Applied'), findsOneWidget);
    expect(find.text('Outcome unknown, check the page'), findsNothing);
  });

  testWidgets('every kind of entry has its icon and semantics label', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    tester.view.physicalSize = const Size(900, 2400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: Column(
              children: [
                const MoneypennyAvatar(),
                ProposalCard(
                  proposal: proposal(),
                  onConfirm: (_) {},
                  onCancel: (_) {},
                ),
                const AssistantViewCard(
                  json: {
                    'version': 1,
                    'blocks': [
                      {'kind': 'text', 'markdown': 'Hello'},
                    ],
                  },
                ),
                NavigationCard(
                  data: {
                    'id': 'nav-1',
                    'route': const OoRoute(OoPage.overview).toJson(),
                    'label': 'The overview',
                  },
                ),
                const EntryKindIcon(EntryKind.pageLink),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    for (final kind in EntryKind.values) {
      expect(
        find.bySemanticsLabel(kind.label),
        findsOneWidget,
        reason: '$kind',
      );
    }
    expect(
      EntryKind.proposal.label,
      'Proposal: nothing changes until you confirm',
    );
    expect(EntryKind.navigation.label, 'Link to a page in Outer Office');
    expect(
      EntryKind.pageLink.label,
      'Page she published, opens outside Outer Office',
    );
    for (final (role, kind) in [
      ('moneypenny', EntryKind.answer),
      ('proposal', EntryKind.proposal),
      ('view', EntryKind.view),
      ('navigation', EntryKind.navigation),
      ('page', EntryKind.pageLink),
    ]) {
      expect(EntryKind.of(AssistantEntry(role: role, text: '', at: 0)), kind);
    }
    expect(
      EntryKind.of(const AssistantEntry(role: 'person', text: '', at: 0)),
      isNull,
    );
    semantics.dispose();
  });
}
