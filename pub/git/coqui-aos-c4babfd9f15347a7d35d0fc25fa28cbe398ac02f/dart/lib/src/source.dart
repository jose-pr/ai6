/// Sources: where a skill or an overlay comes from, `[git+]<location>[@<ref>][#<path>]`.
library;

import 'dart:math' as math;

import 'error.dart';
import 'names.dart';
import 'text.dart';

/// A parsed source. Its text form hides user information and query, so [toString] is safe to
/// log. [location] holds them: never print it.
class Source {
  /// Creates a source. [kind] is `directory`, `git` or `url`.
  const Source(this.kind, this.location, this.reference, this.path);

  /// Parses `[git+]<location>[@<ref>][#<path>]`, or refuses with `invalid-source`.
  factory Source.parse(String text) => _parseSource(text);

  /// What the location is: `directory`, `git` or `url`.
  final String kind;

  /// The location, user information and query included.
  final String location;

  /// The ref after `@`, for a Git source.
  final String? reference;

  /// The path after `#`.
  final String? path;

  /// The same source with another ref.
  Source withReference(String? reference) =>
      Source(kind, location, reference, path);

  /// The source as text for a person: no user information and no query.
  String shown() => shownWith(reference);

  /// [shown], with [reference] in place of the source's own.
  String shownWith(String? reference) {
    var shownLocation = location;
    final parts = _authorityOf(location);
    if (parts != null) {
      final at = parts.authority.lastIndexOf('@');
      final host = at == -1
          ? parts.authority
          : parts.authority.substring(at + 1);
      final rest = parts.rest.split('?').first;
      shownLocation = '${parts.scheme}://$host$rest';
    }
    final shown = StringBuffer(shownLocation);
    if (reference != null && reference != '') shown.write('@$reference');
    if (path != null) shown.write('#$path');
    return shown.toString();
  }

  /// What identifies the source, credentials and all: `location[@ref][#path]`. Never show it.
  String keyWith(String? reference) {
    final key = StringBuffer(location);
    final chosen = reference ?? this.reference;
    if (chosen != null && chosen != '') key.write('@$chosen');
    if (path != null) key.write('#$path');
    return key.toString();
  }

  /// Whether the source is on this machine or its network edge: a directory, `file:`, loopback
  /// or link-local.
  bool isLocal() {
    if (asciiLower(location).startsWith('file:') || kind == 'directory') {
      return true;
    }
    if (kind == 'git' && _localGit(location)) return true;
    return hostIsLocal(sourceHost(location));
  }

  /// Whether the location holds a password, a token, or a query.
  bool holdsCredentials() {
    final parts = _authorityOf(location);
    if (parts == null) return false;
    final at = parts.authority.lastIndexOf('@');
    final user = at == -1 ? '' : parts.authority.substring(0, at);
    final scheme = asciiLower(parts.scheme);
    return parts.rest.contains('?') ||
        user.contains(':') ||
        (user != '' && (scheme == 'http' || scheme == 'https'));
  }

  /// Whether the location is an `http://` or `https://` URL.
  bool isWebUrl() {
    final lower = asciiLower(location);
    return lower.startsWith('http://') || lower.startsWith('https://');
  }

  @override
  String toString() => shown();

  /// A form safe to log: the kind and [shown].
  Map<String, Object> toJson() => {'kind': kind, 'shown': shown()};
}

bool _isGitLocation(String location) =>
    location.endsWith('.git') ||
    const ['git@', 'ssh://', 'git://'].any(location.startsWith);

bool _isSourceControl(int code) =>
    code <= 0x1f ||
    (code >= 0x7f && code <= 0x9f) ||
    code == 0x2028 ||
    code == 0x2029 ||
    code == 0x200e ||
    code == 0x200f ||
    (code >= 0x202a && code <= 0x202e) ||
    (code >= 0x2066 && code <= 0x2069);

final RegExp _drive = RegExp(r'^[A-Za-z]:');

bool _hasDrive(String location) => _drive.hasMatch(location);

typedef _Authority = ({String scheme, String authority, String rest});

_Authority? _authorityOf(String location) {
  final at = location.indexOf('://');
  if (at == -1) return null;
  final scheme = location.substring(0, at);
  final rest = location.substring(at + 3);
  var end = rest.length;
  for (var i = 0; i < rest.length; i += 1) {
    if (rest[i] == '/' || rest[i] == '?') {
      end = i;
      break;
    }
  }
  return (
    scheme: scheme,
    authority: rest.substring(0, end),
    rest: rest.substring(end),
  );
}

({String host, String rest})? _hostAndPath(String location) {
  if (_hasDrive(location)) return null;
  final at = location.indexOf(':');
  if (at == -1) return null;
  final host = location.substring(0, at);
  if (host == '' || host.contains('/') || host.contains(r'\')) return null;
  return (host: host, rest: location.substring(at + 1));
}

final RegExp _escapedDash = RegExp(r'^%2[dD]');

/// Whether [text], with its `%XX` escapes decoded, starts with `-`.
bool _decodedStartsWithDash(String text) {
  final first = text.length < 3 ? text : text.substring(0, 3);
  if (_escapedDash.hasMatch(first)) return true;
  return text.startsWith('-');
}

(String, String)? _splitLast(String text, String separator) {
  final at = text.lastIndexOf(separator);
  return at == -1
      ? null
      : (text.substring(0, at), text.substring(at + separator.length));
}

final RegExp _refDelimiter = RegExp(r'[/:\\]');
final RegExp _slashEnds = RegExp(r'^/+|/+$');

Source _parseSource(String text) {
  AosError refuse(String why) => AosError('invalid-source', why);
  var raw = trimWhitespace(text);
  if (raw == '') throw refuse('empty');
  for (final code in raw.runes) {
    if (_isSourceControl(code)) throw refuse('control character');
  }
  final explicitGit = raw.startsWith('git+');
  if (explicitGit) raw = raw.substring('git+'.length);
  var body = raw;
  String? path;
  final hash = raw.indexOf('#');
  if (hash != -1) {
    body = raw.substring(0, hash);
    final trimmed = trimWhitespace(
      raw.substring(hash + 1),
    ).replaceAll(_slashEnds, '');
    path = trimmed == '' ? null : trimmed;
  }
  var location = trimWhitespace(body);
  String? reference;
  final split = _splitLast(location, '@');
  if (split != null) {
    final (head, tail) = split;
    final authority = _authorityOf(location);
    final afterAuthority = authority == null
        ? location.length
        : location.length - authority.rest.length;
    final inAuthority =
        location.contains('://') && head.length < afterAuthority;
    if (head != '' &&
        tail != '' &&
        !inAuthority &&
        !_refDelimiter.hasMatch(tail) &&
        !head.endsWith('://') &&
        !head.endsWith(':') &&
        (explicitGit || _isGitLocation(head))) {
      location = head;
      reference = tail;
    }
  }
  if (path != null && !validRelativePath(path)) {
    throw refuse('the #path is not a relative path');
  }
  var parts = <String>[];
  final authority = _authorityOf(location);
  if (authority != null) {
    if (authority.authority.contains(r'\')) {
      throw refuse('backslash in the authority');
    }
    final user = _splitLast(authority.authority, '@');
    final hostport = user == null ? authority.authority : user.$2;
    var host = hostport;
    var port = '';
    if (!hostport.endsWith(']')) {
      final pair = _splitLast(hostport, ':');
      if (pair != null) {
        host = pair.$1;
        port = pair.$2;
      }
    }
    parts = [user == null ? '' : user.$1, host, port];
  } else {
    final found = _hostAndPath(location);
    if (found != null) {
      final user = _splitLast(found.host, '@');
      parts = [
        user == null ? '' : user.$1,
        user == null ? found.host : user.$2,
        found.rest,
      ];
    }
  }
  if (parts.any(_decodedStartsWithDash)) {
    throw refuse('a user, host or port that reads as an option');
  }
  if (explicitGit || _isGitLocation(location)) {
    if (location.startsWith('-') ||
        (reference != null && reference.startsWith('-'))) {
      throw refuse('a location or ref that reads as an option');
    }
    if (location.contains('::')) throw refuse('a Git remote helper');
    final scheme = location.indexOf('://');
    if (scheme != -1 &&
        !const [
          'https',
          'http',
          'ssh',
          'git',
          'file',
        ].contains(asciiLower(location.substring(0, scheme)))) {
      throw refuse('a scheme Git is not given');
    }
    return Source('git', location, reference, path);
  }
  return Source(
    location.contains('://') ? 'url' : 'directory',
    location,
    null,
    path,
  );
}

bool _localGit(String location) {
  final scheme = location.indexOf('://');
  if (scheme != -1) return asciiLower(location.substring(0, scheme)) == 'file';
  return _hasDrive(location) || _hostAndPath(location) == null;
}

/// The host of a source's location, lowercased: a URL's, or an scp-like `host:path`'s.
String? sourceHost(String location) {
  final parts = _authorityOf(location);
  String hostport;
  if (parts != null) {
    final at = parts.authority.lastIndexOf('@');
    hostport = at == -1 ? parts.authority : parts.authority.substring(at + 1);
  } else {
    final found = _hostAndPath(location);
    if (found == null) return null;
    final at = found.host.lastIndexOf('@');
    final host = at == -1 ? found.host : found.host.substring(at + 1);
    return host == '' ? null : asciiLower(host);
  }
  if (hostport.startsWith('[')) {
    return asciiLower(hostport.substring(1).split(']').first);
  }
  final pair = _splitLast(hostport, ':');
  return asciiLower(pair == null ? hostport : pair.$1);
}

final RegExp _octet = RegExp(r'^(0|[1-9][0-9]{0,2})$');

/// An IPv4 address as four octets: dotted decimal, no leading zeros.
List<int>? parseIpv4(String text) {
  final parts = text.split('.');
  if (parts.length != 4) return null;
  final octets = <int>[];
  for (final part in parts) {
    if (!_octet.hasMatch(part)) return null;
    final octet = int.parse(part);
    if (octet > 255) return null;
    octets.add(octet);
  }
  return octets;
}

final RegExp _v6Chars = RegExp(r'^[0-9a-fA-F:.]+$');
final RegExp _v6Group = RegExp(r'^[0-9a-fA-F]{1,4}$');

/// An IPv6 address as eight 16-bit groups, with `::` and a trailing IPv4 part allowed.
List<int>? parseIpv6(String text) {
  if (!_v6Chars.hasMatch(text) || !text.contains(':')) return null;
  final halves = text.split('::');
  if (halves.length > 2) return null;
  List<int>? groups(String half) {
    if (half == '') return [];
    final out = <int>[];
    final items = half.split(':');
    for (var index = 0; index < items.length; index += 1) {
      final item = items[index];
      if (item.contains('.')) {
        final v4 = parseIpv4(item);
        if (v4 == null || index != items.length - 1) return null;
        out
          ..add((v4[0] << 8) | v4[1])
          ..add((v4[2] << 8) | v4[3]);
      } else {
        if (!_v6Group.hasMatch(item)) return null;
        out.add(int.parse(item, radix: 16));
      }
    }
    return out;
  }

  final head = groups(halves[0]);
  if (head == null) return null;
  if (halves.length == 1) return head.length == 8 ? head : null;
  final tail = groups(halves[1]);
  if (tail == null || head.length + tail.length > 7) return null;
  return [
    ...head,
    ...List<int>.filled(8 - head.length - tail.length, 0),
    ...tail,
  ];
}

/// One part of a shorthand IPv4 host: `0x` hexadecimal, a leading 0 octal, else decimal.
int? _ipv4Part(String part) {
  var digits = part;
  var radix = 10;
  if (part.length >= 2 &&
      part[0] == '0' &&
      (part[1] == 'x' || part[1] == 'X')) {
    digits = part.substring(2);
    radix = 16;
  } else if (part.length > 1 && part.startsWith('0')) {
    digits = part.substring(1);
    radix = 8;
  }
  final allowed = radix == 16
      ? RegExp(r'^[0-9a-fA-F]+$')
      : radix == 8
      ? RegExp(r'^[0-7]+$')
      : RegExp(r'^[0-9]+$');
  if (!allowed.hasMatch(digits)) return null;
  return int.tryParse(digits, radix: radix);
}

/// An IPv4 host the way a URL parser reads it: one to four parts, the last filling the remaining
/// bytes, so `127.1` and `2130706433` are loopback.
List<int>? _parseIpv4Shorthand(String host) {
  final parts = host.split('.');
  if (parts.length > 4) return null;
  final values = <int>[];
  for (final part in parts) {
    final number = _ipv4Part(part);
    if (number == null) return null;
    values.add(number);
  }
  final last = values.last;
  final lead = values.sublist(0, values.length - 1);
  if (lead.any((value) => value > 255) ||
      last >= math.pow(256, 5 - values.length)) {
    return null;
  }
  var address = last;
  for (var at = 0; at < lead.length; at += 1) {
    address += lead[at] * math.pow(256, 3 - at).toInt();
  }
  return [
    (address ~/ 16777216) % 256,
    (address ~/ 65536) % 256,
    (address ~/ 256) % 256,
    address % 256,
  ];
}

List<int>? _mappedV4(List<int> groups) {
  if (groups.take(5).every((group) => group == 0) && groups[5] == 0xffff) {
    return [groups[6] >> 8, groups[6] & 0xff, groups[7] >> 8, groups[7] & 0xff];
  }
  return null;
}

bool _localV4(List<int> v4) =>
    v4[0] == 127 ||
    (v4[0] == 169 && v4[1] == 254) ||
    (v4[0] == 0 && v4[1] == 0 && v4[2] == 0 && v4[3] == 0);

/// Whether a host is this machine: `localhost`, loopback, link-local or unspecified.
bool hostIsLocal(String? host) {
  if (host == null) return false;
  if (host == 'localhost' || host.endsWith('.localhost')) return true;
  final v4 = _parseIpv4Shorthand(host);
  if (v4 != null) return _localV4(v4);
  final g = parseIpv6(host);
  if (g == null) return false;
  final mapped = _mappedV4(g);
  if (mapped != null) return _localV4(mapped);
  final loopback = g.take(7).every((group) => group == 0) && g[7] == 1;
  final unspecified = g.every((group) => group == 0);
  return loopback || unspecified || (g[0] & 0xffc0) == 0xfe80;
}

bool _privateV4(List<int> v4) {
  final a = v4[0];
  final b = v4[1];
  final c = v4[2];
  return a == 0 ||
      a == 10 ||
      (a == 172 && b >= 16 && b < 32) ||
      (a == 192 && b == 168) ||
      (a == 100 && b >= 64 && b < 128) ||
      (a == 169 && b == 254) ||
      (a == 192 && b == 0 && c == 0) ||
      (a == 192 && b == 0 && c == 2) ||
      (a == 198 && (b == 18 || b == 19)) ||
      (a == 198 && b == 51 && c == 100) ||
      (a == 203 && b == 0 && c == 113) ||
      a >= 240;
}

/// Whether a host is on a private or reserved network, which is not this machine.
bool hostIsPrivate(String? host) {
  if (host == null || hostIsLocal(host)) return false;
  final v4 = _parseIpv4Shorthand(host);
  if (v4 != null) return _privateV4(v4);
  final s = parseIpv6(host);
  if (s == null) return false;
  final mapped = _mappedV4(s);
  if (mapped != null) return _privateV4(mapped);
  return (s[0] & 0xfe00) == 0xfc00 ||
      (s[0] == 0x2001 && s[1] == 0x0db8) ||
      (s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0) ||
      s[0] == 0x2002;
}
