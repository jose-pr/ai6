import 'dart:js_interop';

import 'package:web/web.dart' as web;

/// Hands [text] to the browser as a download named [name]; the browser decides where it goes.
Future<String?> saveText(String name, String text) async {
  final blob = web.Blob(
    [text.toJS].toJS,
    web.BlobPropertyBag(type: 'text/markdown;charset=utf-8'),
  );
  final url = web.URL.createObjectURL(blob);
  final link = web.HTMLAnchorElement()
    ..href = url
    ..download = name;
  link.click();
  web.URL.revokeObjectURL(url);
  return null;
}
