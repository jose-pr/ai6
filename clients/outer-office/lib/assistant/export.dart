import 'dart:convert';

import 'assistant_controller.dart';
import 'save_text_stub.dart'
    if (dart.library.io) 'save_text_io.dart'
    if (dart.library.js_interop) 'save_text_web.dart'
    as platform;

/// A conversation as a Markdown document: frontmatter naming it, then each entry under a
/// heading with who said it and when (UTC). Her words and the person's are kept as written;
/// a proposal, view or page keeps its data as a JSON block.
String conversationMarkdown({
  required String? conversationId,
  required List<AssistantEntry> entries,
  required DateTime exportedAt,
}) {
  final out = StringBuffer()
    ..writeln('---')
    ..writeln('conversation: ${jsonEncode(conversationId ?? 'not started')}')
    ..writeln('exported_at: ${exportedAt.toUtc().toIso8601String()}')
    ..writeln('entries: ${entries.length}')
    ..writeln('---');
  for (final entry in entries) {
    final when = DateTime.fromMillisecondsSinceEpoch(
      (entry.at * 1000).round(),
      isUtc: true,
    ).toIso8601String();
    final who = switch (entry.role) {
      'person' => 'You',
      'moneypenny' => 'Moneypenny',
      'tools' => 'Tools',
      final other => other[0].toUpperCase() + other.substring(1),
    };
    out
      ..writeln()
      ..writeln('## $who · $when')
      ..writeln();
    if (entry.text.isNotEmpty) out.writeln(entry.text);
    if (entry.data case final data? when data.isNotEmpty) {
      final json = const JsonEncoder.withIndent('  ').convert(data);
      // A fence longer than any backtick run in the data, so it cannot close early.
      final run = RegExp('`+')
          .allMatches(json)
          .fold(
            0,
            (most, match) =>
                match.end - match.start > most ? match.end - match.start : most,
          );
      final fence = '`' * (run < 3 ? 3 : run + 1);
      out
        ..writeln()
        ..writeln('${fence}json')
        ..writeln(json)
        ..writeln(fence);
    }
  }
  return out.toString();
}

/// The file name an export gets: `moneypenny-<conversation>.md`, safe on every platform.
String exportFileName(String? conversationId) {
  final id = (conversationId ?? 'conversation').replaceAll(
    RegExp(r'[^A-Za-z0-9_-]'),
    '-',
  );
  return 'moneypenny-$id.md';
}

/// Saves [text] as [name]: a download in a browser, a file in the Downloads folder on a desktop.
/// Returns where it went, or `null` when the browser chose.
Future<String?> saveText(String name, String text) =>
    platform.saveText(name, text);
