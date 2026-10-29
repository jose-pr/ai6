import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_view.dart'
    show OutsideTextStrip;
import 'package:outer_office/assistant/chat_layout.dart';
import 'package:outer_office/assistant/navigation_card.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/overview_page.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'demo_conversation_test.dart' show askFromOverview;
import 'polish_test.dart' show FakeChannel, atSize;
import 'routes_test.dart' show locations;

/// The board of a person who may read G's Missions and audit log, talking to [channel];
/// every request it sends is kept in [requests].
MissionBoard _board(FakeChannel channel, List<http.BaseRequest> requests) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    assistantChannel: () => channel,
    client: MockClient((request) async {
      requests.add(request);
      return http.Response(
        '[]',
        200,
        headers: {'content-type': 'application/json'},
      );
    }),
  );
  board.accessToken = 'user-token';
  board.loading = false;
  board.userSession = const UserSessionSummary(
    personId: 'ana',
    name: 'ana',
    displayName: 'Ana Ortiz',
    groups: ['operators'],
    expiresAt: 1_900_000_000,
    operations: [
      UserOperationAccess(
        operationId: 'G',
        actions: ['mission.read', 'audit.read'],
      ),
    ],
  );
  board.sessions = const [
    SessionSummary(
      operationId: 'G',
      missionId: 'g-failed',
      sessionId: 's-g-failed',
      state: 'failed',
      sequence: 3,
    ),
    SessionSummary(
      operationId: 'G',
      missionId: 'g-working',
      sessionId: 's-g-working',
      state: 'working',
      sequence: 2,
    ),
  ];
  return board;
}

/// Opens the app for [board] at 1400 x 900, with the chat docked and connected; the web
/// locations the app reports are kept in the returned list.
Future<(MoneypennyChat, List<MethodCall>)> _open(
  WidgetTester tester,
  MissionBoard board,
  FakeChannel channel,
) async {
  await atSize(tester, const Size(1400, 900));
  final reported = <MethodCall>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.navigation,
    (call) async {
      reported.add(call);
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.navigation,
      null,
    ),
  );
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await tester.pumpAndSettle();
  final chat = tester
      .widget<MissionBoardPage>(find.byType(MissionBoardPage))
      .chat!;
  chat.open(to: ChatLayout.docked, focus: false);
  await tester.pumpAndSettle();
  channel.answer({'type': 'ready', 'conversation': 'c-1', 'entries': []});
  await tester.pumpAndSettle();
  return (chat, reported);
}

var _clock = 0;

/// Moneypenny offers [route] with [label], after reading outside text when [exposed].
Future<void> _offer(
  WidgetTester tester,
  FakeChannel channel,
  Map<String, Object?> route, {
  String label = 'Look here',
  bool exposed = false,
}) async {
  final id = 'nav-${++_clock}';
  channel.answer({
    'type': 'entry',
    'entry': {
      'role': 'navigation',
      'text': label,
      'at': _clock.toDouble(),
      'data': {
        'id': id,
        'route': route,
        'label': label,
        if (exposed) 'exposed': true,
      },
    },
  });
  await tester.pumpAndSettle();
}

Finder _card(String id) => find.byKey(ValueKey('navigation-$id'));

Finder _openButton(String id) =>
    find.descendant(of: _card(id), matching: find.text('Open'));

void main() {
  setUp(() {
    _clock = 0;
    SharedPreferences.setMockInitialValues({});
  });

  testWidgets('a card shows the route\'s page and fields, and Open opens it as '
      'a new history entry that only reads', (tester) async {
    final channel = FakeChannel();
    final requests = <http.BaseRequest>[];
    final board = _board(channel, requests);
    addTearDown(board.dispose);
    final (chat, reported) = await _open(tester, board, channel);
    await _offer(tester, channel, {
      'page': 'audit',
      'operation_id': 'G',
    }, label: 'Who changed G');
    final card = _card('nav-1');
    expect(card, findsOneWidget);
    expect(
      find.descendant(of: card, matching: find.text('Audit')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: card, matching: find.text('Operation: G')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: card, matching: find.text('Who changed G')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('opened-by-moneypenny')), findsNothing);
    expect(
      find.byType(OverviewPage),
      findsOneWidget,
      reason: 'nothing opens until the person chooses it',
    );

    requests.clear();
    await tester.tap(_openButton('nav-1'));
    await tester.pumpAndSettle();
    expect(locations(reported).last, (
      '/audit?operation_id=G',
      false,
    ), reason: 'a new history entry, so Back returns');
    expect(find.byKey(const ValueKey('opened-by-moneypenny')), findsOneWidget);
    expect(find.byKey(const ValueKey('filter-operation_id')), findsOneWidget);
    expect(requests, isNotEmpty, reason: 'the Audit page loaded its records');
    expect(requests.map((request) => request.method).toSet(), {
      'GET',
    }, reason: 'following a page only reads');
    expect(chat.layout, ChatLayout.docked);

    // The person's own change of filter is theirs, not hers.
    await tester.tap(
      find.descendant(
        of: find.byKey(const ValueKey('filter-operation_id')),
        matching: find.byTooltip('Remove this filter'),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('opened-by-moneypenny')), findsNothing);
  });

  testWidgets('a route the app does not know is shown as refused, with no '
      'Open', (tester) async {
    final channel = FakeChannel();
    final board = _board(channel, []);
    addTearDown(board.dispose);
    await _open(tester, board, channel);
    await _offer(tester, channel, {'page': 'missions', 'token': 'secret'});
    expect(_openButton('nav-1'), findsNothing);
    expect(find.textContaining('missions has no token'), findsOneWidget);
  });

  testWidgets('with the follow preference on, a clean card opens at once; an '
      'exposed one, or one over a dialog, waits for Open', (tester) async {
    SharedPreferences.setMockInitialValues({'oo.chat.ana.follow': true});
    final channel = FakeChannel();
    final board = _board(channel, []);
    addTearDown(board.dispose);
    final (chat, reported) = await _open(tester, board, channel);
    expect(chat.followAtOnce, isTrue);

    await _offer(tester, channel, {
      'page': 'missions',
      'operation_id': 'G',
      'state': ['failed'],
    });
    expect(locations(reported).last, (
      '/missions?operation_id=G&state=failed',
      false,
    ));
    expect(find.text('Mission g-failed'), findsOneWidget);
    expect(find.text('Mission g-working'), findsNothing);
    expect(find.byKey(const ValueKey('opened-by-moneypenny')), findsOneWidget);

    // Offered after reading outside text: the card says so and waits.
    await _offer(tester, channel, {
      'page': 'sessions',
      'operation_id': 'G',
    }, exposed: true);
    expect(
      locations(reported).last.$1,
      '/missions?operation_id=G&state=failed',
    );
    expect(
      find.descendant(
        of: _card('nav-2'),
        matching: find.text(OutsideTextStrip.message),
      ),
      findsOneWidget,
    );
    await tester.tap(_openButton('nav-2'));
    await tester.pumpAndSettle();
    expect(locations(reported).last, ('/sessions?operation_id=G', false));

    // A dialog over the page keeps it where it is.
    unawaited(
      showDialog<void>(
        context: tester.element(find.byType(MissionBoardPage)),
        builder: (_) => const AlertDialog(content: Text('Busy')),
      ),
    );
    await tester.pumpAndSettle();
    await _offer(tester, channel, {'page': 'audit', 'operation_id': 'G'});
    expect(locations(reported).last.$1, '/sessions?operation_id=G');
    expect(find.text('Busy'), findsOneWidget);
  });

  testWidgets('the menu turns the follow preference on for this person', (
    tester,
  ) async {
    final channel = FakeChannel();
    final board = _board(channel, []);
    addTearDown(board.dispose);
    final (chat, reported) = await _open(tester, board, channel);
    expect(chat.followAtOnce, isFalse, reason: 'off by default');
    await _offer(tester, channel, {'page': 'audit'});
    expect(find.byType(OverviewPage), findsOneWidget);

    await tester.tap(find.byTooltip('Moneypenny menu'));
    await tester.pumpAndSettle();
    await tester.tap(
      find.widgetWithText(
        CheckedPopupMenuItem<void>,
        "Open Moneypenny's links at once",
      ),
    );
    await tester.pumpAndSettle();
    expect(chat.followAtOnce, isTrue);
    final preferences = await SharedPreferences.getInstance();
    expect(preferences.getBool('oo.chat.ana.follow'), isTrue);

    await _offer(tester, channel, {'page': 'sessions'});
    expect(locations(reported).last, ('/sessions', false));
  });

  testWidgets('following from an expanded chat docks it so the page is in '
      'view', (tester) async {
    final channel = FakeChannel();
    final board = _board(channel, []);
    addTearDown(board.dispose);
    final (chat, _) = await _open(tester, board, channel);
    chat.toggleExpanded();
    await tester.pumpAndSettle();
    expect(chat.layout, ChatLayout.expanded);
    await _offer(tester, channel, {
      'page': 'missions',
      'state': ['failed'],
    });
    await tester.tap(_openButton('nav-1'));
    await tester.pumpAndSettle();
    expect(chat.layout, ChatLayout.docked);
    expect(find.text('Mission g-failed'), findsOneWidget);
  });

  testWidgets('the preview\'s sample offers pages to open', (tester) async {
    await atSize(tester, const Size(1400, 900));
    final board = MissionBoard(
      apiOrigin: Uri.parse('https://oo.example'),
      demo: true,
      pollInterval: null,
      assistantChannel: () => throw StateError('the preview opened a socket'),
      client: MockClient(
        (request) => throw StateError('the preview sent ${request.url}'),
      ),
    );
    addTearDown(board.dispose);
    await tester.pumpWidget(
      OuterOfficeApp(board: board, desktopWindows: false),
    );
    await tester.pumpAndSettle();
    await askFromOverview(tester, 'Which Stations are dark?');
    final card = _card('demo-nav-dark');
    expect(card, findsOneWidget);
    expect(
      find.descendant(
        of: card,
        matching: find.text('Station: station-remote-02'),
      ),
      findsOneWidget,
    );
    await tester.tap(find.descendant(of: card, matching: find.text('Open')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('opened-by-moneypenny')), findsOneWidget);
    expect(find.byKey(const ValueKey('filter-station_id')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  test('a navigation entry\'s route is read from its JSON form', () {
    expect(
      navigationRoute({
        'route': {'page': 'audit', 'station_id': 'hq'},
      }).toUri(),
      'app:/audit?station_id=hq',
    );
    expect(() => navigationRoute(null), throwsFormatException);
  });
}
