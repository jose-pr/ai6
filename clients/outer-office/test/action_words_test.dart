import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:outer_office/access/access_words.dart';

void main() {
  test(
    'every action the access kinds define has words, none is printed raw',
    () {
      final source = File('../../crates/ai6/src/directorates/m/access/mod.rs')
          .readAsStringSync();
      final block = RegExp(
        r'pub const ACTIONS: &\[&str\] = &\[(.*?)\];',
        dotAll: true,
      ).firstMatch(source)!.group(1)!;
      final actions = [
        for (final match in RegExp(r'"([a-z_.]+)"').allMatches(block))
          match.group(1)!,
      ];
      expect(actions.length, greaterThan(30));
      final groups = accessGroups(actions);
      expect(
        [for (final group in groups) group.name],
        isNot(contains('Other')),
        reason: [
          for (final group in groups)
            if (group.name == 'Other') ...group.actions,
        ].join(', '),
      );
      final named = [for (final group in groups) ...group.actions]..sort();
      expect(named, ([...actions]..sort()));
    },
  );
}
