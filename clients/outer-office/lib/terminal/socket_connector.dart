import 'package:web_socket_channel/web_socket_channel.dart';

import 'socket_connector_stub.dart'
    if (dart.library.io) 'socket_connector_io.dart'
    if (dart.library.js_interop) 'socket_connector_web.dart'
    as platform;

WebSocketChannel connectRunSocket(
  Uri uri, {
  required List<String> protocols,
  required String origin,
  required String? cookie,
}) => platform.connectRunSocket(
  uri,
  protocols: protocols,
  origin: origin,
  cookie: cookie,
);
