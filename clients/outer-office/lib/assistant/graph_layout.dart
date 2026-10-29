import 'dart:math' as math;

/// Where the nodes of a directed graph go when it is drawn in layers, sources at the top.
///
/// Edges that close a cycle are turned around for the layout only (a depth-first search from
/// the sources decides which). Each node's layer is the longest path to it from a source; the
/// order within each layer comes from barycentre sweeps, down then up, that move a node toward
/// the mean position of its neighbours; x positions then follow the same means, at least one
/// slot apart. A tree, laid out this way, has each parent centred over its children.
class GraphLayout {
  GraphLayout._(this.layerOf, this.layers, this.x);

  /// The layer of each node, by index.
  final List<int> layerOf;

  /// The nodes of each layer, top first, left to right.
  final List<List<int>> layers;

  /// The centre of each node, by index, in slots: neighbours in a layer are at least 1 apart,
  /// and the leftmost node of the graph is at 0.
  final List<double> x;

  /// The widest a layer reaches, in slots.
  double get width => x.isEmpty ? 0 : x.reduce(math.max) + 1;

  /// Lays out [count] nodes joined by [edges] (`(from, to)` node indices).
  factory GraphLayout.of(int count, List<(int, int)> edges, {int sweeps = 4}) {
    final out = List.generate(count, (_) => <int>[]);
    final incoming = List.filled(count, 0);
    for (final (from, to) in edges) {
      out[from].add(to);
      incoming[to]++;
    }

    // Break cycles: an edge back to a node still on the search stack is turned around.
    final state = List.filled(count, 0);
    final down = <(int, int)>{};
    void visit(int node) {
      state[node] = 1;
      for (final next in out[node]) {
        if (state[next] == 1) {
          down.add((next, node));
        } else {
          down.add((node, next));
          if (state[next] == 0) visit(next);
        }
      }
      state[node] = 2;
    }

    for (final node in [
      for (var node = 0; node < count; node++)
        if (incoming[node] == 0) node,
      for (var node = 0; node < count; node++) node,
    ]) {
      if (state[node] == 0) visit(node);
    }

    final below = List.generate(count, (_) => <int>[]);
    final above = List.generate(count, (_) => <int>[]);
    final waiting = List.filled(count, 0);
    for (final (from, to) in down) {
      below[from].add(to);
      above[to].add(from);
      waiting[to]++;
    }

    // Longest path from a source, in topological order.
    final layerOf = List.filled(count, 0);
    final ready = [
      for (var node = 0; node < count; node++)
        if (waiting[node] == 0) node,
    ];
    for (var at = 0; at < ready.length; at++) {
      final node = ready[at];
      for (final next in below[node]) {
        layerOf[next] = math.max(layerOf[next], layerOf[node] + 1);
        if (--waiting[next] == 0) ready.add(next);
      }
    }
    final depth = count == 0 ? 0 : layerOf.reduce(math.max) + 1;
    final layers = List.generate(depth, (_) => <int>[]);
    for (var node = 0; node < count; node++) {
      layers[layerOf[node]].add(node);
    }

    final position = List.filled(count, 0.0);
    void number(List<int> layer) {
      for (final (index, node) in layer.indexed) {
        position[node] = index.toDouble();
      }
    }

    layers.forEach(number);
    double? mean(List<int> nodes) => nodes.isEmpty
        ? null
        : nodes.map((node) => position[node]).reduce((a, b) => a + b) /
              nodes.length;
    void order(List<int> layer, List<List<int>> neighbours) {
      final keys = {
        for (final node in layer)
          node: mean(neighbours[node]) ?? position[node],
      };
      // A stable sort: ties keep their current order.
      final indexed = layer.indexed.toList()
        ..sort((a, b) {
          final byKey = keys[a.$2]!.compareTo(keys[b.$2]!);
          return byKey != 0 ? byKey : a.$1.compareTo(b.$1);
        });
      layer
        ..clear()
        ..addAll(indexed.map((entry) => entry.$2));
      number(layer);
    }

    for (var sweep = 0; sweep < sweeps; sweep++) {
      for (var layer = 1; layer < depth; layer++) {
        order(layers[layer], above);
      }
      for (var layer = depth - 2; layer >= 0; layer--) {
        order(layers[layer], below);
      }
    }

    // Positions: each node toward its neighbours' mean, kept in order and a slot apart.
    final x = List.filled(count, 0.0);
    for (final layer in layers) {
      for (final (index, node) in layer.indexed) {
        x[node] = index.toDouble();
      }
    }
    void place(List<int> layer, List<List<int>> neighbours) {
      final wanted = [
        for (final node in layer)
          neighbours[node].isEmpty
              ? x[node]
              : neighbours[node]
                        .map((other) => x[other])
                        .reduce((a, b) => a + b) /
                    neighbours[node].length,
      ];
      final placed = [...wanted];
      for (var index = 1; index < placed.length; index++) {
        placed[index] = math.max(placed[index], placed[index - 1] + 1);
      }
      // Shift the layer back by half its overshoot so it stays centred on what it wanted.
      if (placed.isNotEmpty) {
        final overshoot =
            (placed.last - wanted.last).clamp(0, double.infinity) / 2;
        for (var index = 0; index < placed.length; index++) {
          x[layer[index]] = placed[index] - overshoot;
        }
      }
    }

    for (var sweep = 0; sweep < sweeps; sweep++) {
      for (var layer = depth - 2; layer >= 0; layer--) {
        place(layers[layer], below);
      }
      for (var layer = 1; layer < depth; layer++) {
        place(layers[layer], above);
      }
    }
    if (count > 0) {
      final left = x.reduce(math.min);
      for (var node = 0; node < count; node++) {
        x[node] -= left;
      }
    }
    return GraphLayout._(layerOf, layers, x);
  }
}
