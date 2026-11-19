/// The environment an overlay builds, and the consumer's facts it is built from.
library;

import 'error.dart';
import 'limits.dart';
import 'names.dart';
import 'subst.dart';
import 'tree.dart';
import 'value.dart';

/// Variables in the order they were set, with the ones that came from a secret marked. On
/// Windows names compare without regard to ASCII case. A placeholder never reads a secret
/// variable.
class Environment {
  /// An environment holding [base]; [windows] folds the case of names.
  Environment([
    Iterable<(String, String)> base = const [],
    this.windows = false,
  ]) {
    for (final (name, value) in base) {
      set(name, value, false);
    }
  }

  /// Whether names compare without regard to ASCII case.
  final bool windows;

  // Key -> [first spelling of the name, value].
  Map<String, List<String>> _entries = {};
  Set<String> _secret = {};
  Settings _settings = defaultSettings;

  /// Variables only a placeholder sees (the overlay's own root, name and state); empty outside
  /// an application.
  List<(String, String)> extra = [];

  /// The settings that bound a substitution.
  Environment settings(Settings settings) {
    _settings = settings;
    return this;
  }

  /// An independent copy.
  Environment clone() {
    final copy = Environment([], windows);
    copy._entries = {
      for (final entry in _entries.entries) entry.key: [...entry.value],
    };
    copy._secret = {..._secret};
    copy._settings = _settings;
    copy.extra = [...extra];
    return copy;
  }

  /// The key a name is held under: itself, or its ASCII-lowercase form on Windows.
  String _key(String name) => windows ? asciiLower(name) : name;

  /// The value of a variable, secret or not.
  String? get(String name) => _entries[_key(name)]?[1];

  String? _lookup(String name) {
    for (final (key, value) in extra) {
      if (key == name || (windows && asciiLower(key) == asciiLower(name))) {
        return value;
      }
    }
    final key = _key(name);
    final found = _entries[key];
    if (found == null) return null;
    if (_secret.contains(key)) throw AosError('secret-placeholder', name);
    return found[1];
  }

  /// Sets a variable, keeping the name's first spelling; [secret] marks its value as one.
  void set(String name, String value, bool secret) {
    final key = _key(name);
    final held = _entries[key];
    if (held == null) {
      _entries[key] = [name, value];
    } else {
      held[1] = value;
    }
    if (secret) {
      _secret.add(key);
    } else {
      _secret.remove(key);
    }
  }

  /// Removes a variable.
  void unset(String name) {
    final key = _key(name);
    _entries.remove(key);
    _secret.remove(key);
  }

  /// Every variable, in the order set.
  List<(String, String)> values() => [
    for (final held in _entries.values) (held[0], held[1]),
  ];

  /// Whether a variable's value came from a secret.
  bool isSecret(String name) => _secret.contains(_key(name));

  /// Substitutes placeholders in [text]; a refusal names [variable], never the text.
  String substitute(String text, String variable) {
    try {
      return substituteWith(text, _lookup, _settings);
    } on AosError catch (error) {
      throw AosError(error.errorClass, variable);
    }
  }
}

/// The consumer's facts: its operating system, where the overlay is, what the user granted, and
/// what it can run.
class Facts {
  /// Facts for [os], everything else empty.
  Facts(this.os);

  /// The operating system's name as the specification writes it.
  String os;

  /// The overlay's root directory.
  String root = '';

  /// The overlay's identity.
  String identity = '';

  /// The overlay's state directory, if the consumer keeps one.
  String? state;

  /// Whether the overlay is pinned.
  bool pinned = false;

  /// The environment an overlay's own is built on.
  Map<String, String> baseEnv = {};

  /// The host's environment, which an `env` secret provider reads.
  Map<String, String> hostEnv = {};

  /// What the user granted: `secret` (secret names) or a provider's name, to keys.
  Map<String, List<String>> grants = {};

  /// Secrets by name.
  Map<String, String> secrets = {};

  /// Secret files by relative path; null when the consumer holds none.
  Map<String, String>? secretFiles;

  /// The secret providers the consumer has; null means `env` and `file`.
  List<String>? providers;

  /// Values of other providers, by provider and key.
  Map<String, Map<String, String>> providerValues = {};

  /// Whether the consumer enforces network limits; null when it does not say.
  bool? enforcesNetwork;

  /// Whether the consumer runs env scripts; null when it does not say.
  bool? envScriptsEnabled;

  /// The interpreters the consumer has; null when it does not say.
  List<String>? interpreters;

  /// Package versions the consumer knows, by name.
  Map<String, String> packages = {};

  /// The overlay's files.
  OverlayTree tree = OverlayTree.empty();

  /// The bounds in force.
  Settings settings = defaultSettings;

  /// An independent copy that differs in the fields given; a null argument keeps the field.
  /// To clear a nullable field, copy and assign it.
  Facts copyWith({
    String? os,
    String? root,
    String? identity,
    String? state,
    bool? pinned,
    Map<String, String>? baseEnv,
    Map<String, String>? hostEnv,
    Map<String, List<String>>? grants,
    Map<String, String>? secrets,
    Map<String, String>? secretFiles,
    List<String>? providers,
    Map<String, Map<String, String>>? providerValues,
    bool? enforcesNetwork,
    bool? envScriptsEnabled,
    bool? gateEnvScripts,
    String? digest,
    String? allowed,
    List<String>? interpreters,
    Map<String, String>? packages,
    OverlayTree? tree,
    Settings? settings,
  }) {
    final copy = Facts(os ?? this.os)
      ..root = root ?? this.root
      ..identity = identity ?? this.identity
      ..state = state ?? this.state
      ..pinned = pinned ?? this.pinned
      ..baseEnv = baseEnv ?? this.baseEnv
      ..hostEnv = hostEnv ?? this.hostEnv
      ..grants = grants ?? this.grants
      ..secrets = secrets ?? this.secrets
      ..secretFiles = secretFiles ?? this.secretFiles
      ..providers = providers ?? this.providers
      ..providerValues = providerValues ?? this.providerValues
      ..enforcesNetwork = enforcesNetwork ?? this.enforcesNetwork
      ..envScriptsEnabled = envScriptsEnabled ?? this.envScriptsEnabled
      ..gateEnvScripts = gateEnvScripts ?? this.gateEnvScripts
      ..digest = digest ?? this.digest
      ..allowed = allowed ?? this.allowed
      ..interpreters = interpreters ?? this.interpreters
      ..packages = packages ?? this.packages
      ..tree = tree ?? this.tree
      ..settings = settings ?? this.settings;
    return copy;
  }

  /// Whether env scripts run only while [digest] is the [allowed] one, the allowance of the
  /// overlay's hooks; off, none is held back.
  bool gateEnvScripts = false;

  /// The overlay's tree digest, when the consumer has computed it.
  String? digest;

  /// The one digest at which the user allowed this overlay's hooks and env scripts.
  String? allowed;

  /// Whether the operating system is Windows.
  bool get windows => os == 'windows';

  /// Whether the user granted the reference [entry] (a `secret` name, or a provider's key).
  bool granted(Document entry) {
    final secret = entry['secret'];
    if (secret is String) {
      return (grants['secret'] ?? const []).contains(secret);
    }
    final provider = textOf(entry, 'provider');
    return (grants[provider] ?? const []).contains(textOf(entry, 'key'));
  }

  /// The value a reference resolves to, null when not granted or not held.
  String? secret(Document entry) {
    final provider = entry['provider'];
    final key = textOf(entry, 'key');
    if (provider == 'file' && !validRelativePath(key)) {
      throw AosError('invalid-reference', key);
    }
    if (!granted(entry)) return null;
    final name = entry['secret'];
    if (name is String) return secrets[name];
    final kind = provider is String ? provider : '';
    final held = providers;
    final has = held == null
        ? kind == 'env' || kind == 'file'
        : held.contains(kind);
    if (!has) return null;
    if (kind == 'env') {
      return Environment([
        for (final e in hostEnv.entries) (e.key, e.value),
      ], windows).get(key);
    }
    if (kind == 'file') {
      final value = secretFiles?[key];
      if (value == null) return null;
      return value.endsWith('\r\n')
          ? value.substring(0, value.length - 2)
          : value.endsWith('\n')
          ? value.substring(0, value.length - 1)
          : value;
    }
    return providerValues[kind]?[key];
  }
}

/// A mapping's text field, or the empty text.
String textOf(Object? entry, String key) {
  final value = entry is Map<String, Object?> ? entry[key] : null;
  return value is String ? value : '';
}
