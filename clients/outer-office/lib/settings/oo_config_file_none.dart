/// No local configuration file where there is no file system (the web).
String? desktopConfigPath() => null;

/// There is no local `OO.cfg` to read.
String? readDesktopConfig() => null;

/// There is no local `OO.cfg` to write.
Future<void> writeDesktopConfig(String text) =>
    Future.error(UnsupportedError('OO.cfg cannot be written here'));
