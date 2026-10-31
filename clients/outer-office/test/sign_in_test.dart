import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/oo_freshness.dart';
import 'package:outer_office/shell/sign_in_page.dart';
import 'package:outer_office/shell/top_bar.dart';
import 'package:outer_office/theme/oo_backdrop.dart';
import 'package:outer_office/theme/oo_decor.dart';
import 'package:outer_office/theme/oo_tokens.dart';

import 'widget_test.dart' show userProfileResponse;

const _json = {'content-type': 'application/json'};

/// The API behind the page, at the page's own origin: every request is kept, and [login] answers
/// `POST /v1/auth/login` until a test changes it.
class Api {
  Api({this.methods = const ['password']});

  /// What `GET /v1/auth/methods` lists; `null` when it answers 404, as a proxy that does not
  /// pass the route would.
  final List<String>? methods;
  final requests = <http.Request>[];

  /// What `GET /v1/board` answers.
  int boardStatus = 200;

  http.Response Function() login = () =>
      http.Response(jsonEncode({'token': 'user-token'}), 201, headers: _json);

  Future<http.Response> handle(http.Request request) async {
    requests.add(request);
    final path = request.url.path;
    if (path == '/v1/auth/methods') {
      return methods == null
          ? http.Response('not found', 404)
          : http.Response(
              jsonEncode({'methods': methods}),
              200,
              headers: _json,
            );
    }
    if (path == '/v1/auth/login') return login();
    if (path == '/v1/auth/logout') return http.Response('', 204);
    if (path == '/v1/board') {
      return http.Response('[]', boardStatus, headers: _json);
    }
    if (path == '/v1/user/profile') {
      return http.Response(userProfileResponse(), 200, headers: _json);
    }
    if (path == '/v1/stations') {
      return http.Response(
        jsonEncode({'headquarters_station': null, 'stations': []}),
        200,
        headers: _json,
      );
    }
    return http.Response('[]', 200, headers: _json);
  }

  http.Request? sent(String path) =>
      requests.where((request) => request.url.path == path).firstOrNull;
}

/// The app at `size`, on the sign-in page.
Future<MissionBoard> open(
  WidgetTester tester,
  Api api,
  Size size, {
  String themeId = 'night-ops',
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://ai6.example'),
    pollInterval: null,
    themeId: themeId,
    client: MockClient(api.handle),
  );
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  return board;
}

Future<void> tryToSignIn(
  WidgetTester tester,
  String name,
  String secret,
) async {
  await tester.enterText(find.byType(TextField).at(0), name);
  await tester.enterText(find.byType(TextField).at(1), secret);
  await tester.pump();
  await tester.tap(find.text('Continue'));
  await tester.pumpAndSettle();
}

FilledButton continueButton(WidgetTester tester) => tester.widget<FilledButton>(
  find.ancestor(of: find.text('Continue'), matching: find.byType(FilledButton)),
);

String typed(WidgetTester tester, int field) =>
    tester.widget<TextField>(find.byType(TextField).at(field)).controller!.text;

const desktop = Size(1400, 900);
const phone = Size(390, 844);
const smallPhone = Size(320, 480);

void main() {
  for (final (name, size) in [
    ('desktop', desktop),
    ('phone', phone),
    ('a small phone', smallPhone),
  ]) {
    group('at $name width', () {
      testWidgets('the form shows the wordmark over the wallpaper', (
        tester,
      ) async {
        final api = Api();
        await open(tester, api, size);
        expect(find.byType(SignInPage), findsOneWidget);
        expect(find.byType(OoWordmark), findsOneWidget);
        expect(find.byType(OoBackdrop), findsOneWidget);
        expect(find.text('Sign in'), findsOneWidget);
        expect(find.text('Account'), findsOneWidget);
        expect(find.text('Password'), findsOneWidget);
        expect(continueButton(tester).onPressed, isNull);
        final password = tester.widget<TextField>(find.byType(TextField).at(1));
        expect(password.obscureText, isTrue);
        expect(tester.takeException(), isNull);
        // Nothing is tried until the person asks: a try counts against the login limit.
        expect(api.sent('/v1/auth/login'), isNull);
      });

      testWidgets('signing in sends the name and password to the page\'s own '
          'origin and opens Overview', (tester) async {
        final api = Api();
        final board = await open(tester, api, size);
        await tryToSignIn(tester, ' operator ', 'hunter2');
        final login = api.sent('/v1/auth/login')!;
        expect(login.url.origin, 'https://ai6.example');
        expect(jsonDecode(login.body), {
          'name': 'operator',
          'secret': 'hunter2',
        });
        expect(login.headers.containsKey('authorization'), isFalse);
        expect(board.accessToken, 'user-token');
        expect(find.byType(SignInPage), findsNothing);
        expect(find.text('Command overview'), findsWidgets);
        expect(tester.takeException(), isNull);
      });

      testWidgets('a wrong password says so, empties the field and keeps no '
          'token', (tester) async {
        final api = Api()..login = () => http.Response('unauthorized', 401);
        final board = await open(tester, api, size);
        await tryToSignIn(tester, 'operator', 'wrong');
        expect(
          find.text('That account and password did not work.'),
          findsOneWidget,
        );
        expect(board.accessToken, isNull);
        expect(typed(tester, 0), 'operator');
        expect(typed(tester, 1), isEmpty);
        expect(continueButton(tester).onPressed, isNull);
        expect(tester.takeException(), isNull);
      });

      testWidgets(
        'the login limit\'s 429 counts its Retry-After down and holds '
        'Continue until then',
        (tester) async {
          final clock = _Clock();
          ooClock = clock.now;
          addTearDown(() => ooClock = DateTime.now);
          final api = Api()
            ..login = () =>
                http.Response('slow down', 429, headers: {'retry-after': '2'});
          await open(tester, api, size);
          await tester.enterText(find.byType(TextField).at(0), 'operator');
          await tester.enterText(find.byType(TextField).at(1), 'wrong');
          await tester.pump();
          await tester.tap(find.text('Continue'));
          await tester.pump();
          await tester.pump();
          expect(
            find.text('Too many sign-in tries. Try again in 2 seconds.'),
            findsOneWidget,
          );

          clock.at = clock.at.add(const Duration(seconds: 1));
          await tester.pump(const Duration(seconds: 1));
          expect(
            find.text('Too many sign-in tries. Try again in 1 second.'),
            findsOneWidget,
          );
          await tester.enterText(find.byType(TextField).at(1), 'right');
          await tester.pump();
          expect(
            continueButton(tester).onPressed,
            isNull,
            reason: 'still held',
          );

          clock.at = clock.at.add(const Duration(seconds: 1));
          await tester.pump(const Duration(seconds: 1));
          await tester.pump();
          expect(find.textContaining('Too many sign-in tries'), findsNothing);
          expect(continueButton(tester).onPressed, isNotNull);
          expect(tester.takeException(), isNull);
        },
      );
    });
  }

  testWidgets('Enter in the password field signs in, and the fields name '
      'themselves to a password manager', (tester) async {
    final api = Api();
    final board = await open(tester, api, desktop);
    final fields = tester
        .widgetList<TextField>(find.byType(TextField))
        .toList();
    expect(fields[0].autofillHints, [AutofillHints.username]);
    expect(fields[1].autofillHints, [AutofillHints.password]);
    expect(find.byType(AutofillGroup), findsOneWidget);
    expect(
      fields.where((field) => field.autofocus),
      isEmpty,
      reason: 'a focused field is not connected for a screen reader on the web',
    );
    await tester.enterText(find.byType(TextField).at(0), 'operator');
    await tester.enterText(find.byType(TextField).at(1), 'hunter2');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(board.accessToken, 'user-token');
  });

  testWidgets('a 429 with no Retry-After still says what happened', (
    tester,
  ) async {
    final api = Api()..login = () => http.Response('slow down', 429);
    await open(tester, api, desktop);
    await tryToSignIn(tester, 'operator', 'wrong');
    expect(
      find.text('Too many sign-in tries. Wait a while and try again.'),
      findsOneWidget,
    );
  });

  testWidgets('a refusal that is not about the person says who to ask', (
    tester,
  ) async {
    final api = Api();
    await open(tester, api, desktop);
    for (final (status, says) in [
      (403, 'public origin and reverse proxy'),
      (500, 'could not check the sign-in'),
      (502, 'could not check the sign-in'),
      (404, 'refused the sign-in (HTTP 404)'),
    ]) {
      api.login = () => http.Response('no', status);
      await tryToSignIn(tester, 'operator', 'secret');
      expect(
        find.byKey(const ValueKey('sign-in-error')),
        findsOneWidget,
        reason: '$status',
      );
      expect(find.textContaining(says), findsOneWidget, reason: '$status');
    }
  });

  testWidgets('a gateway that cannot be reached is said to be', (tester) async {
    final api = Api()
      ..login = () => throw http.ClientException('connection refused');
    await open(tester, api, desktop);
    await tryToSignIn(tester, 'operator', 'secret');
    expect(
      find.text('Could not connect to Universal Exports.'),
      findsOneWidget,
    );
  });

  testWidgets('signing out revokes the token and shows an empty form', (
    tester,
  ) async {
    final api = Api();
    final board = await open(tester, api, desktop);
    await tryToSignIn(tester, 'operator', 'hunter2');
    expect(board.accessToken, 'user-token');

    await tester.tap(find.byType(UserChip));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Sign out'));
    await tester.pumpAndSettle();

    expect(board.accessToken, isNull);
    expect(board.userSession, isNull);
    final logout = api.sent('/v1/auth/logout')!;
    expect(logout.method, 'POST');
    expect(logout.headers['authorization'], 'Bearer user-token');
    expect(find.byType(SignInPage), findsOneWidget);
    expect(typed(tester, 0), isEmpty);
    expect(typed(tester, 1), isEmpty);
  });

  testWidgets('an ended sign-in shows the form with the reason', (
    tester,
  ) async {
    final api = Api();
    final board = await open(tester, api, desktop);
    await tryToSignIn(tester, 'operator', 'hunter2');
    api.boardStatus = 401;
    await board.refresh();
    await tester.pumpAndSettle();
    expect(find.byType(SignInPage), findsOneWidget);
    expect(find.text('Your sign-in ended. Sign in again.'), findsOneWidget);
  });

  group('what the gateway accepts', () {
    testWidgets('a proxy alone is a button and no form, and posts no secret', (
      tester,
    ) async {
      final api = Api(methods: ['proxy']);
      final board = await open(tester, api, desktop);
      expect(find.byType(TextField), findsNothing);
      await tester.tap(find.text('Continue with organization sign-in'));
      await tester.pumpAndSettle();
      expect(jsonDecode(api.sent('/v1/auth/login')!.body), isEmpty);
      expect(board.accessToken, 'user-token');
    });

    testWidgets('a refused proxy sign-in points at the proxy', (tester) async {
      final api = Api(methods: ['proxy'])
        ..login = () => http.Response('no', 401);
      await open(tester, api, phone);
      await tester.tap(find.text('Continue with organization sign-in'));
      await tester.pumpAndSettle();
      expect(
        find.textContaining('authenticated organization proxy'),
        findsOneWidget,
      );
    });

    testWidgets('both are the form with the proxy under it', (tester) async {
      final api = Api(methods: ['password', 'proxy']);
      await open(tester, api, phone);
      expect(find.byType(TextField), findsNWidgets(2));
      expect(find.text('Continue'), findsOneWidget);
      expect(
        find.widgetWithText(
          OutlinedButton,
          'Continue with organization sign-in',
        ),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('neither says so and asks for nothing', (tester) async {
      final api = Api(methods: []);
      await open(tester, api, desktop);
      expect(find.byType(TextField), findsNothing);
      expect(find.byType(FilledButton), findsNothing);
      expect(find.textContaining('no sign-in set up'), findsOneWidget);
    });

    testWidgets('a gateway that does not say gets the form', (tester) async {
      final api = Api(methods: null);
      await open(tester, api, desktop);
      expect(find.byType(TextField), findsNWidgets(2));
      expect(find.text('Continue'), findsOneWidget);
    });
  });

  for (final theme in OoThemeId.values) {
    testWidgets('it draws in ${theme.label} at both widths', (tester) async {
      for (final size in [desktop, phone]) {
        await open(tester, Api(), size, themeId: theme.id);
        expect(find.byType(OoWordmark), findsOneWidget);
        expect(tester.takeException(), isNull);
      }
    });
  }
}

class _Clock {
  DateTime at = DateTime(2026, 10, 31, 12);
  DateTime now() => at;
}
