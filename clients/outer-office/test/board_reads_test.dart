import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/settings/oo_config.dart';
import 'package:outer_office/shell/board_reads.dart';
import 'package:outer_office/shell/oo_freshness.dart';
import 'package:outer_office/shell/oo_shell.dart';

import 'moneypenny_off_test.dart' show app, gateway;

void main() {
  group('what the board reads', () {
    /// A signed-in board on [page], its reads counted in [counts], on a fake clock.
    Future<MissionBoard> running(
      WidgetTester tester,
      Map<String, int> counts,
      OoPage page, {
      RefreshSettings refresh = const RefreshSettings(),
    }) async {
      final before = ooClock;
      ooClock = () => tester.binding.clock.now();
      addTearDown(() => ooClock = before);
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        client: gateway(counts),
        pollInterval: refresh.sessions,
        limits: OoLimits(refresh: refresh),
      );
      await board.login('ana', 'secret');
      await tester.pump();
      board.showPage(page);
      await tester.pump();
      counts.clear();
      return board;
    }

    Future<void> run(WidgetTester tester, Duration span) async {
      const step = Duration(seconds: 1);
      for (var at = Duration.zero; at < span; at += step) {
        await tester.pump(step);
      }
    }

    int total(Map<String, int> counts) =>
        counts.values.fold(0, (sum, count) => sum + count);

    testWidgets('a hidden tab asks far less, and catches up when shown', (
      tester,
    ) async {
      final counts = <String, int>{};
      final board = await running(tester, counts, OoPage.overview);
      await run(tester, const Duration(seconds: 60));
      final shown = total(counts);
      expect(shown, greaterThan(40));

      counts.clear();
      board.setVisible(false);
      await run(tester, const Duration(seconds: 60));
      final hidden = total(counts);
      expect(hidden, lessThanOrEqualTo(shown ~/ 5), reason: '$counts');
      await run(tester, const Duration(seconds: 10));

      counts.clear();
      board.setVisible(true);
      await tester.pump();
      expect(counts['/v1/board'], 1, reason: 'shown again, it reads at once');
      board.dispose();
    });

    testWidgets('hidden = 0 reads nothing until the tab is back', (
      tester,
    ) async {
      final counts = <String, int>{};
      final board = await running(
        tester,
        counts,
        OoPage.overview,
        refresh: const RefreshSettings(hidden: null),
      );
      board.setVisible(false);
      await tester.pump();
      counts.clear();
      await run(tester, const Duration(seconds: 120));
      expect(total(counts), 0, reason: '$counts');
      board.setVisible(true);
      await tester.pump();
      expect(total(counts), greaterThan(0));
      board.dispose();
    });

    testWidgets(
      'a page that shows nothing from the board stops the fast reads',
      (tester) async {
        final counts = <String, int>{};
        final board = await running(tester, counts, OoPage.audit);
        await run(tester, const Duration(seconds: 30));
        // Every 15 s for the bell, not every 3 s; two Operations for each swept read.
        expect(counts['/v1/board'], lessThanOrEqualTo(2), reason: '$counts');
        expect(counts['/v1/stations'], lessThanOrEqualTo(2), reason: '$counts');
        expect(counts['/v1/missions'], isNull, reason: 'nothing here shows it');
        expect(counts['/v1/user/profile'], isNull, reason: 'not every refresh');
        board.dispose();
      },
    );

    testWidgets('Missions reads its two sources live', (tester) async {
      final counts = <String, int>{};
      final board = await running(tester, counts, OoPage.missions);
      await run(tester, const Duration(seconds: 30));
      expect(counts['/v1/board'], greaterThanOrEqualTo(9), reason: '$counts');
      expect(
        counts['/v1/missions'],
        greaterThanOrEqualTo(9),
        reason: '$counts',
      );
      expect(counts['/v1/stations'], lessThanOrEqualTo(2), reason: '$counts');
      board.dispose();
    });

    testWidgets('the profile is read at its own pace', (tester) async {
      final counts = <String, int>{};
      final board = await running(tester, counts, OoPage.overview);
      await run(tester, const Duration(seconds: 125));
      expect(
        counts['/v1/user/profile'],
        inInclusiveRange(2, 3),
        reason: '$counts',
      );
      board.dispose();
    });

    testWidgets('a page\'s own timer waits while the tab is hidden', (
      tester,
    ) async {
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final counts = <String, int>{};
      final board = await running(tester, counts, OoPage.overview);
      await tester.pumpWidget(app(board));
      await run(tester, const Duration(seconds: 21));
      const timeline = '/v1/user/operations/G/timeline';
      expect(counts[timeline], greaterThanOrEqualTo(2), reason: '$counts');
      counts.clear();
      board.setVisible(false);
      await run(tester, const Duration(seconds: 30));
      expect(counts[timeline], isNull, reason: '$counts');
      await tester.pumpWidget(const SizedBox());
      board.dispose();
    });

    testWidgets('opening a page reads what it shows at once', (tester) async {
      final counts = <String, int>{};
      final board = await running(tester, counts, OoPage.audit);
      await run(tester, const Duration(seconds: 7));
      expect(counts['/v1/missions'], isNull);
      board.showPage(OoPage.missions);
      await tester.pump();
      expect(counts['/v1/missions'], 1);
      board.dispose();
    });

    test('the page decides what is live', () {
      expect(readsOf(OoPage.missions).live, contains(BoardRead.missions));
      expect(readsOf(OoPage.audit).live, isEmpty);
      for (final page in OoPage.values) {
        expect(
          {...readsOf(page).live, ...readsOf(page).background},
          containsAll(const {
            BoardRead.board,
            BoardRead.stations,
            BoardRead.heldBriefs,
            BoardRead.proposals,
            BoardRead.greenInk,
          }),
          reason: 'the bell on $page',
        );
      }
    });

    testWidgets('the app reports a hidden tab to the board', (tester) async {
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: gateway({}),
      );
      await tester.pumpWidget(app(board));
      tester.binding
        ..handleAppLifecycleStateChanged(AppLifecycleState.resumed)
        ..handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      expect(board.visible, isTrue, reason: 'a window behind another is shown');
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      expect(board.visible, isFalse);
      tester.binding
        ..handleAppLifecycleStateChanged(AppLifecycleState.inactive)
        ..handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      expect(board.visible, isTrue);
      await tester.pumpWidget(const SizedBox());
      board.dispose();
    });

    test('ai6.cfg sets the new refreshes', () {
      final config = OoConfig.parse('[oo.refresh]\nprofile = 30\nhidden = 0\n');
      expect(config.limits.refresh.profile, const Duration(seconds: 30));
      expect(config.limits.refresh.hidden, isNull);
      expect(const RefreshSettings().hidden, RefreshSettings.defaultHidden);
    });
  });
}
