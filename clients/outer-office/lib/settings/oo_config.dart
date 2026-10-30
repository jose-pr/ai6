import 'dart:convert';

import 'package:toml/toml.dart';
import 'package:yaml/yaml.dart';
import 'package:yaml_edit/yaml_edit.dart';

import '../routes/oo_route.dart';
import '../theme/oo_tokens.dart';

/// The formats an `OO.cfg` can be written in.
enum OoConfigFormat { toml, json, yaml }

/// Outer Office's own configuration, `OO.cfg`: where Universal Exports is, the deployment's
/// default theme, Moneypenny's chat sizes and the bounds of routes. The person's own choices
/// (`oo.theme`, the chat width they drag to) win over it.
///
/// A document names its format on its first line, `#!json`, `#!yaml` (`#!yml`) or `#!toml`
/// (the last word of the line, after any `/`, read without case or leading dots); a document
/// starting `%YAML` is YAML; anything else is TOML. Nothing is guessed from the content.
/// Unknown tables and keys are refused; `[assets]` belongs to the web page's bootstrap and is
/// left alone.
class OoConfig {
  const OoConfig({
    this.unexUrl,
    this.theme,
    this.chat = const ChatSettings(),
    this.routeTextBytes,
    this.limits = const OoLimits(),
  });

  /// `[unex] url`: Universal Exports' origin.
  final Uri? unexUrl;

  /// `[appearance] theme`: a theme id or `system`, the default under the person's choice.
  final String? theme;

  /// `[chat]`: Moneypenny's chat sizes and timing.
  final ChatSettings chat;

  /// `[routes] text_bytes`: the longest `text` filter a route may carry, in bytes; `null`
  /// keeps [OoRouteLimits.defaultTextBytes].
  final int? routeTextBytes;

  /// `[refresh]`, `[controls]`, `[pages]`, `[terminal]` and `[limits]`: how often pages ask
  /// again, how a control is followed, how much a page asks for, the terminal's pace and how
  /// much a field takes.
  final OoLimits limits;

  /// The route limits this file sets, over the defaults.
  OoRouteLimits get routeLimits => routeTextBytes == null
      ? const OoRouteLimits()
      : OoRouteLimits(textBytes: routeTextBytes!);

  /// The defaults: no origin, no theme, the default chat and the default route limits.
  static const none = OoConfig();

  /// Reads an `OO.cfg`'s text.
  ///
  /// Throws a [FormatException] naming the format, the table or the key that is wrong.
  factory OoConfig.parse(String text) {
    final document = decodeOoConfig(text);
    const known = {
      'unex',
      'appearance',
      'chat',
      'routes',
      'assets',
      'refresh',
      'controls',
      'pages',
      'terminal',
      'limits',
    };
    for (final key in document.keys) {
      if (!known.contains(key)) {
        throw FormatException('OO.cfg: unknown table [$key]');
      }
    }
    Map<String, Object?> table(String name, Set<String> keys) {
      final value = document[name];
      if (value == null) return const {};
      if (value is! Map) {
        throw FormatException('OO.cfg: [$name] is not a table');
      }
      final entries = value.cast<String, Object?>();
      for (final key in entries.keys) {
        if (!keys.contains(key)) {
          throw FormatException('OO.cfg: unknown key $name.$key');
        }
      }
      return entries;
    }

    String? string(Map<String, Object?> table, String name, String key) {
      final value = table[key];
      if (value == null) return null;
      if (value is! String) {
        throw FormatException('OO.cfg: $name.$key is not text');
      }
      return value;
    }

    double? number(Map<String, Object?> table, String key) {
      final value = table[key];
      if (value == null) return null;
      if (value is! num || value <= 0 || !value.isFinite) {
        throw FormatException('OO.cfg: chat.$key is not a positive number');
      }
      return value.toDouble();
    }

    final limits = OoLimits._read(table);
    final unex = table('unex', {'url'});
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
                'OO.cfg: chat.transition_ms is not a whole number of 0 or more',
              ),
            },
    );
    if (chat.minWidth > chat.maxWidth ||
        chat.width < chat.minWidth ||
        chat.width > chat.maxWidth) {
      throw FormatException(
        'OO.cfg: chat.width ${chat.width} is not between chat.min_width '
        '${chat.minWidth} and chat.max_width ${chat.maxWidth}',
      );
    }
    final appearance = table('appearance', {'theme'});
    final routes = table('routes', {'text_bytes'});
    final textBytes = routes['text_bytes'];
    if (textBytes != null && (textBytes is! int || textBytes < 1)) {
      throw const FormatException(
        'OO.cfg: routes.text_bytes is not a whole number of bytes, 1 or more',
      );
    }
    final url = string(unex, 'unex', 'url');
    final origin = url == null ? null : Uri.tryParse(url);
    if (url != null &&
        (origin == null || !origin.hasScheme || origin.host.isEmpty)) {
      throw FormatException('OO.cfg: unex.url $url is not an origin');
    }
    return OoConfig(
      unexUrl: origin,
      theme: string(appearance, 'appearance', 'theme'),
      chat: chat,
      routeTextBytes: textBytes as int?,
      limits: limits,
    );
  }

  /// The theme to start with when the person has chosen none: [theme] when it is a theme id
  /// or `system`; else Night Ops, with [warn] told of an unknown id.
  String defaultTheme({void Function(String message)? warn}) {
    final theme = this.theme;
    if (theme == null) return OoThemeId.nightOps.id;
    final valid = ooThemePreference(theme);
    if (valid != theme) {
      warn?.call(
        'OO.cfg: appearance.theme $theme is not a theme; using $valid',
      );
    }
    return valid;
  }
}

/// How long an `OO.cfg` value of seconds is, whole or fractional, in a [Duration].
Duration _seconds(double seconds) =>
    Duration(microseconds: (seconds * Duration.microsecondsPerSecond).round());

/// How often each page asks again, `OO.cfg`'s `[refresh]` table, in seconds; `0` turns a
/// page's automatic refresh off.
class RefreshSettings {
  const RefreshSettings({
    this.sessions = defaultSessions,
    this.overview = defaultOverview,
    this.timeline = defaultTimeline,
    this.pools = defaultPools,
    this.slow = defaultSlow,
    this.silentIntervals = defaultSilentIntervals,
  });

  static const defaultSessions = Duration(seconds: 3);
  static const defaultOverview = Duration(seconds: 10);
  static const defaultTimeline = Duration(seconds: 10);
  static const defaultPools = Duration(seconds: 15);
  static const defaultSlow = Duration(seconds: 15);
  static const defaultSilentIntervals = 3;

  static const keys = {
    'sessions',
    'overview',
    'timeline',
    'pools',
    'slow',
    'silent_intervals',
  };

  /// `sessions`: the board of Sessions, Stations and Missions.
  final Duration? sessions;

  /// `overview`: the overview's activity.
  final Duration? overview;

  /// `timeline`: the Operation timeline.
  final Duration? timeline;

  /// `pools`: the account pools.
  final Duration? pools;

  /// `slow`: the green ink requests and Mission proposals the board reads, which change
  /// slowly; `null` (0) reads them with every refresh.
  final Duration? slow;

  /// `silent_intervals`: a Station that has not checked in for this many refresh intervals
  /// (of `sessions`) is shown as silent.
  final int silentIntervals;
}

/// How a control is followed, `OO.cfg`'s `[controls]` table, in seconds.
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

/// How much a page asks for, `OO.cfg`'s `[pages]` table.
class PageSettings {
  const PageSettings({this.timeline = 50, this.audit = 100, this.summary = 5});

  static const keys = {'timeline', 'audit', 'summary'};

  /// `timeline`: items in one page of an Operation's timeline.
  final int timeline;

  /// `audit`: records in the audit log.
  final int audit;

  /// `summary`: items of recent activity an Operation's summary shows.
  final int summary;
}

/// The live terminal's pace, `OO.cfg`'s `[terminal]` table.
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

/// How much a text field takes, `OO.cfg`'s `[limits]` table, in characters; `0` takes the
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

/// Everything in `OO.cfg` that tunes how Outer Office asks and what it accepts; every value
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

  /// The tables of `OO.cfg`, [table] being its table reader.
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
          'OO.cfg: $name.$key is not a number of seconds, '
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
          'OO.cfg: $name.$key is not a whole number of $least or more',
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
        silentIntervals:
            whole(refreshTable, 'refresh', 'silent_intervals') ??
            refreshDefaults.silentIntervals,
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

/// Moneypenny's chat sizes and timing, `OO.cfg`'s `[chat]` table; every value defaults to
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
  });

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

  /// [width] kept between [minWidth] and [maxWidth].
  double clamp(double width) => width.clamp(minWidth, maxWidth);
}

/// The format `text` declares by its first line ([OoConfig]).
///
/// Throws a [FormatException] for a name no format answers to, listing json, toml and yaml.
OoConfigFormat declaredFormat(String text) {
  // Only the first 256 characters are looked at.
  final plain = _withoutBom(text);
  final head = plain.length > 256 ? plain.substring(0, 256) : plain;
  final firstLine = head.split('\n').first.trimRight();
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
      'toml' || 'cfg' => OoConfigFormat.toml,
      _ => throw FormatException(
        'OO.cfg: #!$name is not one of json, toml, yaml',
      ),
    };
  }
  if (head.startsWith('%YAML')) return OoConfigFormat.yaml;
  return OoConfigFormat.toml;
}

/// The document of an `OO.cfg`'s text, in the format its first line declares.
Map<String, Object?> decodeOoConfig(String text) {
  final format = declaredFormat(text);
  final body = _body(text);
  final Object? decoded;
  try {
    decoded = switch (format) {
      OoConfigFormat.json =>
        body.trim().isEmpty ? <String, Object?>{} : jsonDecode(body),
      OoConfigFormat.yaml => _plain(loadYaml(body)),
      OoConfigFormat.toml => TomlDocument.parse(body).toMap(),
    };
  } on Object catch (error) {
    throw FormatException('OO.cfg is not valid ${format.name}: $error');
  }
  if (decoded == null) return {};
  if (decoded is! Map) throw const FormatException('OO.cfg is not a table');
  return decoded.cast<String, Object?>();
}

/// `text` with `[unex] url` set to [url], in the format it declares, keeping its `#!` line.
/// TOML and JSON are written anew, so their comments are lost; YAML is edited in place.
String withUnexUrl(String text, Uri url) {
  final format = declaredFormat(text);
  final origin = url.toString();
  final firstLine = _withoutBom(text).split('\n').first;
  final header = firstLine.startsWith('#!') ? '$firstLine\n' : '';
  final body = _body(text);
  switch (format) {
    case OoConfigFormat.yaml:
      final editor = YamlEditor(body.trim().isEmpty ? '{}' : body);
      final unex = decodeOoConfig(text)['unex'];
      if (unex is Map) {
        editor.update(['unex', 'url'], origin);
      } else {
        editor.update(['unex'], {'url': origin});
      }
      return '$header${editor.toString()}';
    case OoConfigFormat.json:
      final document = Map<String, Object?>.of(decodeOoConfig(text));
      document['unex'] = {
        ...?(document['unex'] as Map?)?.cast<String, Object?>(),
        'url': origin,
      };
      return '$header${const JsonEncoder.withIndent('  ').convert(document)}\n';
    case OoConfigFormat.toml:
      final document = Map<String, Object?>.of(decodeOoConfig(text));
      document['unex'] = {
        ...?(document['unex'] as Map?)?.cast<String, Object?>(),
        'url': origin,
      };
      return '$header${TomlDocument.fromMap(document)}';
  }
}

String _withoutBom(String text) =>
    text.startsWith('﻿') ? text.substring(1) : text;

/// The text without its `#!` line, which no parser sees.
String _body(String text) {
  final plain = _withoutBom(text);
  if (!plain.startsWith('#!')) return plain;
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
