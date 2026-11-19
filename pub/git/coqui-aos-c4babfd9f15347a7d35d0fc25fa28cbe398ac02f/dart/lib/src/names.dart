/// Names: overlay identities, variables, relative paths and operating systems.
library;

/// Path components an overlay never carries: version-control and tool caches.
const List<String> excludedComponents = [
  '.git',
  '.hg',
  '.svn',
  '__pycache__',
  '.mypy_cache',
  '.pytest_cache',
  '.ruff_cache',
  '.tox',
  '.venv',
  'node_modules',
];

const List<String> _reservedNames = [
  'default',
  'overlay',
  'system',
  'user',
  'project',
  'project-root',
];

/// ASCII-only lowercase: no other letter changes.
String asciiLower(String text) => text.replaceAllMapped(
  RegExp('[A-Z]'),
  (m) => String.fromCharCode(m[0]!.codeUnitAt(0) + 32),
);

/// ASCII-only uppercase.
String asciiUpper(String text) => text.replaceAllMapped(
  RegExp('[a-z]'),
  (m) => String.fromCharCode(m[0]!.codeUnitAt(0) - 32),
);

/// Whether [name] is a valid overlay identity.
bool validOverlayName(String name) {
  if (!overlayNameShaped(name)) return false;
  final lower = asciiLower(name);
  final stem = lower.split('.').first;
  return !_reservedNames.contains(lower) && !_isDeviceStem(stem, false);
}

final RegExp _nameShape = RegExp(r'^[A-Za-z][A-Za-z0-9_.-]*$');
final RegExp _nameEnd = RegExp(r'[A-Za-z0-9]$');

/// An identity's shape alone: a letter first, then letters, digits, `_`, `.` and `-`, ending in
/// a letter or digit.
bool overlayNameShaped(String name) =>
    _nameShape.hasMatch(name) && _nameEnd.hasMatch(name);

/// The identity compared: ASCII lowercase, `_` and `.` made `-`.
String normalizeName(String name) =>
    asciiLower(name.replaceAll(RegExp('[_.]'), '-'));

/// `NAME_OVERLAY_ROOT`.
String rootVariable(String identity) =>
    '${asciiUpper(normalizeName(identity)).replaceAll(RegExp('[^A-Z0-9]'), '_')}'
    '_OVERLAY_ROOT';

/// `NAME_OVERLAY_STATE`.
String stateVariable(String identity) {
  final root = rootVariable(identity);
  return '${root.substring(0, root.length - 'ROOT'.length)}STATE';
}

final RegExp _variableName = RegExp(r'^[A-Za-z_][A-Za-z0-9_]*$');

/// Whether [name] is an environment variable's name: `[A-Za-z_][A-Za-z0-9_]*`.
bool isVariableName(String name) => _variableName.hasMatch(name);

/// Whether a variable is the specification's own: `AOS_*`, `*_OVERLAY_ROOT`, `*_OVERLAY_STATE`.
bool isReservedVariable(String name) {
  final upper = asciiUpper(name);
  return upper.startsWith('AOS_') ||
      upper.endsWith('_OVERLAY_ROOT') ||
      upper.endsWith('_OVERLAY_STATE');
}

const Set<String> _sensitiveNames = {
  'PATH',
  'PATHEXT',
  'HOME',
  'USERPROFILE',
  'SHELL',
  'COMSPEC',
  'SYSTEMROOT',
  'BASH_ENV',
  'ENV',
  'ZDOTDIR',
  'PROMPT_COMMAND',
  'IFS',
  'EDITOR',
  'VISUAL',
  'PAGER',
  'LESSOPEN',
  'LESSCLOSE',
  'XDG_CONFIG_HOME',
  'XDG_DATA_HOME',
  'APPDATA',
  'LOCALAPPDATA',
  'TMPDIR',
  'TEMP',
  'TMP',
  'PSMODULEPATH',
  'CLASSPATH',
  'JAVA_TOOL_OPTIONS',
  '_JAVA_OPTIONS',
  'JDK_JAVA_OPTIONS',
  'PERL5OPT',
  'PERL5LIB',
  'RUBYOPT',
  'RUBYLIB',
  'RUSTFLAGS',
  'GOPROXY',
  'GONOSUMDB',
  'GONOSUMCHECK',
  'GOPRIVATE',
  'GOFLAGS',
  'SSH_ASKPASS',
  'SSLKEYLOGFILE',
  'REQUESTS_CA_BUNDLE',
  'CURL_CA_BUNDLE',
  'CLAUDE_CONFIG_DIR',
  'CODEX_HOME',
};

const List<String> _sensitivePrefixes = [
  'LD_',
  'DYLD_',
  'GIT_',
  'PYTHON',
  'NODE_',
  'NPM_CONFIG_',
  'PIP_',
  'UV_',
  'CARGO_',
  'RUSTC_',
  'DOTNET_',
  'COR_',
  'CORECLR_',
  'SSL_',
];

/// Whether a variable changes how a program loads code or where it talks: needs the user's
/// say-so.
bool isSensitiveVariable(String name) {
  final upper = asciiUpper(name);
  return _sensitiveNames.contains(upper) ||
      _sensitivePrefixes.any(upper.startsWith) ||
      upper.endsWith('_PROXY') ||
      upper.endsWith('_BASE_URL');
}

/// Whether a variable is a list of directories joined by the platform's separator.
bool isPathVariable(String name) {
  final upper = asciiUpper(name);
  return upper == 'PATH' || upper == 'PYTHONPATH' || upper == 'NODE_PATH';
}

final RegExp _refusedInComponent = RegExp(
  r'[\\:*?"<>|\u0000-\u001f\u007f-\u009f\u200c-\u200f\u202a-\u202e\u2066-\u206f\ufeff]',
);

bool _isDeviceStem(String stem, bool console) {
  if (const ['con', 'prn', 'aux', 'nul'].contains(stem)) return true;
  if (console && (stem == 'conin\$' || stem == 'conout\$')) return true;
  final digits = console ? '0123456789¹²³' : '123456789';
  for (final prefix in const ['com', 'lpt']) {
    if (stem.startsWith(prefix)) {
      final rest = stem.substring(prefix.length).runes.toList();
      if (rest.length == 1 && digits.contains(String.fromCharCode(rest[0]))) {
        return true;
      }
    }
  }
  return false;
}

bool _validComponent(String component) {
  if (component == '' ||
      component.endsWith('.') ||
      component.endsWith(' ') ||
      _refusedInComponent.hasMatch(component)) {
    return false;
  }
  final lower = asciiLower(component);
  if (excludedComponents.contains(lower) || lower.endsWith('.pyc')) {
    return false;
  }
  return !_isDeviceStem(lower.split('.').first, true);
}

final RegExp _shortName = RegExp('~[0-9]');

/// Whether [path] is a relative path an overlay may name: `/`-separated, no component that a
/// platform or a tool reserves.
bool validRelativePath(String path) =>
    path != '' &&
    !_shortName.hasMatch(path) &&
    path.split('/').every(_validComponent);

final RegExp _osName = RegExp(r'^[a-z][a-z0-9-]*$');

/// An operating system's name shape: lowercase letters, digits and `-`, a letter first.
bool validOsName(String name) => _osName.hasMatch(name);

const List<String> _unixNames = [
  'macos',
  'linux',
  'freebsd',
  'openbsd',
  'netbsd',
];

/// Whether the current operating system is one of [names] (null means every one; `unix` is every
/// Unix).
bool osMatches(Iterable<String>? names, String current) {
  if (names == null) return true;
  return names.any(
    (name) =>
        name == current ||
        (name == 'unix' &&
            current != 'windows' &&
            _unixNames.contains(current)),
  );
}

final RegExp _toolName = RegExp(r'^[A-Za-z0-9][A-Za-z0-9_.-]*$');

/// A tool's name shape: letters and digits first, then `_`, `.` and `-`.
bool validToolName(String name) => _toolName.hasMatch(name);
