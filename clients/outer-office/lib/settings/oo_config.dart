import 'dart:convert';

import 'package:toml/toml.dart';
import 'package:yaml/yaml.dart';
import 'package:yaml_edit/yaml_edit.dart';

import '../routes/oo_route.dart';
import '../theme/oo_art.dart';
import '../theme/oo_tokens.dart';

/// The formats an `ai6.cfg` can be written in.
enum OoConfigFormat { toml, json, yaml }

/// What Outer Office reads of `ai6.cfg`, the one bootstrap file of a host or a user: where
/// Universal Exports is, and `[oo]`: the deployment's default theme, Moneypenny's chat sizes and
/// the bounds of routes. The person's own choices (`oo.theme`, the chat width they drag to) win
/// over it. The file's other sections are other programs', and are left alone.
///
/// A file's extension names its format; a `.cfg` file, or text with no name, names it on its first
/// line, `#!json`, `#!yaml` (`#!yml`) or `#!toml` (the last word of the line, after any `/`, read
/// without case or leading dots); a document starting `%YAML` is YAML; anything else is TOML.
/// Nothing is guessed from the content. Unknown sections, and unknown tables and keys of `[oo]`,
/// are refused, by name and never quoting a value; `[oo.assets]` belongs to the web page's bootstrap
/// and is left alone.
class OoConfig {
  const OoConfig({
    this.unexUrl,
    this.caFile,
    this.theme,
    this.chat = const ChatSettings(),
    this.routeTextBytes,
    this.limits = const OoLimits(),
    this.art = const OoArtLimits(),
  });

  /// `[unex] url`: Universal Exports' origin.
  final Uri? unexUrl;

  /// `[unex] ca_file`: a PEM file of more certificate authorities to trust for Universal
  /// Exports, relative to the file that names it. The web has its browser's.
  final String? caFile;

  /// `[oo.appearance] theme`: a theme id or `system`, the default under the person's choice.
  final String? theme;

  /// `[oo.chat]`: Moneypenny's chat sizes and timing.
  final ChatSettings chat;

  /// `[oo.routes] text_bytes`: the longest `text` filter a route may carry, in bytes; `null`
  /// keeps [OoRouteLimits.defaultTextBytes].
  final int? routeTextBytes;

  /// `[oo.art]`: how large a hero picture may be fetched.
  final OoArtLimits art;

  /// `[oo.refresh]`, `.controls`, `.pages`, `.terminal` and `.limits`: how often pages ask
  /// again, how a control is followed, how much a page asks for, the terminal's pace and how
  /// much a field takes.
  final OoLimits limits;

  /// The route limits this file sets, over the defaults.
  OoRouteLimits get routeLimits => routeTextBytes == null
      ? const OoRouteLimits()
      : OoRouteLimits(textBytes: routeTextBytes!);

  /// The defaults: no origin, no theme, the default chat and the default route limits.
  static const none = OoConfig();

  /// Reads an `ai6.cfg`'s text. [path] is the file's, when it has one: its extension may fix the
  /// format.
  ///
  /// Throws a [FormatException] naming the format, the table or the key that is wrong, and never
  /// a value: the file may hold another program's secret.
  factory OoConfig.parse(String text, {String? path}) {
    final document = decodeOoConfig(text, path: path);
    const sections = {'provider', 'registry', 'unex', 'station', 'oo'};
    for (final key in document.keys) {
      if (!sections.contains(key)) {
        throw FormatException('ai6.cfg: unknown section [$key]');
      }
    }
    Map<String, Object?> section(String name) {
      final value = document[name];
      if (value == null) return const {};
      if (value is! Map) {
        throw FormatException('ai6.cfg: [$name] is not a table');
      }
      return value.cast<String, Object?>();
    }

    final oo = section('oo');
    const known = {
      'appearance',
      'chat',
      'routes',
      'art',
      'assets',
      'refresh',
      'controls',
      'pages',
      'terminal',
      'limits',
    };
    for (final key in oo.keys) {
      if (!known.contains(key)) {
        throw FormatException('ai6.cfg: unknown table [oo.$key]');
      }
    }
    Map<String, Object?> table(String name, Set<String> keys) {
      final value = oo[name];
      if (value == null) return const {};
      if (value is! Map) {
        throw FormatException('ai6.cfg: [oo.$name] is not a table');
      }
      final entries = value.cast<String, Object?>();
      for (final key in entries.keys) {
        if (!keys.contains(key)) {
          throw FormatException('ai6.cfg: unknown key oo.$name.$key');
        }
      }
      return entries;
    }

    String? string(Map<String, Object?> table, String name, String key) {
      final value = table[key];
      if (value == null) return null;
      if (value is! String) {
        throw FormatException('ai6.cfg: $name.$key is not text');
      }
      return value;
    }

    double? number(Map<String, Object?> table, String key) {
      final value = table[key];
      if (value == null) return null;
      if (value is! num || value <= 0 || !value.isFinite) {
        throw FormatException('ai6.cfg: oo.chat.$key is not a positive number');
      }
      return value.toDouble();
    }

    final limits = OoLimits._read(table);
    final unex = section('unex');
    final chatTable = table('chat', ChatSettings.keys);
    const defaults = ChatSettings();
    final chat = ChatSettings(
      minWidth: number(chatTable, 'min_width') ?? defaults.minWidth,
      maxWidth: number(chatTable, 'max_width') ?? defaults.maxWidth,
      width: number(chatTable, 'width') ?? defaults.width,
      widthStep: number(chatTable, 'width_step') ?? defaults.widthStep,
      minPageWidth:
          number(chatTable, 'min_page_width') ?? defaults.minPageWidth,
      textWidth: number(chatTable, 'text_width') ?? defaults.textWidth,
      windowWidth: number(chatTable, 'window_width') ?? defaults.windowWidth,
      windowHeight: number(chatTable, 'window_height') ?? defaults.windowHeight,
      transitionMs: chatTable['transition_ms'] == null
          ? defaults.transitionMs
          : switch (chatTable['transition_ms']) {
              final int ms when ms >= 0 => ms,
              _ => throw const FormatException(
                'ai6.cfg: oo.chat.transition_ms is not a whole number of 0 or more',
              ),
            },
      connectTimeout: number(chatTable, 'connect_timeout') == null
          ? defaults.connectTimeout
          : _seconds(number(chatTable, 'connect_timeout')!),
    );
    if (chat.minWidth > chat.maxWidth ||
        chat.width < chat.minWidth ||
        chat.width > chat.maxWidth) {
      throw FormatException(
        'ai6.cfg: oo.chat.width ${chat.width} is not between chat.min_width '
        '${chat.minWidth} and chat.max_width ${chat.maxWidth}',
      );
    }
    final appearance = table('appearance', {'theme'});
    final routes = table('routes', {'text_bytes'});
    final textBytes = routes['text_bytes'];
    if (textBytes != null && (textBytes is! int || textBytes < 1)) {
      throw const FormatException(
        'ai6.cfg: oo.routes.text_bytes is not a whole number of bytes, 1 or more',
      );
    }
    final maxWidth = table('art', {'max_width'})['max_width'];
    if (maxWidth != null && (maxWidth is! int || maxWidth < 1)) {
      throw const FormatException(
        'ai6.cfg: oo.art.max_width is not a whole number of pixels, 1 or more',
      );
    }
    final url = string(unex, 'unex', 'url');
    final origin = url == null ? null : Uri.tryParse(url);
    if (url != null &&
        (origin == null || !origin.hasScheme || origin.host.isEmpty)) {
      throw const FormatException('ai6.cfg: unex.url is not an origin');
    }
    return OoConfig(
      unexUrl: origin,
      caFile: string(unex, 'unex', 'ca_file'),
      theme: string(appearance, 'oo.appearance', 'theme'),
      chat: chat,
      routeTextBytes: textBytes as int?,
      limits: limits,
      art: maxWidth == null
          ? const OoArtLimits()
          : OoArtLimits(maxWidth: maxWidth as int),
    );
  }

  /// The theme to start with when the person has chosen none: [theme] when it is a theme id
  /// or `system`; else Night Ops, with [warn] told of an unknown id.
  String defaultTheme({void Function(String message)? warn}) {
    final theme = this.theme;
    if (theme == null) return OoThemeId.nightOps.id;
    final valid = ooThemePreference(theme);
    if (valid != theme) {
      warn?.call('ai6.cfg: oo.appearance.theme is not a theme; using $valid');
    }
    return valid;
  }
}

/// How long an `ai6.cfg` value of seconds is, whole or fractional, in a [Duration].
Duration _seconds(double seconds) =>
    Duration(microseconds: (seconds * Duration.microsecondsPerSecond).round());

/// How often each page asks again, `ai6.cfg`'s `[oo.refresh]` table, in seconds; `0` turns a
/// page's automatic refresh off.
class RefreshSettings {
  const RefreshSettings({
    this.sessions = defaultSessions,
    this.overview = defaultOverview,
    this.timeline = defaultTimeline,
    this.pools = defaultPools,
    this.slow = defaultSlow,
    this.profile = defaultProfile,
    this.hidden = defaultHidden,
    this.silentIntervals = defaultSilentIntervals,
    this.renewBefore = defaultRenewBefore,
    this.renewRetry = defaultRenewRetry,
  });

  static const defaultSessions = Duration(seconds: 3);
  static const defaultOverview = Duration(seconds: 10);
  static const defaultTimeline = Duration(seconds: 10);
  static const defaultPools = Duration(seconds: 15);
  static const defaultSlow = Duration(seconds: 15);
  static const defaultProfile = Duration(seconds: 60);
  static const defaultHidden = Duration(seconds: 60);
  static const defaultSilentIntervals = 3;
  static const defaultRenewBefore = Duration(seconds: 600);
  static const defaultRenewRetry = Duration(seconds: 30);

  static const keys = {
    'sessions',
    'overview',
    'timeline',
    'pools',
    'slow',
    'profile',
    'hidden',
    'silent_intervals',
    'renew_before',
    'renew_retry',
  };

  /// `sessions`: what the page shown displays from the board of Sessions, Stations and
  /// Missions.
  final Duration? sessions;

  /// `overview`: the overview's activity.
  final Duration? overview;

  /// `timeline`: the Operation timeline.
  final Duration? timeline;

  /// `pools`: the account pools.
  final Duration? pools;

  /// `slow`: what the page shown does not display but the bell and the search need: Sessions,
  /// Stations, held briefs, green ink requests and Mission proposals; `null` (0) reads them
  /// with every refresh.
  final Duration? slow;

  /// `profile`: who the person is and what they may do, which changes rarely; `null` (0) reads
  /// it only at sign-in and when the operator menu opens.
  final Duration? profile;

  /// `hidden`: everything, while the tab or window cannot be seen; `null` (0) reads nothing
  /// until it can.
  final Duration? hidden;

  /// `silent_intervals`: a Station that has not checked in for this many refresh intervals
  /// (of `sessions`) is shown as silent.
  final int silentIntervals;

  /// `renew_before`: how long before the access token ends the app renews it, and before the
  /// sign-in itself ends the person is told; `null` (0) does neither, and the sign-in ends with
  /// the token. A shorter lifetime than this is renewed halfway through.
  final Duration? renewBefore;

  /// `renew_retry`: how soon a renewal that failed is tried again when [sessions] is off; else
  /// it is tried again at [sessions].
  final Duration renewRetry;
}

/// How a control is followed, `ai6.cfg`'s `[oo.controls]` table, in seconds.
class ControlSettings {
  const ControlSettings({
    this.wait = defaultWait,
    this.poll = defaultPoll,
    this.confirming = defaultConfirming,
  });

  static const defaultWait = Duration(seconds: 10);
  static const defaultPoll = Duration(milliseconds: 500);
  static const defaultConfirming = Duration(seconds: 30);

  static const keys = {'wait', 'poll', 'confirming'};

  /// `wait`: how long to follow a control for the Station's outcome; `0` returns once it is
  /// queued.
  final Duration wait;

  /// `poll`: how often to ask for its status meanwhile.
  final Duration poll;

  /// `confirming`: how long a proposal of Moneypenny's may stay "applying" before its card
  /// says the outcome is unknown.
  final Duration confirming;
}

/// How much a page asks for, `ai6.cfg`'s `[oo.pages]` table.
class PageSettings {
  const PageSettings({
    this.timeline = 50,
    this.audit = 100,
    this.summary = 5,
    this.search = 24,
  });

  static const keys = {'timeline', 'audit', 'summary', 'search'};

  /// `timeline`: items in one page of an Operation's timeline.
  final int timeline;

  /// `audit`: records in the audit log.
  final int audit;

  /// `summary`: items of recent activity, Operations and Stations a summary lists.
  final int summary;

  /// `search`: results the top bar's search lists at once.
  final int search;
}

/// The live terminal's pace, `ai6.cfg`'s `[oo.terminal]` table.
class TerminalSettings {
  const TerminalSettings({
    this.scrollbackLines = 10000,
    this.connectTimeout = const Duration(seconds: 10),
    this.retryDelay = const Duration(seconds: 1),
    this.retryMaxDelay = const Duration(seconds: 16),
  });

  static const keys = {
    'scrollback_lines',
    'connect_timeout',
    'retry_delay',
    'retry_max_delay',
  };

  /// `scrollback_lines`: lines the terminal keeps.
  final int scrollbackLines;

  /// `connect_timeout`: how long the socket may take to open.
  final Duration connectTimeout;

  /// `retry_delay`: the wait before the first reconnect; each next one doubles, up to
  /// [retryMaxDelay].
  final Duration retryDelay;

  /// `retry_max_delay`: the longest wait between reconnects.
  final Duration retryMaxDelay;
}

/// How much a text field takes, `ai6.cfg`'s `[oo.limits]` table, in characters; `0` takes the
/// bound away.
class InputSettings {
  const InputSettings({
    this.missionTitleChars = 512,
    this.missionPromptChars = 65536,
    this.reasonChars = 512,
  });

  static const keys = {
    'mission_title_chars',
    'mission_prompt_chars',
    'reason_chars',
  };

  /// `mission_title_chars`: a new Mission's title.
  final int missionTitleChars;

  /// `mission_prompt_chars`: a new Mission's instructions.
  final int missionPromptChars;

  /// `reason_chars`: the reason for lowering an Agent's clearance.
  final int reasonChars;

  /// A text field's `maxLength` for a bound: none when it is `0`.
  static int? field(int chars) => chars == 0 ? null : chars;
}

/// Everything in `ai6.cfg`'s `[oo]` that tunes how Outer Office asks and what it accepts; every value
/// defaults to the one here.
class OoLimits {
  const OoLimits({
    this.refresh = const RefreshSettings(),
    this.controls = const ControlSettings(),
    this.pages = const PageSettings(),
    this.terminal = const TerminalSettings(),
    this.inputs = const InputSettings(),
  });

  final RefreshSettings refresh;
  final ControlSettings controls;
  final PageSettings pages;
  final TerminalSettings terminal;
  final InputSettings inputs;

  /// The tables of `[oo]`, [table] being its table reader.
  factory OoLimits._read(
    Map<String, Object?> Function(String name, Set<String> keys) table,
  ) {
    double? seconds(
      Map<String, Object?> entries,
      String name,
      String key, {
      bool zero = false,
    }) {
      final value = entries[key];
      if (value == null) return null;
      if (value is! num ||
          !value.isFinite ||
          value < 0 ||
          (!zero && value == 0)) {
        throw FormatException(
          'ai6.cfg: oo.$name.$key is not a number of seconds, '
          '${zero ? '0' : 'more than 0'} or more',
        );
      }
      return value.toDouble();
    }

    int? whole(
      Map<String, Object?> entries,
      String name,
      String key, {
      int least = 1,
    }) {
      final value = entries[key];
      if (value == null) return null;
      if (value is! int || value < least) {
        throw FormatException(
          'ai6.cfg: oo.$name.$key is not a whole number of $least or more',
        );
      }
      return value;
    }

    Duration? refresh(Map<String, Object?> entries, String key, Duration base) {
      final value = seconds(entries, 'refresh', key, zero: true);
      if (value == null) return base;
      return value == 0 ? null : _seconds(value);
    }

    const refreshDefaults = RefreshSettings();
    const controlDefaults = ControlSettings();
    const pageDefaults = PageSettings();
    const terminalDefaults = TerminalSettings();
    const inputDefaults = InputSettings();
    final refreshTable = table('refresh', RefreshSettings.keys);
    final controlsTable = table('controls', ControlSettings.keys);
    final pagesTable = table('pages', PageSettings.keys);
    final terminalTable = table('terminal', TerminalSettings.keys);
    final inputsTable = table('limits', InputSettings.keys);
    final renewRetry = seconds(refreshTable, 'refresh', 'renew_retry');
    final wait = seconds(controlsTable, 'controls', 'wait', zero: true);
    final poll = seconds(controlsTable, 'controls', 'poll');
    final confirming = seconds(controlsTable, 'controls', 'confirming');
    final connect = seconds(terminalTable, 'terminal', 'connect_timeout');
    final retry = seconds(terminalTable, 'terminal', 'retry_delay');
    final retryMax = seconds(terminalTable, 'terminal', 'retry_max_delay');
    return OoLimits(
      refresh: RefreshSettings(
        sessions: refresh(refreshTable, 'sessions', refreshDefaults.sessions!),
        overview: refresh(refreshTable, 'overview', refreshDefaults.overview!),
        timeline: refresh(refreshTable, 'timeline', refreshDefaults.timeline!),
        pools: refresh(refreshTable, 'pools', refreshDefaults.pools!),
        slow: refresh(refreshTable, 'slow', refreshDefaults.slow!),
        profile: refresh(refreshTable, 'profile', refreshDefaults.profile!),
        hidden: refresh(refreshTable, 'hidden', refreshDefaults.hidden!),
        silentIntervals:
            whole(refreshTable, 'refresh', 'silent_intervals') ??
            refreshDefaults.silentIntervals,
        renewBefore: refresh(
          refreshTable,
          'renew_before',
          RefreshSettings.defaultRenewBefore,
        ),
        renewRetry: renewRetry == null
            ? refreshDefaults.renewRetry
            : _seconds(renewRetry),
      ),
      controls: ControlSettings(
        wait: wait == null ? controlDefaults.wait : _seconds(wait),
        poll: poll == null ? controlDefaults.poll : _seconds(poll),
        confirming: confirming == null
            ? controlDefaults.confirming
            : _seconds(confirming),
      ),
      pages: PageSettings(
        timeline:
            whole(pagesTable, 'pages', 'timeline') ?? pageDefaults.timeline,
        audit: whole(pagesTable, 'pages', 'audit') ?? pageDefaults.audit,
        summary: whole(pagesTable, 'pages', 'summary') ?? pageDefaults.summary,
        search: whole(pagesTable, 'pages', 'search') ?? pageDefaults.search,
      ),
      terminal: TerminalSettings(
        scrollbackLines:
            whole(terminalTable, 'terminal', 'scrollback_lines') ??
            terminalDefaults.scrollbackLines,
        connectTimeout: connect == null
            ? terminalDefaults.connectTimeout
            : _seconds(connect),
        retryDelay: retry == null
            ? terminalDefaults.retryDelay
            : _seconds(retry),
        retryMaxDelay: retryMax == null
            ? terminalDefaults.retryMaxDelay
            : _seconds(retryMax),
      ),
      inputs: InputSettings(
        missionTitleChars:
            whole(inputsTable, 'limits', 'mission_title_chars', least: 0) ??
            inputDefaults.missionTitleChars,
        missionPromptChars:
            whole(inputsTable, 'limits', 'mission_prompt_chars', least: 0) ??
            inputDefaults.missionPromptChars,
        reasonChars:
            whole(inputsTable, 'limits', 'reason_chars', least: 0) ??
            inputDefaults.reasonChars,
      ),
    );
  }
}

/// Moneypenny's chat sizes and timing, `ai6.cfg`'s `[oo.chat]` table; every value defaults to
/// the one here. Widths are logical pixels.
class ChatSettings {
  const ChatSettings({
    this.minWidth = 320,
    this.maxWidth = 720,
    this.width = 400,
    this.widthStep = 16,
    this.minPageWidth = 360,
    this.textWidth = 960,
    this.windowWidth = 520,
    this.windowHeight = 760,
    this.transitionMs = 200,
    this.connectTimeout = defaultConnectTimeout,
  });

  static const defaultConnectTimeout = Duration(seconds: 10);

  /// The table's keys, in the order above.
  static const keys = {
    'min_width',
    'max_width',
    'width',
    'width_step',
    'min_page_width',
    'text_width',
    'window_width',
    'window_height',
    'transition_ms',
    'connect_timeout',
  };

  /// `min_width`, `max_width`: how narrow and wide the docked chat may be dragged.
  final double minWidth;
  final double maxWidth;

  /// `width`: the docked chat's width until the person drags it.
  final double width;

  /// `width_step`: how far an arrow key moves the drag handle.
  final double widthStep;

  /// `min_page_width`: the page keeps at least this much beside the docked chat.
  final double minPageWidth;

  /// `text_width`: the widest column her text keeps to when expanded.
  final double textWidth;

  /// `window_width`, `window_height`: the size her own window opens at (desktop).
  final double windowWidth;
  final double windowHeight;

  /// `transition_ms`: how long the chat takes to move; none when the system asks for no
  /// animations.
  final int transitionMs;

  /// `connect_timeout`: how long, in seconds, her socket may take to say it is ready before
  /// the chat says it failed and offers to try again.
  final Duration connectTimeout;

  /// [width] kept between [minWidth] and [maxWidth].
  double clamp(double width) => width.clamp(minWidth, maxWidth);
}

/// The format a file named [path] fixes by its extension; `null` when its first line may say.
///
/// Throws a [FormatException] for an extension no format takes, as the Rust programs do.
OoConfigFormat? _fixedFormat(String? path) {
  if (path == null) return null;
  final name = path.split(RegExp(r'[/\\]')).last;
  final dot = name.lastIndexOf('.');
  final extension = dot < 1 ? '' : name.substring(dot + 1).toLowerCase();
  return switch (extension) {
    'cfg' => null,
    'toml' => OoConfigFormat.toml,
    'json' => OoConfigFormat.json,
    'yaml' || 'yml' => OoConfigFormat.yaml,
    _ => throw const FormatException(
      'ai6.cfg: the extension must be one of cfg, toml, json, yaml, yml',
    ),
  };
}

/// The format of `text`, a file named [path] or one with no name ([OoConfig]).
///
/// Throws a [FormatException] for a name no format answers to, listing json, toml and yaml.
OoConfigFormat declaredFormat(String text, {String? path}) {
  final fixed = _fixedFormat(path);
  if (fixed != null) return fixed;
  final plain = _withoutBom(text);
  final firstLine = plain.split('\n').first.trimRight();
  if (firstLine.startsWith('#!')) {
    final words = firstLine.substring(2).trim().split(RegExp(r'\s+'));
    var name = words.isEmpty ? '' : words.last;
    name = name
        .split(RegExp(r'[/\\]'))
        .last
        .toLowerCase()
        .replaceFirst(RegExp(r'^\.+'), '');
    return switch (name) {
      'json' => OoConfigFormat.json,
      'yaml' || 'yml' => OoConfigFormat.yaml,
      'toml' => OoConfigFormat.toml,
      _ => throw const FormatException(
        'ai6.cfg: the first line names no format; one of json, toml, yaml',
      ),
    };
  }
  if (plain.startsWith('%YAML')) return OoConfigFormat.yaml;
  return OoConfigFormat.toml;
}

/// The document of an `ai6.cfg`'s text, in the format it declares ([declaredFormat]).
///
/// Throws a [FormatException] saying where a parser stopped: a parser's own text may quote the
/// file, which may hold another program's secret, so it is dropped.
Map<String, Object?> decodeOoConfig(String text, {String? path}) {
  final format = declaredFormat(text, path: path);
  final body = _body(text, path);
  final Object? decoded;
  try {
    decoded = switch (format) {
      OoConfigFormat.json =>
        body.trim().isEmpty ? <String, Object?>{} : jsonDecode(body),
      OoConfigFormat.yaml => _plain(loadYaml(body)),
      OoConfigFormat.toml => TomlDocument.parse(body).toMap(),
    };
  } on FormatException catch (error) {
    throw FormatException(
      'ai6.cfg is not valid ${format.name}${_where(text, body, error.offset)}',
    );
  } on Object {
    throw FormatException('ai6.cfg is not valid ${format.name}');
  }
  if (decoded == null) return {};
  if (decoded is! Map) throw const FormatException('ai6.cfg is not a table');
  return decoded.cast<String, Object?>();
}

/// ` at line L, column C` of the file for an offset into its [body], counting a removed first line.
String _where(String text, String body, int? offset) {
  if (offset == null || offset > body.length) return '';
  final before = body.substring(0, offset);
  final removed = _withoutBom(text).length == body.length ? 0 : 1;
  final line = '\n'.allMatches(before).length + 1 + removed;
  final column = offset - (before.lastIndexOf('\n') + 1) + 1;
  return ' at line $line, column $column';
}

/// `text` with `[unex] url` set to [url], in the format it declares, keeping its `#!` line and
/// every other program's section. TOML and YAML are edited in place, so their comments stay; JSON
/// is written anew and has none.
///
/// Throws a [FormatException], rather than risk another program's settings, when the edit would
/// change anything else; such a file is edited by hand.
String withUnexUrl(String text, Uri url, {String? path}) {
  final format = declaredFormat(text, path: path);
  final origin = url.toString();
  final fixed = _fixedFormat(path) != null;
  final firstLine = _withoutBom(text).split('\n').first;
  final header = !fixed && firstLine.startsWith('#!') ? '$firstLine\n' : '';
  final body = _body(text, path);
  final before = decodeOoConfig(text, path: path);
  final String written;
  switch (format) {
    case OoConfigFormat.yaml:
      final editor = YamlEditor(body.trim().isEmpty ? '{}' : body);
      final unex = before['unex'];
      if (unex is Map) {
        editor.update(['unex', 'url'], origin);
      } else {
        editor.update(['unex'], {'url': origin});
      }
      written = '$header${editor.toString()}';
    case OoConfigFormat.json:
      final document = Map<String, Object?>.of(before);
      document['unex'] = {
        ...?(document['unex'] as Map?)?.cast<String, Object?>(),
        'url': origin,
      };
      written =
          '$header${const JsonEncoder.withIndent('  ').convert(document)}\n';
    case OoConfigFormat.toml:
      written = '$header${_tomlWithUnexUrl(body, origin)}';
  }
  Object? without(Map<String, Object?> document) {
    final unex = {...?(document['unex'] as Map?)?.cast<String, Object?>()};
    unex.remove('url');
    return jsonEncode({...document, 'unex': unex});
  }

  final after = decodeOoConfig(written, path: path);
  if (without(before) != without(after) ||
      (after['unex'] as Map?)?['url'] != origin) {
    throw const FormatException(
      'ai6.cfg: the address cannot be written here; edit the file by hand',
    );
  }
  return written;
}

/// The TOML [body] with `url` of its `[unex]` table replaced or added, every other line untouched.
String _tomlWithUnexUrl(String body, String origin) {
  final line = 'url = ${jsonEncode(origin)}';
  final lines = body.split('\n');
  final newline = body.contains('\r\n') ? '\r' : '';
  final header = RegExp(r'^\s*\[\s*unex\s*\]\s*(#.*)?\r?$');
  final at = lines.indexWhere(header.hasMatch);
  if (at < 0) {
    final kept = body.isEmpty || body.endsWith('\n') ? body : '$body\n';
    return '$kept${kept.isEmpty ? '' : '\n'}[unex]\n$line\n';
  }
  for (var index = at + 1; index < lines.length; index++) {
    if (RegExp(r'^\s*\[').hasMatch(lines[index])) break;
    if (RegExp(r'^\s*url\s*=').hasMatch(lines[index])) {
      lines[index] = '$line$newline';
      return lines.join('\n');
    }
  }
  lines.insert(at + 1, '$line$newline');
  return lines.join('\n');
}

String _withoutBom(String text) =>
    text.startsWith('﻿') ? text.substring(1) : text;

/// The text without its `#!` line, which no parser sees; a fixed extension does not read one.
String _body(String text, String? path) {
  final plain = _withoutBom(text);
  if (_fixedFormat(path) != null || !plain.startsWith('#!')) return plain;
  final end = plain.indexOf('\n');
  return end < 0 ? '' : plain.substring(end + 1);
}

/// A YAML node as plain maps, lists and scalars.
Object? _plain(Object? node) => switch (node) {
  YamlMap map => {
    for (final entry in map.entries) '${entry.key}': _plain(entry.value),
  },
  YamlList list => [for (final item in list) _plain(item)],
  _ => node,
};
