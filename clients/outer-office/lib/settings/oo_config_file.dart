import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'oo_config.dart';
import 'oo_config_file_none.dart'
    if (dart.library.io) 'oo_config_file_io.dart'
    as platform;

/// Where the desktop app's `OO.cfg` is: `OO_CONFIG`, else `OO/OO.cfg` in the platform's
/// configuration directory (`%APPDATA%`, `~/Library/Application Support`,
/// `$XDG_CONFIG_HOME` or `~/.config`); `null` where there is none (the web).
String? desktopConfigPath() => platform.desktopConfigPath();

/// The desktop `OO.cfg`'s text, or `null` when there is none.
String? readDesktopConfig() => platform.readDesktopConfig();

/// Replaces the desktop `OO.cfg`'s text, creating its directory.
Future<void> writeDesktopConfig(String text) =>
    platform.writeDesktopConfig(text);

/// The desktop `OO.cfg`, read; [OoConfig.none] when there is none or it cannot be read, which
/// is logged.
OoConfig loadDesktopConfig() {
  try {
    final text = readDesktopConfig();
    return text == null ? OoConfig.none : OoConfig.parse(text);
  } on Object catch (error) {
    debugPrint(
      'Outer Office: ${desktopConfigPath()}: $error; using the defaults',
    );
    return OoConfig.none;
  }
}

/// The web page's `OO.cfg`, beside `index.html` at [page], as read-only deployment defaults;
/// [OoConfig.none] when it is missing or unreadable, which is logged.
Future<OoConfig> loadWebConfig(Uri page, {http.Client? client}) async {
  final owned = client == null;
  final reader = client ?? http.Client();
  try {
    final response = await reader.get(page.resolve('OO.cfg'));
    if (response.statusCode == 404) return OoConfig.none;
    if (response.statusCode != 200) {
      throw FormatException('HTTP ${response.statusCode}');
    }
    return OoConfig.parse(response.body);
  } on Object catch (error) {
    debugPrint('Outer Office: OO.cfg: $error; using the defaults');
    return OoConfig.none;
  } finally {
    if (owned) reader.close();
  }
}
