/// No local configuration file where there is no file system (the web).
String? desktopConfigPath() => null;

/// There is no local `ai6.cfg` to read.
String? readDesktopConfig() => null;

/// There is no local `ai6.cfg` to write.
Future<void> writeDesktopConfig(String text) =>
    Future.error(UnsupportedError('ai6.cfg cannot be written here'));

/// A browser keeps its own certificate authorities.
void trustCaFile(String caFile, String? configPath) {}
