import 'package:web_socket_channel/io.dart';

IOWebSocketChannel connectSessionSocket(
  Uri uri, {
  required List<String> protocols,
  required String origin,
  required String? cookie,
}) => IOWebSocketChannel.connect(
  uri,
  protocols: protocols,
  headers: {'Origin': origin, 'x-forwarded-proto': 'https', 'Cookie': ?cookie},
);
