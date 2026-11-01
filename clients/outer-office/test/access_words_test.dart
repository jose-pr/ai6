import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/access/access_words.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/routes/oo_route.dart';
import 'package:outer_office/shell/oo_shell.dart';
import 'package:outer_office/shell/operations_page.dart';

import 'moneypenny_off_test.dart' show gateway, profile;

void main() {
  group('Your access', () {
    test('plain words by group, no action lost', () {
      final groups = accessGroups(const [
        'session.read',
        'mission.read',
        'mission.control',
        'approval.decide',
        'pool.read',
        'cable.send',
        'audit.read',
        'agent.configure',
        'something.new',
      ]);
      expect(groups.map((group) => group.name), [
        'Missions',
        'Sessions',
        'Green ink',
        'Agents',
        'Pools',
        'Cables',
        'Audit',
        'Other',
      ]);
      expect(groups.first.words, ['see', 'cancel, retry, resume, interrupt']);
      expect(groups.first.actions, ['mission.read', 'mission.control']);
      expect(groups.last.words, ['something.new']);
    });

    testWidgets('the Operations page shows words, the names on hover', (
      tester,
    ) async {
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: gateway({}),
      );
      board.accessToken = 'token';
      board.loading = false;
      board.userSession = UserSessionSummary.fromJson(profile());
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: OperationsPage(
              board: board,
              route: const OoRoute(OoPage.operations),
              onRoute: (_) {},
              pages: const [OoPage.operations],
            ),
          ),
        ),
      );
      expect(find.textContaining('mission.read'), findsNothing);
      expect(find.textContaining('see, queue new'), findsWidgets);
      expect(find.byTooltip('mission.read, mission.create'), findsWidgets);
      await tester.pumpWidget(const SizedBox());
      board.dispose();
    });
  });
}
