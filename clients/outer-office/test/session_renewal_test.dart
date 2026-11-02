import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/settings/oo_config.dart';
import 'package:outer_office/shell/oo_freshness.dart';
import 'package:outer_office/shell/sign_in_page.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'moneypenny_off_test.dart' show app, profile;

const _json = {'content-type': 'application/json'};
const _hour = Duration(hours: 1);
const _renewBefore = RefreshSettings.defaultRenewBefore;

double _unix(DateTime time) => time.millisecondsSinceEpoch / 1000;

/// Her socket: says nothing until the test does, and keeps what the app sent.
class FakeChannel implements AssistantChannel {
  final _incoming = StreamController<String>();
  final sent = <String>[];

  void ready({String conversation = 'conversation-1'}) => _incoming.add(
    jsonEncode({'type': 'ready', 'conversation': conversation, 'entries': []}),
  );

  /// The first frame it was sent.
  Map<String, Object?> get hello => (jsonDecode(sent.first) as Map).cast();

  @override
  Stream<String> get incoming => _incoming.stream;

  @override
  void send(String frame) => sent.add(frame);

  @override
  Future<void> close() async {
    if (!_incoming.isClosed) await _incoming.close();
  }
}

/// A gateway whose clock is the test's: tokens last an hour from when they are asked for, up to
/// [session] from sign-in.
class FakeGateway {
  FakeGateway({
    this.session = const Duration(hours: 12),
    this.clockAhead = Duration.zero,
  });

  final Duration session;

  /// How far the gateway's clock is ahead of the app's.
  final Duration clockAhead;
  final renewals = <String>[];
  var person = 'ana';
  var moneypenny = false;
  var signIns = 0;

  /// The browser holds a session cookie, as far as this gateway can tell.
  var hasCookie = true;
  var resumes = 0;
  var signOuts = <String>[];

  /// Held until the test says, so it can look at the app while it waits.
  Completer<void>? resumeHeld;

  /// Answers a resume in place of the gateway's, when set.
  http.Response Function()? resumeAnswer;

  /// Answers a renewal in place of the gateway's, when set.
  http.Response Function()? renewAnswer;

  late DateTime _signedInAt;

  Map<String, Object?> _times() {
    final cap = _signedInAt.add(session);
    final wanted = ooClock().add(_hour);
    final until = wanted.isAfter(cap) ? cap : wanted;
    return {
      'expires_at': _unix(until.add(clockAhead)),
      'expires_in': until.difference(ooClock()).inMilliseconds / 1000,
      'ends_at': _unix(cap.add(clockAhead)),
    };
  }

  MockClient get client => MockClient((request) async {
    switch (request.url.path) {
      case '/v1/auth/login':
        signIns++;
        _signedInAt = ooClock();
        return http.Response(
          jsonEncode({'token': 'token-$signIns', ..._times()}),
          201,
          headers: _json,
        );
      case '/v1/auth/resume':
        resumes++;
        await resumeHeld?.future;
        if (resumeAnswer case final answer?) return answer();
        if (!hasCookie) return http.Response('{}', 401, headers: _json);
        return http.Response(
          jsonEncode({'token': 'resumed-$resumes', ..._times()}),
          201,
          headers: _json,
        );
      case '/v1/auth/logout':
        signOuts.add(request.headers['authorization']!);
        hasCookie = false;
        return http.Response('', 204);
      case '/v1/auth/renew':
        renewals.add(request.headers['authorization']!);
        if (renewAnswer case final answer?) return answer();
        return http.Response(jsonEncode(_times()), 200, headers: _json);
      case '/v1/user/profile':
        return http.Response(
          jsonEncode({
            ...profile(moneypenny: moneypenny),
            'person_id': person,
            'name': person,
          }),
          200,
          headers: _json,
        );
      case '/v1/stations':
        return http.Response(
          jsonEncode({'headquarters_station': null, 'stations': []}),
          200,
          headers: _json,
        );
    }
    return http.Response('[]', 200, headers: _json);
  });
}

/// The app signed in to [gateway] with the board's poll off, on the test's clock.
Future<MissionBoard> signedIn(
  WidgetTester tester,
  FakeGateway gateway, {
  AssistantChannel Function()? channel,
  RefreshSettings refresh = const RefreshSettings(),
}) async {
  tester.view.physicalSize = const Size(1400, 1000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final before = ooClock;
  ooClock = () => tester.binding.clock.now();
  addTearDown(() => ooClock = before);
  SharedPreferences.setMockInitialValues({});
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: gateway.client,
    limits: OoLimits(refresh: refresh),
    assistantChannel: channel,
  );
  await tester.pumpWidget(app(board));
  await board.login('ana', 'secret');
  await tester.pump();
  await tester.pump();
  return board;
}

/// The app as a page reload starts it: a new board, no token, asking to resume.
Future<MissionBoard> reloaded(
  WidgetTester tester,
  FakeGateway gateway, {
  AssistantChannel Function()? channel,
}) async {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: gateway.client,
    assistantChannel: channel,
  );
  unawaited(board.resumeSignIn());
  await tester.pumpWidget(app(board));
  return board;
}

Future<void> close(WidgetTester tester, MissionBoard board) async {
  await tester.pumpWidget(const SizedBox());
  await tester.pump(const Duration(seconds: 1));
  board.dispose();
}

/// Lets what an answer started run on.
Future<void> settle(WidgetTester tester) async {
  await tester.pump();
  await tester.pump();
}

void main() {
  testWidgets('renews before expiry and stays signed in', (tester) async {
    final channels = <FakeChannel>[];
    final gateway = FakeGateway()..moneypenny = true;
    final board = await signedIn(
      tester,
      gateway,
      channel: () {
        final channel = FakeChannel();
        channels.add(channel);
        return channel;
      },
    );
    channels.single.ready();
    await settle(tester);
    expect(gateway.renewals, isEmpty);

    await tester.pump(_hour - _renewBefore - const Duration(seconds: 1));
    expect(gateway.renewals, isEmpty, reason: 'not before it is due');

    await tester.pump(const Duration(seconds: 2));
    await settle(tester);
    expect(gateway.renewals, ['Bearer token-1']);
    expect(board.accessToken, 'token-1');
    expect(find.byType(SignInPage), findsNothing);

    await tester.pump(_hour - _renewBefore);
    await settle(tester);
    expect(gateway.renewals, hasLength(2), reason: 'each answer sets the next');
    expect(board.accessToken, 'token-1');
    expect(find.byType(SignInPage), findsNothing);
    expect(channels, hasLength(1), reason: 'the chat keeps its socket');
    await close(tester, board);
  });

  testWidgets('a lifetime shorter than renew_before renews halfway through', (
    tester,
  ) async {
    final gateway = FakeGateway();
    final board = await signedIn(
      tester,
      gateway,
      refresh: const RefreshSettings(renewBefore: Duration(hours: 2)),
    );
    await tester.pump(const Duration(minutes: 29));
    expect(gateway.renewals, isEmpty, reason: 'no asking at once');
    await tester.pump(const Duration(minutes: 2));
    await settle(tester);
    expect(gateway.renewals, hasLength(1));
    await close(tester, board);
  });

  for (final clockAhead in [
    const Duration(hours: 3),
    -const Duration(hours: 3),
  ]) {
    testWidgets('a clock off by $clockAhead still renews once, on time', (
      tester,
    ) async {
      final gateway = FakeGateway(clockAhead: clockAhead);
      final board = await signedIn(tester, gateway);
      await tester.pump(const Duration(minutes: 49));
      expect(gateway.renewals, isEmpty);
      await tester.pump(const Duration(minutes: 2));
      await settle(tester);
      expect(gateway.renewals, hasLength(1));
      await close(tester, board);
    });
  }

  testWidgets('renew_before 0 never renews', (tester) async {
    final gateway = FakeGateway();
    final board = await signedIn(
      tester,
      gateway,
      refresh: const RefreshSettings(renewBefore: null),
    );
    await tester.pump(_hour * 2);
    expect(gateway.renewals, isEmpty);
    await close(tester, board);
  });

  testWidgets('a refused renewal signs out', (tester) async {
    final channels = <FakeChannel>[];
    final gateway = FakeGateway()
      ..moneypenny = true
      ..renewAnswer = () => http.Response('', 401);
    final board = await signedIn(
      tester,
      gateway,
      channel: () {
        final channel = FakeChannel();
        channels.add(channel);
        return channel;
      },
    );
    expect(find.byType(SignInPage), findsNothing);
    await tester.pump(_hour);
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    expect(gateway.renewals, hasLength(1));
    expect(board.accessToken, isNull);
    expect(find.byType(SignInPage), findsOneWidget);
    expect(board.loginError, 'Your sign-in ended. Sign in again.');
    await tester.pump(_hour);
    expect(gateway.renewals, hasLength(1), reason: 'nothing asks again');
    await close(tester, board);
  });

  testWidgets('a failed renewal tries again and leaves the person signed in', (
    tester,
  ) async {
    final gateway = FakeGateway();
    final board = await signedIn(tester, gateway);
    gateway.renewAnswer = () => throw http.ClientException('no route');
    await tester.pump(_hour - _renewBefore);
    await settle(tester);
    expect(gateway.renewals, hasLength(1));
    expect(board.accessToken, 'token-1');

    await tester.pump(RefreshSettings.defaultSessions);
    await settle(tester);
    expect(gateway.renewals, hasLength(2), reason: 'at the board\'s pace');

    gateway.renewAnswer = () => http.Response('busy', 503);
    await tester.pump(RefreshSettings.defaultSessions);
    await settle(tester);
    expect(gateway.renewals, hasLength(3), reason: 'a 503 is no refusal');
    expect(board.accessToken, 'token-1');

    gateway.renewAnswer = null;
    await tester.pump(RefreshSettings.defaultSessions);
    await settle(tester);
    expect(gateway.renewals, hasLength(4));
    await tester.pump(const Duration(minutes: 5));
    expect(gateway.renewals, hasLength(4), reason: 'back to its time');
    await close(tester, board);
  });

  testWidgets('with polling off a failed renewal waits renew_retry', (
    tester,
  ) async {
    final gateway = FakeGateway();
    final board = await signedIn(
      tester,
      gateway,
      refresh: const RefreshSettings(
        sessions: null,
        renewRetry: Duration(seconds: 40),
      ),
    );
    gateway.renewAnswer = () => http.Response('busy', 503);
    await tester.pump(_hour - _renewBefore);
    await settle(tester);
    expect(gateway.renewals, hasLength(1));
    await tester.pump(const Duration(seconds: 39));
    expect(gateway.renewals, hasLength(1));
    await tester.pump(const Duration(seconds: 1));
    await settle(tester);
    expect(gateway.renewals, hasLength(2));
    await close(tester, board);
  });

  testWidgets('warns before the cap, once', (tester) async {
    final gateway = FakeGateway(session: const Duration(minutes: 90));
    final signedInAt = tester.binding.clock.now();
    final board = await signedIn(tester, gateway);
    final strip = find.textContaining('Your sign-in ends at');

    // The renewal at 50 minutes reaches the cap, at 90.
    await tester.pump(const Duration(minutes: 51));
    await settle(tester);
    expect(gateway.renewals, hasLength(1));
    expect(strip, findsNothing);

    await tester.pump(const Duration(minutes: 28));
    expect(strip, findsNothing, reason: 'not until renew_before ahead');

    await tester.pump(const Duration(minutes: 2));
    await settle(tester);
    final cap = signedInAt.add(const Duration(minutes: 90));
    expect(
      find.text(
        'Your sign-in ends at ${ooClockMinute(cap)}. '
        'Sign in again to keep working.',
      ),
      findsOneWidget,
    );
    expect(gateway.renewals, hasLength(1), reason: 'no further renewal');
    expect(board.accessToken, 'token-1');

    await tester.tap(find.byTooltip('Dismiss'));
    await settle(tester);
    expect(strip, findsNothing);
    await tester.pump(const Duration(minutes: 5));
    await settle(tester);
    expect(strip, findsNothing, reason: 'told once');
    await close(tester, board);
  });

  testWidgets('a store that never answers ends in a retry that says hello', (
    tester,
  ) async {
    final channels = <FakeChannel>[];
    final controller = AssistantController(
      token: 'token-1',
      connect: () {
        final channel = FakeChannel();
        channels.add(channel);
        return channel;
      },
      lastConversation: Completer<String?>().future,
    );
    addTearDown(controller.dispose);
    controller.open();
    await tester.pump();
    expect(channels.single.sent, isEmpty, reason: 'it waits for the store');
    await tester.pump(ChatSettings.defaultConnectTimeout);
    expect(controller.problem, 'Moneypenny did not answer.');
    controller.retry();
    expect(channels.last.hello['token'], 'token-1');
    expect(channels.last.hello.containsKey('conversation'), isFalse);
    await tester.pump(ChatSettings.defaultConnectTimeout);
  });

  testWidgets(
    'a reload resumes the sign-in with no password and its last conversation',
    (tester) async {
      final channels = <FakeChannel>[];
      AssistantChannel channel() {
        final channel = FakeChannel();
        channels.add(channel);
        return channel;
      }

      final gateway = FakeGateway()..moneypenny = true;
      final first = await signedIn(tester, gateway, channel: channel);
      await settle(tester);
      expect(gateway.resumes, 0, reason: 'signing in does not ask');
      channels.last.ready();
      await settle(tester);
      await close(tester, first);

      gateway.resumeHeld = Completer();
      final board = await reloaded(tester, gateway, channel: channel);
      await tester.pump();
      expect(board.resuming, isTrue);
      expect(
        find.byType(SignInPage),
        findsNothing,
        reason: 'no flash of the sign-in page while it asks',
      );
      gateway.resumeHeld!.complete();
      await settle(tester);
      await settle(tester);

      expect(gateway.signIns, 1, reason: 'no new sign-in');
      expect(board.accessToken, 'resumed-1');
      expect(board.resuming, isFalse);
      expect(board.loginError, isNull);
      expect(find.byType(SignInPage), findsNothing);
      expect(channels.last.hello['token'], 'resumed-1');
      expect(channels.last.hello['conversation'], 'conversation-1');

      // It is a sign-in like any other: renewed on time, with the token it was given.
      await tester.pump(_hour);
      await settle(tester);
      expect(gateway.renewals, ['Bearer resumed-1']);
      await close(tester, board);
    },
  );

  testWidgets('no session cookie shows the sign-in page and no error', (
    tester,
  ) async {
    final gateway = FakeGateway()..hasCookie = false;
    final board = await reloaded(tester, gateway);
    await settle(tester);
    expect(gateway.resumes, 1);
    expect(board.accessToken, isNull);
    expect(board.resuming, isFalse);
    expect(board.loginError, isNull);
    expect(find.byType(SignInPage), findsOneWidget);
    await close(tester, board);
  });

  for (final (what, answer) in [
    ('too many refusals', http.Response('{}', 429, headers: _json)),
    ('another site', http.Response('{}', 403, headers: _json)),
    ('a failing gateway', http.Response('{}', 500, headers: _json)),
    ('an unreadable answer', http.Response('nonsense', 201, headers: _json)),
  ]) {
    testWidgets('$what shows the sign-in page and no error', (tester) async {
      final gateway = FakeGateway()..resumeAnswer = () => answer;
      final board = await reloaded(tester, gateway);
      await settle(tester);
      expect(board.accessToken, isNull);
      expect(board.resuming, isFalse);
      expect(board.loginError, isNull);
      expect(find.byType(SignInPage), findsOneWidget);
      await close(tester, board);
    });
  }

  testWidgets('a gateway out of reach shows the sign-in page and no error', (
    tester,
  ) async {
    final gateway = FakeGateway()
      ..resumeAnswer = () => throw http.ClientException('no route');
    final board = await reloaded(tester, gateway);
    await settle(tester);
    expect(board.accessToken, isNull);
    expect(board.loginError, isNull);
    expect(find.byType(SignInPage), findsOneWidget);
    await close(tester, board);
  });

  testWidgets('signing out after a resume leaves nothing to resume', (
    tester,
  ) async {
    final gateway = FakeGateway();
    final first = await signedIn(tester, gateway);
    await close(tester, first);
    final board = await reloaded(tester, gateway);
    await settle(tester);
    expect(board.accessToken, 'resumed-1');

    await board.logout();
    await settle(tester);
    expect(gateway.signOuts, ['Bearer resumed-1']);
    expect(find.byType(SignInPage), findsOneWidget);
    await close(tester, board);

    final again = await reloaded(tester, gateway);
    await settle(tester);
    expect(again.accessToken, isNull);
    expect(again.loginError, isNull);
    expect(find.byType(SignInPage), findsOneWidget);
    await close(tester, again);
  });

  testWidgets('a new sign-in says hello with the stored conversation', (
    tester,
  ) async {
    final channels = <FakeChannel>[];
    final gateway = FakeGateway()..moneypenny = true;
    final board = await signedIn(
      tester,
      gateway,
      channel: () {
        final channel = FakeChannel();
        channels.add(channel);
        return channel;
      },
    );
    await settle(tester);
    expect(channels.last.hello.containsKey('conversation'), isFalse);
    channels.last.ready();
    await settle(tester);

    // A real new sign-in: the same person, a new token.
    await board.logout();
    await settle(tester);
    await board.login('ana', 'secret');
    await settle(tester);
    await settle(tester);
    expect(channels, hasLength(2));
    expect(channels.last.hello['token'], 'token-2');
    expect(channels.last.hello['conversation'], 'conversation-1');

    // That one is gone: the gateway opened another, and that is the one to come back to.
    channels.last.ready(conversation: 'conversation-2');
    await settle(tester);
    await board.logout();
    await settle(tester);
    await board.login('ana', 'secret');
    await settle(tester);
    await settle(tester);
    expect(channels.last.hello['conversation'], 'conversation-2');

    // Someone else on this device has their own.
    await board.logout();
    await settle(tester);
    gateway.person = 'bea';
    await board.login('bea', 'secret');
    await settle(tester);
    await settle(tester);
    expect(channels.last.hello.containsKey('conversation'), isFalse);
    await close(tester, board);
  });
}
