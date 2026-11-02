/// A WebSocket URL for [path] on [origin], from its scheme, host and port alone: on the web
/// [origin] is the page's own URL, and its query and `#/route` are no part of a socket's.
Uri socketUri(Uri origin, String path, [Map<String, String>? query]) => Uri(
  scheme: origin.scheme == 'https' ? 'wss' : 'ws',
  host: origin.host,
  port: origin.hasPort ? origin.port : null,
  path: path,
  queryParameters: query,
);
