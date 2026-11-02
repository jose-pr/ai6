import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'oo_config.dart';
import 'oo_config_file_none.dart'
    if (dart.library.io) 'oo_config_file_io.dart'
    as platform;

/// The name of the file beside the web page, and of the desktop's.
const configFileName = 'ai6.cfg';

/// Where the desktop app's `ai6.cfg` is, found as the Rust programs find it: `AI6_CONFIG`, else
/// `ai6/ai6.cfg` in the user's configuration directory (`$XDG_CONFIG_HOME`, `%APPDATA%` or
/// `~/.config`), else the system's (`/etc`, `%ProgramData%`), else where a new one goes: the
/// user's. `null` where there is none (the web).
String? desktopConfigPath() => platform.desktopConfigPath();

/// The desktop `ai6.cfg`'s text, or `null` when there is none.
String? readDesktopConfig() => platform.readDesktopConfig();

/// Replaces the desktop `ai6.cfg`'s text, creating its directory.
Future<void> writeDesktopConfig(String text) =>
    platform.writeDesktopConfig(text);

/// The desktop `ai6.cfg`, read; [OoConfig.none] when there is none or it cannot be read, which
/// is logged, by path and never quoting the file.
OoConfig loadDesktopConfig() {
  try {
    final text = readDesktopConfig();
    return text == null
        ? OoConfig.none
        : OoConfig.parse(text, path: desktopConfigPath());
  } on Object catch (error) {
    debugPrint(
      'Outer Office: ${desktopConfigPath()}: $error; using the defaults',
    );
    return OoConfig.none;
  }
}

/// Trusts [OoConfig.caFile] for this process's connections, as well as the system's roots. A file
/// that cannot be used is logged: too little trust refuses the connection, so nothing is weakened.
void trustDesktopCa(OoConfig config) {
  final caFile = config.caFile;
  if (caFile == null) return;
  try {
    platform.trustCaFile(caFile, desktopConfigPath());
  } on Object catch (error) {
    debugPrint('Outer Office: [unex] ca_file: $error');
  }
}

/// The text of the `ai6.cfg` beside the web page, as [loadWebConfig] found it; `null` when there
/// is none. The settings page shows it.
String? webConfigText;

/// The web page's `ai6.cfg`, beside `index.html` at [page]: a public one, read-only deployment
/// defaults, that `ai6 config public` writes. [OoConfig.none] when it is missing or unreadable,
/// which is logged.
Future<OoConfig> loadWebConfig(Uri page, {http.Client? client}) async {
  final owned = client == null;
  final reader = client ?? http.Client();
  try {
    final response = await reader.get(page.resolve(configFileName));
    if (response.statusCode == 404) return OoConfig.none;
    if (response.statusCode != 200) {
      throw FormatException('HTTP ${response.statusCode}');
    }
    webConfigText = response.body;
    return OoConfig.parse(response.body);
  } on Object catch (error) {
    debugPrint('Outer Office: $configFileName: $error; using the defaults');
    return OoConfig.none;
  } finally {
    if (owned) reader.close();
  }
}
