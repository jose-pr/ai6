import 'dart:io';

/// `OO_CONFIG`, else `OO/OO.cfg` in the platform's configuration directory.
String? desktopConfigPath() =>
    configPathFor(Platform.environment, Platform.operatingSystem);

/// Where `OO.cfg` is for an environment and operating system, as [desktopConfigPath] decides.
String? configPathFor(Map<String, String> environment, String operatingSystem) {
  String? set(String name) {
    final value = environment[name];
    return value == null || value.isEmpty ? null : value;
  }

  final named = set('OO_CONFIG');
  if (named != null) return named;
  final separator = operatingSystem == 'windows' ? r'\' : '/';
  String join(String base) => '$base${separator}OO${separator}OO.cfg';
  if (operatingSystem == 'windows') {
    final appData = set('APPDATA');
    return appData == null ? null : join(appData);
  }
  final home = set('HOME');
  if (operatingSystem == 'macos') {
    return home == null ? null : join('$home/Library/Application Support');
  }
  final xdg = set('XDG_CONFIG_HOME');
  if (xdg != null) return join(xdg);
  return home == null ? null : join('$home/.config');
}

/// The `OO.cfg` text, or `null` when there is no file.
String? readDesktopConfig() {
  final path = desktopConfigPath();
  if (path == null) return null;
  final file = File(path);
  return file.existsSync() ? file.readAsStringSync() : null;
}

/// Writes `OO.cfg`, creating its directory.
Future<void> writeDesktopConfig(String text) async {
  final path = desktopConfigPath();
  if (path == null) throw StateError('no configuration directory for OO.cfg');
  final file = File(path);
  await file.parent.create(recursive: true);
  await file.writeAsString(text, flush: true);
}
