/// What a `kb_search` answer must look like (`spec/v0/knowledge.md#search`): its shape and its
/// order. How a passage is found and ranked is the server's own and is not checked.
library;

import 'knowledge_core.dart';
import 'knowledge_serve.dart';
import 'text.dart';

num? _number(Object? value) {
  if (value is bool) return null;
  if (value is int) return value;
  if (value is double) return value;
  if (value is BigInt) {
    return value.isValidInt ? value.toInt() : value.toDouble();
  }
  return null;
}

BigInt? _whole(Object? value) {
  if (value is BigInt) return value;
  if (value is int) return BigInt.from(value);
  return null;
}

/// [passages] by score, highest first, then path as UTF-8 bytes, then first line, then last
/// line; passages equal in all four keep the order given.
List<Map<String, Object?>> searchOrder(List<Map<String, Object?>> passages) {
  final keyed = [
    for (var at = 0; at < passages.length; at += 1) (at, passages[at]),
  ];
  keyed.sort((a, b) {
    final left = a.$2;
    final right = b.$2;
    final byScore = _number(right['score'])!.compareTo(_number(left['score'])!);
    if (byScore != 0) return byScore;
    final byPath = compareText(
      left['path']! as String,
      right['path']! as String,
    );
    if (byPath != 0) return byPath;
    final byFrom = _whole(
      left['from_line'],
    )!.compareTo(_whole(right['from_line'])!);
    if (byFrom != 0) return byFrom;
    final byTo = _whole(left['to_line'])!.compareTo(_whole(right['to_line'])!);
    return byTo != 0 ? byTo : a.$1 - b.$1;
  });
  return [for (final (_, passage) in keyed) passage];
}

bool _sameKeys(Map<Object?, Object?> map, Iterable<String> keys) =>
    map.length == keys.length && keys.every(map.containsKey);

bool _within(
  Map<Object?, Object?> map,
  Iterable<String> need,
  Iterable<String> may,
) =>
    need.every(map.containsKey) &&
    map.keys.every((key) => need.contains(key) || may.contains(key));

/// The rules a `kb_search` [response] to [arguments] breaks, as sorted ids (`overlays`, `mode`,
/// `top`, `path-limit`, `not-knowledge`, `range`, `text`, `inert`, `shape`); empty when it
/// conforms. [modes] are the modes the server declares. The ranking itself is not checked, only
/// the shape and the order the scores imply.
List<String> searchCheck(
  List<KnowledgeOverlay> overlays,
  Object? arguments,
  Object? response,
  Iterable<String> modes, {
  KnowledgeSettings? settings,
  CallerFacts caller = const CallerFacts(),
  Iterable<String> engines = knowledgeEngines,
}) {
  final declared = modes.toList();
  final limit = settings ?? KnowledgeSettings();
  final args = knowledgeArguments('kb_search', arguments, declared, limit);
  final broken = <String>{};
  if (response is! Map || !_sameKeys(response, const ['results'])) {
    return ['shape'];
  }
  final chosen = knowledgeChosen(overlays, args['overlay'] as String?);
  final results = response['results'];
  if (results is! List) return ['shape'];
  final names = [
    for (final entry in results) entry is Map ? entry['overlay'] : null,
  ];
  final wanted = [for (final overlay in chosen) overlay.identity];
  if (names.length != wanted.length ||
      List.generate(
        names.length,
        (at) => names[at] == wanted[at],
      ).contains(false)) {
    broken.add('overlays');
  }
  final prefix = args['path'] as String?;
  final top = _whole(args['top'])!;
  final named = args['mode'];
  for (var at = 0; at < results.length && at < chosen.length; at += 1) {
    final entry = results[at];
    final overlay = chosen[at];
    if (entry is! Map) {
      broken.add('shape');
      continue;
    }
    if (entry.containsKey('error')) {
      final error = entry['error'];
      if (!_sameKeys(entry, const ['overlay', 'error']) ||
          error is! Map ||
          !_sameKeys(error, const ['class', 'message']) ||
          (error['class'] != 'unavailable' &&
              error['class'] != 'mode-unavailable')) {
        broken.add('shape');
      } else if (error['class'] == 'mode-unavailable' && named == 'auto') {
        broken.add('mode');
      }
      continue;
    }
    if (!_within(
      entry,
      const ['overlay', 'mode', 'passages'],
      const ['note'],
    )) {
      broken.add('shape');
      continue;
    }
    if (!declared.contains(entry['mode']) ||
        (named != 'auto' && entry['mode'] != named)) {
      broken.add('mode');
    }
    if (entry.containsKey('note')) {
      final note = entry['note'];
      if (note is! String || note != inert(note)) broken.add('inert');
    }
    final passages = entry['passages'];
    if (passages is! List) {
      broken.add('shape');
      continue;
    }
    if (BigInt.from(passages.length) > top) broken.add('top');
    final held = knowledgeSet(overlay, limit);
    for (final passage in passages) {
      if (passage is! Map ||
          !_sameKeys(passage, const [
            'path',
            'heading',
            'from_line',
            'to_line',
            'score',
            'text',
          ])) {
        broken.add('shape');
        continue;
      }
      final score = _number(passage['score']);
      if (score == null || score.isNaN) {
        broken.add('shape');
        continue;
      }
      final path = passage['path'];
      if (path is! String) {
        broken.add('not-knowledge');
        continue;
      }
      if (prefix != null && !(path == prefix || path.startsWith('$prefix/'))) {
        broken.add('path-limit');
      }
      final List<String> lines;
      try {
        lines = knowledgeLines(
          held.files.containsKey(path)
              ? knowledgeServe(
                  overlay,
                  held,
                  path,
                  const {},
                  caller,
                  limit,
                  knowledgeEngineList(engines),
                )
              : throw KnowledgeError('not-found'),
        );
      } on KnowledgeError {
        broken.add('not-knowledge');
        continue;
      }
      final first = _whole(passage['from_line']);
      final last = _whole(passage['to_line']);
      if (first == null ||
          last == null ||
          first < BigInt.one ||
          first > last ||
          last > BigInt.from(lines.length)) {
        broken.add('range');
        continue;
      }
      final span = inert(
        lines.sublist(first.toInt() - 1, last.toInt()).join('\n'),
      );
      final text = passage['text'];
      if (text is! String || text.isEmpty || !span.contains(text)) {
        broken.add('text');
      }
      for (final field in const ['heading', 'text']) {
        final value = passage[field];
        if (value is! String || value != inert(value)) broken.add('inert');
      }
    }
    final keyed = [
      for (final passage in passages)
        if (passage is Map &&
            _number(passage['score']) != null &&
            passage['path'] is String &&
            _whole(passage['from_line']) != null &&
            _whole(passage['to_line']) != null)
          passage.cast<String, Object?>(),
    ];
    if (keyed.length == passages.length) {
      final ordered = searchOrder(keyed);
      for (var index = 0; index < keyed.length; index += 1) {
        if (!identical(ordered[index], keyed[index])) {
          broken.add('order');
          break;
        }
      }
    }
  }
  return broken.toList()..sort();
}
