import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/factors/second_factor_section.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/theme/oo_theme.dart';
import 'package:qr/qr.dart';

import 'sign_in_test.dart' show Api, desktop, open, tryToSignIn;

const _json = {'content-type': 'application/json'};
const _uri =
    'otpauth://totp/AI6:ada?secret=JBSWY3DPEHPK3PXP&issuer=AI6&digits=6&period=30';

http.Response _reply(int status, Object? body) =>
    http.Response(jsonEncode(body), status, headers: _json);

List<Map<String, dynamic>> _posted(Api api) => [
  for (final request in api.requests)
    if (request.url.path == '/v1/auth/login' && request.method == 'POST')
      jsonDecode(request.body) as Map<String, dynamic>,
];

void main() {
  group('the code step of a sign-in', () {
    testWidgets('password then a one-time code signs in', (tester) async {
      final api = Api();
      final answers = [
        _reply(202, {
          'pending': 'opaque-1',
          'methods': ['totp', 'recovery_code'],
          'expires_in': 300,
        }),
        _reply(201, {'token': 'user-token'}),
      ];
      api.login = () => answers.removeAt(0);
      final board = await open(tester, api, desktop);
      await tryToSignIn(tester, 'ada', 'hunter2');
      expect(board.pendingSignIn, 'opaque-1');
      expect(find.text('Code'), findsOneWidget);
      expect(find.text('Password'), findsNothing);
      await tester.enterText(find.byType(TextField).first, '123456');
      await tester.pump();
      await tester.tap(find.text('Continue'));
      await tester.pumpAndSettle();
      expect(board.accessToken, 'user-token');
      expect(board.pendingSignIn, isNull);
      expect(_posted(api), [
        {'name': 'ada', 'secret': 'hunter2'},
        {'pending': 'opaque-1', 'method': 'totp', 'secret': '123456'},
      ]);
    });

    testWidgets('a wrong code says so and stays on the code step', (
      tester,
    ) async {
      final api = Api();
      var calls = 0;
      api.login = () {
        calls++;
        if (calls == 1) {
          return _reply(202, {
            'pending': 'opaque-1',
            'methods': ['totp', 'recovery_code'],
            'expires_in': 300,
          });
        }
        return http.Response('', 401);
      };
      final board = await open(tester, api, desktop);
      await tryToSignIn(tester, 'ada', 'hunter2');
      await tester.enterText(find.byType(TextField).first, '000000');
      await tester.pump();
      await tester.tap(find.text('Continue'));
      await tester.pumpAndSettle();
      expect(board.accessToken, isNull);
      expect(board.pendingSignIn, 'opaque-1');
      expect(find.byKey(const ValueKey('sign-in-error')), findsOneWidget);
      expect(find.textContaining('That code did not work'), findsOneWidget);
      expect(
        tester.widget<TextField>(find.byType(TextField).first).controller!.text,
        isEmpty,
      );
      // A recovery code is offered, and sent as such.
      await tester.tap(find.text('Use a recovery code'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField).first, 'ABCDE-FGHJK');
      await tester.pump();
      await tester.tap(find.text('Continue'));
      await tester.pumpAndSettle();
      expect(_posted(api).last, {
        'pending': 'opaque-1',
        'method': 'recovery_code',
        'secret': 'ABCDE-FGHJK',
      });
      // Starting again goes back to the password.
      await tester.tap(find.text('Start again'));
      await tester.pumpAndSettle();
      expect(board.pendingSignIn, isNull);
      expect(find.text('Password'), findsOneWidget);
    });

    testWidgets('no recovery toggle when the gateway does not offer it', (
      tester,
    ) async {
      final api = Api();
      api.login = () => _reply(202, {
        'pending': 'p',
        'methods': ['totp'],
        'expires_in': 300,
      });
      await open(tester, api, desktop);
      await tryToSignIn(tester, 'ada', 'hunter2');
      expect(find.text('Use a recovery code'), findsNothing);
    });
  });

  group('the Second factor section', () {
    Future<(MissionBoard, List<http.Request>)> section(
      WidgetTester tester, {
      bool mayEnrol = true,
      bool configured = true,
    }) async {
      final seen = <http.Request>[];
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
        client: MockClient((request) async {
          seen.add(request);
          switch ((request.method, request.url.path)) {
            case ('GET', '/v1/user/factors'):
              return _reply(200, {
                'configured': configured,
                'enrolled': false,
                'enrolling': false,
                'may_enrol': mayEnrol,
              });
            case ('POST', '/v1/user/factors/totp'):
              return _reply(200, {
                'uri': _uri,
                'recovery_codes': ['AAAAA-BBBBB', 'CCCCC-DDDDD'],
              });
            case ('POST', '/v1/user/factors/totp/confirm'):
              final code = (jsonDecode(request.body) as Map)['code'];
              return code == '123456'
                  ? _reply(200, {'enrolled': true})
                  : _reply(400, {'error': 'That code is not right.'});
          }
          return _reply(200, []);
        }),
      )..accessToken = 'token';
      addTearDown(board.dispose);
      tester.view.physicalSize = const Size(800, 1600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(
        OoMaterialApp(
          themeId: 'light',
          backdrop: false,
          title: 'test',
          home: Scaffold(
            body: SingleChildScrollView(
              child: SecondFactorSection(board: board),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      return (board, seen);
    }

    testWidgets('shows the link as a QR code and the recovery codes once', (
      tester,
    ) async {
      final (_, seen) = await section(tester);
      expect(find.text('Second factor'), findsOneWidget);
      await tester.tap(find.text('Set up'));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('second-factor-qr')), findsOneWidget);
      expect(find.textContaining('JBSWY3DPEHPK3PXP'), findsOneWidget);
      expect(find.text('AAAAA-BBBBB'), findsOneWidget);
      expect(find.text('CCCCC-DDDDD'), findsOneWidget);
      // A wrong code keeps the dialog, in the gateway's words.
      await tester.enterText(find.byType(TextField), '000000');
      await tester.tap(find.text('Confirm'));
      await tester.pumpAndSettle();
      expect(find.text('That code is not right.'), findsOneWidget);
      await tester.enterText(find.byType(TextField), '123456');
      await tester.tap(find.text('Confirm'));
      await tester.pumpAndSettle();
      expect(find.text('AAAAA-BBBBB'), findsNothing);
      expect(find.byKey(const ValueKey('second-factor-qr')), findsNothing);
      expect(
        seen.where((r) => r.url.path == '/v1/user/factors/totp').length,
        1,
        reason: 'beginning is asked for once',
      );
    });

    testWidgets('says when the policy does not let this sign-in enrol', (
      tester,
    ) async {
      await section(tester, mayEnrol: false);
      expect(find.textContaining('does not let this sign-in'), findsOneWidget);
      expect(find.text('Set up'), findsNothing);
    });

    testWidgets('shows nothing to set up where no provider is configured', (
      tester,
    ) async {
      await section(tester, configured: false);
      expect(find.text('Set up'), findsNothing);
    });
  });

  test('the qr package makes a matrix for a provisioning link', () {
    final image = QrImage(
      QrCode.fromData(data: _uri, errorCorrectLevel: QrErrorCorrectLevel.M),
    );
    expect(image.moduleCount, greaterThan(20));
    expect(
      List.generate(
        image.moduleCount,
        (x) => image.isDark(0, x),
      ).where((dark) => dark),
      isNotEmpty,
    );
  });
}
