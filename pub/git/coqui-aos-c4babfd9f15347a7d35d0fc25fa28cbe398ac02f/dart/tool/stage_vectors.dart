// Copies ../vectors/*.json to test/vectors/ so a browser test run can fetch them: the test
// server serves only the package directory. The copy is untracked.
import 'dart:io';

void main() {
  final target = Directory('test/vectors')..createSync(recursive: true);
  for (final file in Directory('../vectors').listSync()) {
    if (file is File && file.path.endsWith('.json')) {
      final name = file.uri.pathSegments.last;
      file.copySync('${target.path}/$name');
    }
  }
}
