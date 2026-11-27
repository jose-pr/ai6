enum DiffKind { same, added, removed }

final class DiffLine {
  const DiffLine(this.kind, this.text);
  final DiffKind kind;
  final String text;
}

/// Past this many lines between the common start and end, a diff is shown as all old then all new: the table below grows as the
/// product of the two, and this runs on the UI thread.
const int _maxCompared = 1500;

List<String> _lines(String text) => text.isEmpty ? const [] : text.split('\n');

/// A line diff. The common start and end are taken off first, since an edit usually touches little of a file.
List<DiffLine> diffLines(String? before, String after) {
  final old = _lines(before ?? '');
  final now = _lines(after);
  var start = 0;
  while (start < old.length && start < now.length && old[start] == now[start]) {
    start++;
  }
  var oldEnd = old.length;
  var newEnd = now.length;
  while (oldEnd > start && newEnd > start && old[oldEnd - 1] == now[newEnd - 1]) {
    oldEnd--;
    newEnd--;
  }
  final out = <DiffLine>[for (final line in old.take(start)) DiffLine(DiffKind.same, line)];
  final rows = oldEnd - start;
  final columns = newEnd - start;
  if (rows > _maxCompared || columns > _maxCompared) {
    out
      ..addAll([for (var i = start; i < oldEnd; i++) DiffLine(DiffKind.removed, old[i])])
      ..addAll([for (var i = start; i < newEnd; i++) DiffLine(DiffKind.added, now[i])]);
  } else {
    final table = List.generate(rows + 1, (_) => List.filled(columns + 1, 0));
    for (var i = rows - 1; i >= 0; i--) {
      for (var j = columns - 1; j >= 0; j--) {
        table[i][j] = old[start + i] == now[start + j]
            ? table[i + 1][j + 1] + 1
            : (table[i + 1][j] > table[i][j + 1] ? table[i + 1][j] : table[i][j + 1]);
      }
    }
    var i = 0;
    var j = 0;
    while (i < rows || j < columns) {
      if (i < rows && j < columns && old[start + i] == now[start + j]) {
        out.add(DiffLine(DiffKind.same, old[start + i]));
        i++;
        j++;
      } else if (j >= columns || (i < rows && table[i + 1][j] >= table[i][j + 1])) {
        out.add(DiffLine(DiffKind.removed, old[start + i]));
        i++;
      } else {
        out.add(DiffLine(DiffKind.added, now[start + j]));
        j++;
      }
    }
  }
  out.addAll([for (var i = oldEnd; i < old.length; i++) DiffLine(DiffKind.same, old[i])]);
  return out;
}

/// Only the changes, with [context] unchanged lines around each, since a file is mostly what did not change.
List<DiffLine?> withContext(List<DiffLine> lines, {int context = 3}) {
  final keep = List<bool>.filled(lines.length, false);
  for (var i = 0; i < lines.length; i++) {
    if (lines[i].kind == DiffKind.same) continue;
    for (var at = i - context; at <= i + context; at++) {
      if (at >= 0 && at < lines.length) keep[at] = true;
    }
  }
  final out = <DiffLine?>[];
  var skipping = false;
  for (var i = 0; i < lines.length; i++) {
    if (keep[i]) {
      out.add(lines[i]);
      skipping = false;
    } else if (!skipping) {
      // Null marks lines left out.
      out.add(null);
      skipping = true;
    }
  }
  return out;
}

/// A unified patch as lines. The headers are context, and null marks where a hunk starts.
List<DiffLine?> patchLines(String patch) {
  final out = <DiffLine?>[];
  for (final line in patch.split('\n')) {
    if (line.startsWith('@@')) {
      out.add(null);
    } else if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff ') || line.startsWith('index ')) {
      out.add(DiffLine(DiffKind.same, line));
    } else if (line.startsWith('+')) {
      out.add(DiffLine(DiffKind.added, line.substring(1)));
    } else if (line.startsWith('-')) {
      out.add(DiffLine(DiffKind.removed, line.substring(1)));
    } else if (line.isNotEmpty) {
      out.add(DiffLine(DiffKind.same, line.startsWith(' ') ? line.substring(1) : line));
    }
  }
  return out;
}
