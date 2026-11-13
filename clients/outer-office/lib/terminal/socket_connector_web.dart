import 'package:web_socket_channel/web_socket_channel.dart';

WebSocketChannel connectRunSocket(
  Uri uri, {
  required List<String> protocols,
  required String origin,
  required String? cookie,
}) => WebSocketChannel.connect(uri, protocols: protocols);
