import 'dart:io';

/// Where `ai6.cfg` is on this machine, as [configPathFor] decides.
String? desktopConfigPath() => configPathFor(
  Platform.environment,
  Platform.operatingSystem,
  exists: (path) => File(path).existsSync(),
);

/// The first of `AI6_CONFIG`, the user's and the system's `ai6.cfg` that [exists]; when none does,
/// the user's, where a new one goes. `AI6_CONFIG` is taken whether it exists or not: the Rust
/// programs refuse a named file that is missing, and [readDesktopConfig] does too.
String? configPathFor(
  Map<String, String> environment,
  String operatingSystem, {
  bool Function(String path) exists = _nothingExists,
}) {
  final windows = operatingSystem == 'windows';
  String? set(String name) {
    final value = environment[name];
    return value == null || value.isEmpty ? null : value;
  }

  final named = set('AI6_CONFIG');
  if (named != null) return named;
  final separator = windows ? r'\' : '/';
  String join(String base) => '$base${separator}ai6${separator}ai6.cfg';
  final home = set('HOME') ?? set('USERPROFILE');
  final xdg = set('XDG_CONFIG_HOME');
  final appData = windows ? set('APPDATA') : null;
  final user = xdg != null
      ? join(xdg)
      : appData != null
      ? join(appData)
      : home != null
      ? join('$home$separator.config')
      : null;
  final programData = set('ProgramData');
  final system = windows
      ? (programData == null ? null : join(programData))
      : '/etc/ai6/ai6.cfg';
  for (final path in [user, system]) {
    if (path != null && exists(path)) return path;
  }
  return user;
}

bool _nothingExists(String path) => false;

/// The `ai6.cfg` text, or `null` when there is no file.
///
/// Throws a [FileSystemException] when `AI6_CONFIG` names a file that is not there.
String? readDesktopConfig() {
  final path = desktopConfigPath();
  if (path == null) return null;
  final file = File(path);
  if (file.existsSync()) return file.readAsStringSync();
  if (Platform.environment['AI6_CONFIG']?.isNotEmpty ?? false) {
    throw FileSystemException('named by AI6_CONFIG, does not exist', path);
  }
  return null;
}

/// Writes `ai6.cfg`, creating its directory.
Future<void> writeDesktopConfig(String text) async {
  final path = desktopConfigPath();
  if (path == null) throw StateError('no configuration directory for ai6.cfg');
  final file = File(path);
  await file.parent.create(recursive: true);
  await file.writeAsString(text, flush: true);
}

/// [caFile] resolved beside the file that names it, [configPath], when it is relative.
String caFilePath(String caFile, String? configPath) {
  if (File(caFile).isAbsolute || configPath == null) return caFile;
  return '${File(configPath).parent.path}${Platform.pathSeparator}$caFile';
}

/// Adds the certificates of [caFile] to what [context] (default: every connection's) trusts.
///
/// Throws when the file cannot be read or holds no certificate; its message holds no content.
void trustCaFile(
  String caFile,
  String? configPath, {
  SecurityContext? context,
}) {
  final path = caFilePath(caFile, configPath);
  (context ?? SecurityContext.defaultContext).setTrustedCertificates(path);
}
