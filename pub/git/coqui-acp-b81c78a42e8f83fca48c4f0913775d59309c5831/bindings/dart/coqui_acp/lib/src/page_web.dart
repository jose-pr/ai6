import 'dart:async';
import 'dart:js_interop';
import 'dart:js_interop_unsafe';

import 'page_api.dart';

extension type _JsAcp(JSObject _) implements JSObject {
  external JSNumber? get version;
  external JSPromise<JSArray<_JsAgent>> agents();
  external JSPromise<JSArray<_JsAgent>> request(JSObject options);
  external JSPromise<_JsConnection> connect(_JsAgent agent);
  external JSPromise<JSAny?> revoke(_JsAgent agent);
}

extension type _JsIdentity(JSObject _) implements JSObject {
  external JSString? get peer;
  external JSBoolean? get verified;
}

extension type _JsAgent(JSObject _) implements JSObject {
  external JSString get id;
  external JSString get label;
  external _JsIdentity? get identity;
}

extension type _JsConnection(JSObject _) implements JSObject {
  external _JsAgent get agent;
  external _JsStream get readable;
  external _JsStream get writable;
  external JSPromise<_JsClosed> get closed;
  external void close();
}

extension type _JsClosed(JSObject _) implements JSObject {
  external JSString? get reason;
}

extension type _JsStream(JSObject _) implements JSObject {
  external _JsReader getReader();
  external _JsWriter getWriter();
}

extension type _JsReader(JSObject _) implements JSObject {
  external JSPromise<_JsRead> read();
}

extension type _JsRead(JSObject _) implements JSObject {
  external JSBoolean get done;
  external JSString? get value;
}

extension type _JsWriter(JSObject _) implements JSObject {
  external JSPromise<JSAny?> write(JSString message);
}

extension type _JsEvent(JSObject _) implements JSObject {
  external _JsDetail? get detail;
}

extension type _JsDetail(JSObject _) implements JSObject {
  external JSObject? get acp;
}

AcpPageFailure _failure(Object error) {
  // ignore: invalid_runtime_check_with_js_interop_types
  if (error is JSObject) {
    final name = error.getProperty<JSAny?>('name'.toJS);
    final message = error.getProperty<JSAny?>('message'.toJS);
    return AcpPageFailure(
      name.isA<JSString>() ? (name as JSString).toDart : 'Error',
      message.isA<JSString>() ? (message as JSString).toDart : '',
    );
  }
  return AcpPageFailure('Error', '$error');
}

Future<T> _awaited<T extends JSAny?>(JSPromise<T> promise) async {
  try {
    return await promise.toDart;
  } catch (error) {
    throw _failure(error);
  }
}

bool _usable(JSObject? acp) {
  if (acp == null) return false;
  return acp.getProperty<JSAny?>('request'.toJS).isA<JSFunction>() &&
      (acp as _JsAcp).version?.toDartDouble == 1;
}

/// Finds a provider by event, so markup in the page cannot shadow it. `null` if none answers within [timeout]; one may still
/// arrive later, for instance when the person clicks an extension's button.
Future<AcpPage?> findAcp({
  Duration timeout = const Duration(milliseconds: 100),
}) async {
  final found = Completer<AcpPage?>();
  final answered = ((JSObject event) {
    final acp = (event as _JsEvent).detail?.acp;
    if (_usable(acp) && !found.isCompleted) {
      found.complete(_Acp(acp! as _JsAcp));
    }
  }).toJS;
  globalContext.callMethod<JSAny?>(
    'addEventListener'.toJS,
    'acp:ready'.toJS,
    answered,
  );
  final constructor = globalContext.getProperty<JSFunction>('Event'.toJS);
  globalContext.callMethod<JSAny?>(
    'dispatchEvent'.toJS,
    constructor.callAsConstructor<JSObject>('acp:request'.toJS),
  );
  final timer = Timer(timeout, () {
    if (!found.isCompleted) found.complete(null);
  });
  try {
    return await found.future;
  } finally {
    timer.cancel();
    globalContext.callMethod<JSAny?>(
      'removeEventListener'.toJS,
      'acp:ready'.toJS,
      answered,
    );
  }
}

final class _Agent implements AcpPageAgent {
  _Agent(this.js);
  final _JsAgent js;
  @override
  String get id => js.id.toDart;
  @override
  String get label => js.label.toDart;
  @override
  String? get peer => js.identity?.peer?.toDart;
  @override
  bool get verified => js.identity?.verified?.toDart == true;
}

final class _Connection implements AcpPageConnection {
  _Connection(this.js)
    : _reader = js.readable.getReader(),
      _writer = js.writable.getWriter() {
    _pump();
  }

  final _JsConnection js;
  final _JsReader _reader;
  final _JsWriter _writer;
  // A provider's stream holds what arrives, so nothing is lost before someone listens here either.
  final _incoming = StreamController<String>();

  Future<void> _pump() async {
    try {
      for (;;) {
        final read = await _reader.read().toDart;
        if (read.done.toDart) break;
        final value = read.value;
        if (value != null) _incoming.add(value.toDart);
      }
    } catch (error) {
      _incoming.addError(_failure(error));
    } finally {
      await _incoming.close();
    }
  }

  @override
  AcpPageAgent get agent => _Agent(js.agent);
  @override
  Stream<String> get messages => _incoming.stream;
  @override
  Future<void> send(String message) => _awaited(_writer.write(message.toJS));
  @override
  Future<void> close() async => js.close();
  @override
  Future<String> get closed async =>
      (await _awaited(js.closed)).reason?.toDart ?? '';
}

final class _Acp implements AcpPage {
  _Acp(this.js);
  final _JsAcp js;

  List<AcpPageAgent> _agents(JSArray<_JsAgent> list) => [
    for (final agent in list.toDart) _Agent(agent),
  ];

  _JsAgent _js(AcpPageAgent agent) {
    if (agent is _Agent) return agent.js;
    throw ArgumentError.value(agent, 'agent', 'not from this provider');
  }

  @override
  Future<List<AcpPageAgent>> agents() async =>
      _agents(await _awaited(js.agents()));

  @override
  Future<List<AcpPageAgent>> request({
    String? purpose,
    List<String> peers = const [],
    List<String> mcpOrigins = const [],
    bool permissions = false,
  }) async {
    final options = JSObject();
    if (purpose != null) options.setProperty('purpose'.toJS, purpose.toJS);
    options
      ..setProperty('peers'.toJS, [for (final peer in peers) peer.toJS].toJS)
      ..setProperty(
        'mcpOrigins'.toJS,
        [for (final origin in mcpOrigins) origin.toJS].toJS,
      )
      ..setProperty('permissions'.toJS, permissions.toJS);
    return _agents(await _awaited(js.request(options)));
  }

  @override
  Future<AcpPageConnection> connect(AcpPageAgent agent) async =>
      _Connection(await _awaited(js.connect(_js(agent))));

  @override
  Future<void> revoke(AcpPageAgent agent) async {
    await _awaited(js.revoke(_js(agent)));
  }
}
