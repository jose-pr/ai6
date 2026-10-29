import 'package:flutter_test/flutter_test.dart';
import 'package:outer_office/assistant/graph_layout.dart';

void main() {
  test('nodes take the layer of their longest path from a source', () {
    // G -> hq, G -> worker -> build-box -> mission-7, and G -> mission-7 directly.
    final layout = GraphLayout.of(5, [(0, 1), (0, 2), (2, 3), (3, 4), (0, 4)]);
    expect(layout.layerOf, [0, 1, 1, 2, 3]);
    expect(layout.layers, [
      [0],
      [1, 2],
      [3],
      [4],
    ]);
  });

  test('neighbours in a layer stay at least a slot apart, left to right', () {
    final layout = GraphLayout.of(6, [(0, 1), (0, 2), (0, 3), (0, 4), (0, 5)]);
    final row = [for (final node in layout.layers[1]) layout.x[node]];
    for (var index = 1; index < row.length; index++) {
      expect(row[index] - row[index - 1], greaterThanOrEqualTo(1));
    }
    expect(layout.x.reduce((a, b) => a < b ? a : b), 0);
  });

  test('a parent is centred over its children', () {
    final layout = GraphLayout.of(4, [(0, 1), (0, 2), (1, 3)]);
    expect(layout.x[0], closeTo((layout.x[1] + layout.x[2]) / 2, 0.51));
  });

  test('barycentre ordering uncrosses edges', () {
    // a -> d and b -> c listed crossed: c and d swap under their parents.
    final layout = GraphLayout.of(4, [(0, 3), (1, 2)]);
    expect(layout.layers[1], [3, 2]);
  });

  test('a cycle is laid out without looping, every node placed once', () {
    final layout = GraphLayout.of(3, [(0, 1), (1, 2), (2, 0)]);
    expect(layout.layers.expand((layer) => layer).toList()..sort(), [0, 1, 2]);
    expect(layout.layerOf[0], 0);
  });

  test('nodes without edges sit in the first layer', () {
    final layout = GraphLayout.of(3, const []);
    expect(layout.layers, [
      [0, 1, 2],
    ]);
  });
}
