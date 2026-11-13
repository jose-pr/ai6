import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/assistant/assistant_controller.dart';
import 'package:outer_office/assistant/chat_layout.dart';
import 'package:outer_office/assistant/chat_popout.dart';
import 'package:outer_office/assistant/chat_region.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'polish_test.dart' show FakeChannel;

class FakeHandle implements PopupHandle {
  @override
  bool closed = false;
  int focused = 0;

  @override
  void close() => closed = true;

  @override
  void focus() => focused++;
}

/// A link whose far end the test is: what it posts is [sent], what it hears it is given.
class FakeLink implements ChatLink {
  final sent = <Map<String, Object?>>[];
  final _in = StreamController<Map<String, Object?>>.broadcast(sync: true);
  bool closed = false;

  void hear(Map<String, Object?> message) => _in.add(message);

  @override
  Stream<Map<String, Object?>> get messages => _in.stream;

  @override
  void post(Map<String, Object?> message) => sent.add(message);

  @override
  void close() => closed = true;
}

class FakePopups implements PopupPlatform {
  FakePopups({this.blocked = false});

  bool blocked;
  bool opener = true;
  final handles = <FakeHandle>[];
  final links = <FakeLink>[];
  final pageHides = StreamController<void>.broadcast(sync: true);
  final focuses = StreamController<void>.broadcast(sync: true);
  int closedSelf = 0;
  Uri? opened;

  @override
  Uri get popoutUrl => Uri.parse('https://oo.example/?oo_window=chat');

  @override
  PopupHandle? open(Uri url, {required double width, required double height}) {
    opened = url;
    if (blocked) return null;
    return FakeHandle()..also(handles.add);
  }

  @override
  ChatLink link(String name) => FakeLink()..also(links.add);

  @override
  Stream<void> get pageHide => pageHides.stream;

  @override
  Stream<void> get focused => focuses.stream;

  @override
  bool get openerGone => !opener;

  @override
  void closeSelf() => closedSelf++;
}

extension<T> on T {
  void also(void Function(T) f) => f(this);
}

MoneypennyChat chatWith(
  FakePopups popups, {
  bool outlives = false,
  Listenable? appearance,
  Map<String, Object?> Function()? look,
}) {
  final chat = MoneypennyChat(
    controller: AssistantController(token: 't', connect: FakeChannel.new),
    preferences: ChatPreferences(SharedPreferences.getInstance()),
    windowOutlivesPage: outlives,
    openWindow: popupChatWindowOpener(
      popups,
      appearance: appearance,
      appearanceNow: look ?? () => {'theme': 'night-ops', 'backdrop': true},
    ),
  );
  addTearDown(chat.dispose);
  return chat;
}

/// Pops [chat] out and runs the sync the shell runs after each change.
Future<void> popOut(WidgetTester tester, MoneypennyChat chat) async {
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
}

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  testWidgets(
    'Pop out opens one browser window at the app\'s pop-out address',
    (tester) async {
      final popups = FakePopups();
      final chat = chatWith(popups);
      await popOut(tester, chat);
      expect(chat.layout, ChatLayout.popped);
      expect(popups.opened?.queryParameters, {popoutParameter: popoutChat});
      expect(popups.handles, hasLength(1));
      // Asked again by a rebuild, it does not open a second one.
      chat.syncWindow(tester.element(find.byType(SizedBox)));
      expect(popups.handles, hasLength(1));
      expect(chatPlacement(chat.layout, 1400), ChatPlacement.away);
    },
  );

  testWidgets(
    'the person closing the window brings the chat back into the page',
    (tester) async {
      final popups = FakePopups();
      final chat = chatWith(popups);
      await popOut(tester, chat);
      popups.links.single.hear({'t': 'bye', 'why': 'closed'});
      expect(chat.layout, ChatLayout.docked);
      expect(popups.links.single.closed, isTrue);

      // Bring back from the pop-out is the same; Hide leaves the chat hidden.
      chat.togglePopOut();
      chat.syncWindow(tester.element(find.byType(SizedBox)));
      popups.links.last.hear({'t': 'bye', 'why': 'hide'});
      expect(chat.layout, ChatLayout.collapsed);
    },
  );

  testWidgets('a window the browser closed without a word is found when the '
      'page gets the focus back', (tester) async {
    final popups = FakePopups();
    final chat = chatWith(popups);
    await popOut(tester, chat);
    popups.focuses.add(null);
    expect(chat.layout, ChatLayout.popped, reason: 'still open');
    popups.handles.single.closed = true;
    popups.focuses.add(null);
    expect(chat.layout, ChatLayout.docked);
  });

  testWidgets('Bring back from the page closes the window and tells it', (
    tester,
  ) async {
    final popups = FakePopups();
    final chat = chatWith(popups);
    await popOut(tester, chat);
    chat.togglePopOut();
    chat.syncWindow(tester.element(find.byType(SizedBox)));
    expect(popups.handles.single.closed, isTrue);
    expect(popups.links.single.sent.last, {'t': 'gone'});
    expect(chat.layout, ChatLayout.docked);
  });

  testWidgets('a blocked pop-up leaves the chat in the page and says so', (
    tester,
  ) async {
    final popups = FakePopups(blocked: true);
    final chat = chatWith(popups);
    await popOut(tester, chat);
    expect(chat.layout, ChatLayout.docked);
    expect(chat.windowNotice, ChatWindowBlocked.message);
    expect(popups.links.single.closed, isTrue, reason: 'no channel left open');

    chat.dismissWindowNotice();
    expect(chat.windowNotice, isNull);

    // Allowed, the next try works and starts without the notice.
    popups.blocked = false;
    chat.togglePopOut();
    chat.syncWindow(tester.element(find.byType(SizedBox)));
    expect(chat.layout, ChatLayout.popped);
    expect(chat.windowNotice, isNull);
  });

  testWidgets('the notice shows in the chat until dismissed', (tester) async {
    final popups = FakePopups(blocked: true);
    final chat = chatWith(popups);
    await popOut(tester, chat);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ChatPanel(chat: chat, placement: ChatPlacement.side),
        ),
      ),
    );
    expect(find.text(ChatWindowBlocked.message), findsOneWidget);
    await tester.tap(find.byTooltip('Dismiss'));
    await tester.pump();
    expect(find.text(ChatWindowBlocked.message), findsNothing);
  });

  testWidgets('a page that reloads comes back with the chat in the page', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({
      ChatPreferences.key('ana'): '{"layout":"popped","width":420}',
    });
    final popups = FakePopups();
    final gone = chatWith(popups);
    await gone.restore('ana', narrow: false);
    expect(gone.layout, ChatLayout.docked);

    // A native window outlives the page, so there it is kept.
    final native = chatWith(popups, outlives: true);
    await native.restore('ana', narrow: false);
    expect(native.layout, ChatLayout.popped);
  });

  testWidgets('the opener going away tells the window', (tester) async {
    final popups = FakePopups();
    final chat = chatWith(popups);
    await popOut(tester, chat);
    popups.pageHides.add(null);
    expect(popups.links.single.sent.last, {'t': 'gone'});
  });

  testWidgets('the window gets the conversation and draft it asked for', (
    tester,
  ) async {
    final popups = FakePopups();
    final chat = chatWith(popups);
    chat.message.text = 'half a thought';
    await popOut(tester, chat);
    popups.links.single.hear({'t': 'ask'});
    final state = popups.links.single.sent.last;
    expect(state['t'], 'state');
    expect(state['draft'], 'half a thought');
    expect(state['theme'], 'night-ops');
  });

  testWidgets('the draft follows in both directions without echoing', (
    tester,
  ) async {
    final popups = FakePopups();
    final chat = chatWith(popups);
    await popOut(tester, chat);
    final link = popups.links.single;

    link.hear({'t': 'draft', 'text': 'typed in the window'});
    expect(chat.message.text, 'typed in the window');
    expect(
      link.sent.where((m) => m['t'] == 'draft'),
      isEmpty,
      reason: 'what the window said is not said back',
    );

    chat.message.text = 'typed in the page';
    expect(link.sent.last, {'t': 'draft', 'text': 'typed in the page'});
  });

  testWidgets('a link she offers in the window opens in the page', (
    tester,
  ) async {
    final popups = FakePopups();
    final chat = chatWith(popups);
    final opened = <String>[];
    chat.onLink = opened.add;
    await popOut(tester, chat);
    popups.links.single.hear({'t': 'link', 'link': 'app:/missions'});
    expect(opened, ['app:/missions']);
  });

  testWidgets('the window follows the theme of the page, once per change', (
    tester,
  ) async {
    final popups = FakePopups();
    final theme = ValueNotifier('night-ops');
    addTearDown(theme.dispose);
    final chat = chatWith(
      popups,
      appearance: theme,
      look: () => {'theme': theme.value, 'backdrop': true},
    );
    await popOut(tester, chat);
    final link = popups.links.single;
    link.hear({'t': 'ask'});
    final before = link.sent.length;
    theme.value = 'high-contrast';
    expect(link.sent.length, before + 1);
    expect(link.sent.last['t'], 'appearance');
    expect(link.sent.last['theme'], 'high-contrast');
    theme.notifyListeners();
    expect(link.sent.length, before + 1, reason: 'unchanged');
  });

  group('the pop-out side', () {
    test('asks its opener, and takes the conversation and draft', () async {
      final popups = FakePopups();
      final run = PopoutSession(popups, person: 'ana');
      addTearDown(run.dispose);
      final link = popups.links.single;
      expect(link.sent, [
        {'t': 'ask'},
      ]);
      link.hear({
        't': 'state',
        'conversation': 'c-9',
        'draft': 'so far',
        'theme': 'night-ops',
        'backdrop': false,
      });
      final state = await run.state;
      expect(state.conversation, 'c-9');
      expect(state.draft, 'so far');
      expect(state.appearance, {'theme': 'night-ops', 'backdrop': false});
    });

    test('keeps the draft level with the opener\'s and sends its links', () {
      final popups = FakePopups();
      final run = PopoutSession(popups, person: 'ana');
      addTearDown(run.dispose);
      final chat = MoneypennyChat(
        controller: AssistantController(token: 't', connect: FakeChannel.new),
      );
      addTearDown(chat.dispose);
      run.attach(chat, draft: 'so far');
      final link = popups.links.single;
      expect(chat.message.text, 'so far');

      chat.message.text = 'so far, and more';
      expect(link.sent.last, {'t': 'draft', 'text': 'so far, and more'});
      link.hear({'t': 'draft', 'text': 'edited in the page'});
      expect(chat.message.text, 'edited in the page');
      expect(link.sent.last, {'t': 'draft', 'text': 'so far, and more'});

      chat.onLink!('app:/missions');
      expect(link.sent.last, {'t': 'link', 'link': 'app:/missions'});
    });

    test('Bring back and Hide tell the opener, and close the window', () {
      final back = FakePopups();
      final backRun = PopoutSession(back, person: 'ana');
      addTearDown(backRun.dispose);
      backRun.bringBack();
      expect(back.links.single.sent.last, {'t': 'bye', 'why': 'back'});
      expect(back.closedSelf, 1);

      final hide = FakePopups();
      final hideRun = PopoutSession(hide, person: 'ana');
      addTearDown(hideRun.dispose);
      hideRun.hide();
      expect(hide.links.single.sent.last, {'t': 'bye', 'why': 'hide'});

      // Closed afterwards by the browser, it says nothing more.
      hide.pageHides.add(null);
      expect(
        hide.links.single.sent.where((m) => m['t'] == 'bye'),
        hasLength(1),
      );
    });

    test('closing the window tells the opener', () {
      final popups = FakePopups();
      final run = PopoutSession(popups, person: 'ana');
      addTearDown(run.dispose);
      popups.pageHides.add(null);
      expect(popups.links.single.sent.last, {'t': 'bye', 'why': 'closed'});
    });

    test('an opener that is gone leaves nothing to show', () {
      final popups = FakePopups()..opener = false;
      final run = PopoutSession(popups, person: 'ana');
      addTearDown(run.dispose);
      expect(run.openerClosed.value, isTrue);
      expect(popups.closedSelf, 1);
      expect(popups.links.single.sent, isEmpty);
    });

    test('an opener that reloads or closes sends the window away', () {
      final popups = FakePopups();
      final run = PopoutSession(popups, person: 'ana');
      addTearDown(run.dispose);
      popups.links.single.hear({'t': 'gone'});
      expect(run.openerClosed.value, isTrue);
      expect(popups.closedSelf, 1);
      // The window going afterwards says nothing to an opener that is not there.
      popups.pageHides.add(null);
      expect(popups.links.single.sent.where((m) => m['t'] == 'bye'), isEmpty);
    });
  });

  test('the pop-out is this app with one query parameter', () {
    final url = popoutUriFrom(
      Uri.parse('https://oo.example/app/?x=1#/missions'),
    );
    expect(url.toString(), 'https://oo.example/app/?oo_window=chat');
    expect(startedAsPopout(url), isTrue);
    expect(startedAsPopout(Uri.parse('https://oo.example/')), isFalse);
  });

  test('a frame that is not a JSON object is ignored', () {
    expect(decodeLinkMessage('not json'), isNull);
    expect(decodeLinkMessage('[1]'), isNull);
    expect(decodeLinkMessage(3), isNull);
    expect(decodeLinkMessage('{"t":"ask"}'), {'t': 'ask'});
  });
}
