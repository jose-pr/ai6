import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';

import 'sign_in_test.dart' show Api, desktop, open, tryToSignIn;

/// The `TextInput` channel's calls, kept by method name.
List<String> watchTextInput(WidgetTester tester) {
  final calls = <String>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.textInput,
    (call) async {
      calls.add(call.method);
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.textInput,
      null,
    ),
  );
  return calls;
}

void main() {
  testWidgets('the fields carry the hints a password manager reads', (
    tester,
  ) async {
    await open(tester, Api(), desktop);
    final account = tester.widget<TextField>(find.byType(TextField).at(0));
    final password = tester.widget<TextField>(find.byType(TextField).at(1));
    expect(account.autofillHints, [
      AutofillHints.username,
      AutofillHints.email,
    ]);
    expect(account.keyboardType, TextInputType.emailAddress);
    expect(password.autofillHints, [AutofillHints.password]);
    // The web engine writes autocorrect="off" and nothing for suggestions; neither is needed.
    expect(account.autocorrect, isNot(false));
    expect(password.autocorrect, isNot(false));
    expect(find.byType(AutofillGroup), findsOneWidget);
  });

  testWidgets('a sign-in that went through finishes the autofill context', (
    tester,
  ) async {
    final calls = watchTextInput(tester);
    await open(tester, Api(), desktop);
    calls.clear();
    await tryToSignIn(tester, 'operator', 'hunter2');
    expect(calls, contains('TextInput.finishAutofillContext'));
  });

  testWidgets('a refused sign-in does not finish it', (tester) async {
    final calls = watchTextInput(tester);
    final api = Api()..login = () => http.Response('{}', 401);
    await open(tester, api, desktop);
    calls.clear();
    await tryToSignIn(tester, 'operator', 'wrong');
    expect(calls, isNot(contains('TextInput.finishAutofillContext')));
  });
}
