import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/chat_layout.dart';
import 'package:outer_office/assistant/chat_popout.dart';
import 'package:outer_office/shell/oo_freshness.dart' show ooClock;
import 'package:outer_office/shell/runs_page.dart';
import 'package:outer_office/shell/sign_in_page.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'chat_popout_test.dart' show FakePopups;
import 'session_renewal_test.dart'
    show FakeChannel, FakeGateway, close, settle, signedIn;
import 'shell_navigation.dart';

/// The gateway's idle limit, and what the tests count the token's life by.
const _idle = Duration(minutes: 30);

/// `renew_before` (the default, ten minutes): the token's renewal falls this far ahead of its
/// end, which with no limit but the idle one is also the idle deadline.
const _before = Duration(minutes: 10);

final _dialog = find.text('Stay signed in?');
final _wait = _idle - _before;

/// A gateway whose tokens last [_idle] after each use and which has no fixed cut.
FakeGateway _gateway() => FakeGateway(idle: _idle, cut: false);

/// Runs the clock to [_wait] after the last renewal and lets the answer land.
Future<void> _toDue(WidgetTester tester) async {
  await tester.pump(_wait);
  await settle(tester);
}

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  testWidgets('input renews the sign-in with no question, again and again', (
    tester,
  ) async {
    final gateway = _gateway();
    final board = await signedIn(tester, gateway);

    // Signing in is the first input.
    await tester.pump(_wait - const Duration(seconds: 1));
    expect(gateway.renewals, isEmpty, reason: 'not before it is due');
    await tester.pump(const Duration(seconds: 2));
    await settle(tester);
    expect(gateway.renewals, hasLength(1));
    expect(_dialog, findsNothing);

    await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
    await _toDue(tester);
    expect(gateway.renewals, hasLength(2), reason: 'a key counts');
    expect(_dialog, findsNothing);

    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(300, 300));
    await mouse.moveTo(const Offset(310, 310));
    await _toDue(tester);
    expect(gateway.renewals, hasLength(3), reason: 'a pointer counts');
    expect(_dialog, findsNothing);
    expect(board.accessToken, 'token-1');
    await close(tester, board);
  });

  testWidgets('with no input the question comes renew_before ahead of the idle '
      'deadline, and nothing is renewed', (tester) async {
    final gateway = _gateway();
    final board = await signedIn(tester, gateway);
    await tester.pump(_wait + const Duration(seconds: 1));
    await settle(tester);
    expect(gateway.renewals, hasLength(1));

    // The renewal moved the idle deadline to 30 minutes after it.
    await tester.pump(_wait - const Duration(seconds: 3));
    expect(_dialog, findsNothing);
    await tester.pump(const Duration(seconds: 2));
    await tester.pump();
    expect(_dialog, findsOneWidget);
    expect(find.textContaining('signed out in 10:00'), findsOneWidget);
    expect(gateway.renewals, hasLength(1), reason: 'asked, not renewed');
    expect(board.accessToken, 'token-1');

    await tester.pump(const Duration(seconds: 1));
    expect(find.textContaining('signed out in 9:59'), findsOneWidget);
    await close(tester, board);
  });

  testWidgets('Stay signed in renews, closes the question and starts the idle '
      'time again', (tester) async {
    final gateway = _gateway();
    final board = await signedIn(tester, gateway);
    await _toDue(tester);
    await tester.pump(_wait + const Duration(seconds: 1));
    expect(_dialog, findsOneWidget);

    await tester.tap(find.text('Stay signed in'));
    await settle(tester);
    expect(gateway.renewals, hasLength(2));
    expect(_dialog, findsNothing);
    expect(board.accessToken, 'token-1');
    expect(board.idleEnds, isNull);

    // Idle again, it is asked again a full cycle later; it did not sign out at the old end.
    await tester.pump(_wait - const Duration(seconds: 1));
    expect(_dialog, findsNothing);
    await tester.pump(const Duration(seconds: 2));
    await tester.pump();
    expect(_dialog, findsOneWidget);
    expect(gateway.renewals, hasLength(2));
    await close(tester, board);
  });

  testWidgets('letting the countdown run out signs out, tells the gateway and '
      'keeps the route for the next sign-in', (tester) async {
    final gateway = _gateway();
    final board = await signedIn(tester, gateway);
    await openPage(tester, 'Runs');
    expect(find.byType(RunsPage), findsOneWidget);

    await _toDue(tester);
    await tester.pump(_wait + const Duration(seconds: 1));
    expect(_dialog, findsOneWidget);

    // The deadline is the board's own; pumps before it carry a little drift from the page.
    final left = board.idleEnds!.difference(ooClock());
    await tester.pump(left - const Duration(seconds: 1));
    expect(_dialog, findsOneWidget, reason: 'not before the deadline');
    expect(board.accessToken, 'token-1');

    await tester.pump(const Duration(seconds: 2));
    await settle(tester);
    expect(_dialog, findsNothing);
    expect(board.accessToken, isNull);
    expect(find.byType(SignInPage), findsOneWidget);
    expect(
      board.loginError,
      'You were signed out because you were idle. Sign in again.',
    );
    expect(gateway.signOuts, ['Bearer token-1']);
    expect(gateway.renewals, hasLength(1), reason: 'nothing renewed it');

    await board.login('ana', 'secret');
    await settle(tester);
    expect(board.accessToken, 'token-2');
    expect(find.byType(RunsPage), findsOneWidget);
    await close(tester, board);
  });

  testWidgets(
    "a pop-out's input keeps the opener signed in, once per renewal",
    (tester) async {
      final gateway = _gateway();
      final board = await signedIn(tester, gateway);
      final popups = FakePopups();
      final chat = MoneypennyChat(
        controller: AssistantController(token: 't', connect: FakeChannel.new),
        preferences: ChatPreferences(SharedPreferences.getInstance()),
        windowOutlivesPage: false,
        openWindow: popupChatWindowOpener(
          popups,
          onActivity: board.markActive,
          renewals: board.renewals,
        ),
      );
      addTearDown(chat.dispose);
      late BuildContext context;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (c) {
              context = c;
              return const SizedBox();
            },
          ),
        ),
      );
      chat.open(focus: false);
      chat.togglePopOut();
      chat.syncWindow(context);
      final link = popups.links.single;

      await _toDue(tester);
      expect(gateway.renewals, hasLength(1));
      expect(
        link.sent.where((m) => m['t'] == 'renewed'),
        hasLength(1),
        reason: 'the pop-out is told it may report again',
      );

      link.hear({'t': 'active'});
      await _toDue(tester);
      expect(gateway.renewals, hasLength(2), reason: 'the pop-out was used');
      expect(board.idleEnds, isNull);

      await tester.pump(_wait);
      expect(board.idleEnds, isNotNull, reason: 'nobody used anything');
      expect(gateway.renewals, hasLength(2));
      await close(tester, board);
    },
  );

  test('a pop-out reports input once until its opener says it renewed', () {
    final popups = FakePopups();
    final session = PopoutSession(popups, person: 'ana');
    addTearDown(session.dispose);
    final link = popups.links.single;
    int reports() => link.sent.where((m) => m['t'] == 'active').length;

    session.activity();
    session.activity();
    expect(reports(), 1);
    link.hear({'t': 'renewed'});
    session.activity();
    expect(reports(), 2);
  });

  testWidgets('a fixed end still shows the end-of-sign-in message, not the '
      'question', (tester) async {
    final gateway = FakeGateway(
      idle: _idle,
      session: const Duration(minutes: 40),
    );
    final board = await signedIn(tester, gateway);
    await tester.pump(_wait + const Duration(seconds: 1));
    await settle(tester);
    expect(gateway.renewals, hasLength(1));

    // The renewal reached the cap, 40 minutes in: ten minutes before it, the message.
    await tester.pump(const Duration(minutes: 9, seconds: 58));
    expect(find.textContaining('Your sign-in ends at'), findsNothing);
    await tester.pump(const Duration(seconds: 2));
    await settle(tester);
    expect(find.textContaining('Your sign-in ends at'), findsOneWidget);
    expect(_dialog, findsNothing);
    await close(tester, board);
  });
}
