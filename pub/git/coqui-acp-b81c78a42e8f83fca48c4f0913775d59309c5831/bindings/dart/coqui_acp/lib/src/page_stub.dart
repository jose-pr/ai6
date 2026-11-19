import 'page_api.dart';

/// Outside a browser there is no provider.
Future<AcpPage?> findAcp({
  Duration timeout = const Duration(milliseconds: 100),
}) async => null;
