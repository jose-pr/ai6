import 'dart:js_interop';
import 'dart:js_interop_unsafe';

/// No local configuration file where there is no file system (the web).
String? desktopConfigPath() => null;

/// There is no local `ai6.cfg` to read.
String? readDesktopConfig() => null;

/// There is no local `ai6.cfg` to write.
Future<void> writeDesktopConfig(String text) =>
    Future.error(UnsupportedError('ai6.cfg cannot be written here'));

/// A browser keeps its own certificate authorities.
void trustCaFile(String caFile, String? configPath) {}

/// The page's `ai6.cfg` as its bootstrap script fetched it (`window.ooConfig`, a promise of the
/// HTTP status, 0 when unreachable, and the text); `null` when the page did not start one.
Future<({int status, String text})?> pageConfig() async {
  final promise = globalContext.getProperty<JSAny?>('ooConfig'.toJS);
  if (!promise.isA<JSPromise<JSObject>>()) return null;
  final answer = await (promise as JSPromise<JSObject>).toDart;
  return (
    status: answer.getProperty<JSNumber>('status'.toJS).toDartInt,
    text: answer.getProperty<JSString>('text'.toJS).toDart,
  );
}
