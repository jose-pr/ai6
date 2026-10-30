import 'dart:convert';
import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../routes/oo_route.dart';
import '../shell/oo_shell.dart' show OoPage;
import '../theme/oo_tokens.dart';
import 'charts.dart';
import 'entry_kind.dart';

/// The view schema version this app renders. Universal Exports refuses any other view
/// before it reaches the app; the app checks again and shows a refusal, never a guess.
const assistantViewVersion = 1;

/// The bounds Universal Exports holds views to. It sends its own with each conversation
/// (`view_limits` in the `ready` frame); the defaults are its defaults.
@immutable
class ViewLimits {
  const ViewLimits({
    this.blocks = 24,
    this.viewBytes = 65536,
    this.textBytes = 8192,
    this.shortBytes = 512,
    this.columns = 12,
    this.rows = 200,
    this.items = 100,
    this.series = 8,
    this.points = 200,
    this.pieSlices = 12,
    this.diffBytes = 16384,
    this.fields = 20,
    this.scheduleRows = 50,
    this.graphNodes = 60,
    this.graphEdges = 120,
    this.svgBytes = 32768,
    this.svgElements = 2000,
    this.svgDepth = 16,
  });

  /// The `view_limits` Universal Exports sent; a missing or unusable one keeps its default.
  factory ViewLimits.fromJson(Object? json) {
    final limits = json is Map ? json : const {};
    int read(String key, int fallback) => switch (limits[key]) {
      final int value when value >= 1 => value,
      _ => fallback,
    };
    return ViewLimits(
      blocks: read('blocks', 24),
      viewBytes: read('view_bytes', 65536),
      textBytes: read('text_bytes', 8192),
      shortBytes: read('short_bytes', 512),
      columns: read('columns', 12),
      rows: read('rows', 200),
      items: read('items', 100),
      series: read('series', 8),
      points: read('points', 200),
      pieSlices: read('pie_slices', 12),
      diffBytes: read('diff_bytes', 16384),
      fields: read('fields', 20),
      scheduleRows: read('schedule_rows', 50),
      graphNodes: read('graph_nodes', 60),
      graphEdges: read('graph_edges', 120),
      svgBytes: read('svg_bytes', 32768),
      svgElements: read('svg_elements', 2000),
      svgDepth: read('svg_depth', 16),
    );
  }

  final int blocks;
  final int viewBytes;
  final int textBytes;
  final int shortBytes;
  final int columns;
  final int rows;
  final int items;
  final int series;
  final int points;
  final int pieSlices;
  final int diffBytes;
  final int fields;
  final int scheduleRows;
  final int graphNodes;
  final int graphEdges;
  final int svgBytes;
  final int svgElements;
  final int svgDepth;
}

/// The limits of the view being parsed; [AssistantView.parse] sets them for its call.
ViewLimits _limits = const ViewLimits();

/// A view Moneypenny showed, checked against the schema.
@immutable
class AssistantView {
  const AssistantView({required this.title, required this.blocks});

  final String? title;
  final List<ViewBlock> blocks;

  /// Parses a view; a [FormatException] names the first thing that is not the schema.
  factory AssistantView.parse(
    Object? json, {
    ViewLimits limits = const ViewLimits(),
  }) {
    final outer = _limits;
    _limits = limits;
    try {
      return AssistantView._parse(json);
    } finally {
      _limits = outer;
    }
  }

  factory AssistantView._parse(Object? json) {
    final view = _object(json, 'the view', const {
      'version',
      'title',
      'blocks',
    });
    if (view['version'] != assistantViewVersion) {
      throw const FormatException('version must be $assistantViewVersion');
    }
    final title = _optionalShort(view, 'title');
    final blocks = view['blocks'];
    if (blocks is! List || blocks.isEmpty || blocks.length > _limits.blocks) {
      throw FormatException('a view has 1 to ${_limits.blocks} blocks');
    }
    final parsed = <ViewBlock>[];
    for (var index = 0; index < blocks.length; index++) {
      try {
        parsed.add(ViewBlock.parse(blocks[index]));
      } on FormatException catch (error) {
        throw FormatException('block $index: ${error.message}');
      }
    }
    if (parsed.whereType<FormBlock>().length > 1) {
      throw const FormatException('a view has at most one form');
    }
    return AssistantView(title: title, blocks: parsed);
  }
}

Map<String, Object?> _object(Object? json, String what, Set<String> allowed) {
  if (json is! Map) throw FormatException('$what must be an object');
  for (final key in json.keys) {
    if (!allowed.contains(key)) {
      throw FormatException('unknown field `$key` in $what');
    }
  }
  return json.cast<String, Object?>();
}

String _short(Object? value, String what) {
  if (value is! String ||
      utf8.encode(value).length > _limits.shortBytes ||
      value.runes.any((rune) => rune < 0x20 && rune != 0x0A)) {
    throw FormatException(
      '$what is text of at most ${_limits.shortBytes} bytes without control characters',
    );
  }
  return value;
}

String? _optionalShort(Map<String, Object?> json, String key) =>
    json[key] == null ? null : _short(json[key], key);

String _identifier(Object? value, String what) {
  if (value is! String || !isIdentifier(value)) {
    throw FormatException('$what is not an identifier');
  }
  return value;
}

Object? _scalar(Object? value, String what) {
  if (value == null || value is bool) return value;
  if (value is num && value.isFinite) return value;
  if (value is String) return _short(value, what);
  throw FormatException('$what must be text, a number, true, false or null');
}

List<(String, Object?)> _pairs(Object? items, String what, int max) {
  if (items is! List || items.length > max) {
    throw FormatException('$what has at most $max items');
  }
  return [
    for (final item in items)
      () {
        final pair = _object(item, 'an item', const {'key', 'value'});
        return (
          _short(pair['key'], 'a key'),
          _scalar(pair['value'], 'a value'),
        );
      }(),
  ];
}

/// One block of a view.
@immutable
sealed class ViewBlock {
  const ViewBlock();

  factory ViewBlock.parse(Object? json) {
    if (json is! Map) throw const FormatException('a block must be an object');
    return switch (json['kind']) {
      'text' => TextBlock._parse(json),
      'table' => TableBlock._parse(json),
      'key_values' => KeyValuesBlock._parse(json),
      'chart' => ChartBlock._parse(json),
      'card' => CardBlock._parse(json),
      'timeline' => TimelineBlock._parse(json),
      'diff' => DiffBlock._parse(json),
      'form' => FormBlock._parse(json),
      'schedule' => ScheduleBlock._parse(json),
      'graph' => GraphBlock._parse(json),
      'svg' => SvgBlock._parse(json),
      final kind => throw FormatException('unknown variant `$kind`'),
    };
  }
}

class TextBlock extends ViewBlock {
  const TextBlock(this.markdown);

  final String markdown;

  factory TextBlock._parse(Map json) {
    final block = _object(json, 'text', const {'kind', 'markdown'});
    final markdown = block['markdown'];
    if (markdown is! String ||
        markdown.trim().isEmpty ||
        utf8.encode(markdown).length > _limits.textBytes) {
      throw FormatException('text is 1 to ${_limits.textBytes} bytes');
    }
    for (final match in RegExp(r'\]\(').allMatches(markdown)) {
      if (!markdown.startsWith('app:/', match.end)) {
        throw const FormatException(
          'a link must point into the app (app:/...); outside links are not shown',
        );
      }
      final close = markdown.indexOf(')', match.end);
      final target = markdown.substring(
        match.end,
        close < 0 ? markdown.length : close,
      );
      try {
        OoRoute.parseUri(target);
      } on FormatException catch (error) {
        throw FormatException('a link is not an app route: ${error.message}');
      }
    }
    return TextBlock(markdown);
  }
}

class TableBlock extends ViewBlock {
  const TableBlock(this.columns, this.rows);

  final List<String> columns;
  final List<List<Object?>> rows;

  factory TableBlock._parse(Map json) {
    final block = _object(json, 'a table', const {'kind', 'columns', 'rows'});
    final columns = block['columns'];
    if (columns is! List ||
        columns.isEmpty ||
        columns.length > _limits.columns) {
      throw FormatException('a table has 1 to ${_limits.columns} columns');
    }
    final rows = block['rows'];
    if (rows is! List || rows.length > _limits.rows) {
      throw FormatException('a table has at most ${_limits.rows} rows');
    }
    return TableBlock(
      [for (final column in columns) _short(column, 'a column')],
      [
        for (final row in rows)
          if (row is List && row.length == columns.length)
            [for (final cell in row) _scalar(cell, 'a cell')]
          else
            throw const FormatException('every row has one cell per column'),
      ],
    );
  }
}

class KeyValuesBlock extends ViewBlock {
  const KeyValuesBlock(this.items);

  final List<(String, Object?)> items;

  factory KeyValuesBlock._parse(Map json) {
    final block = _object(json, 'a key-value list', const {'kind', 'items'});
    final items = _pairs(block['items'], 'a key-value list', _limits.items);
    if (items.isEmpty) {
      throw FormatException('a key-value list has 1 to ${_limits.items} items');
    }
    return KeyValuesBlock(items);
  }
}

@immutable
class ChartSeries {
  const ChartSeries(this.name, this.points);

  final String name;
  final List<(Object?, double)> points;
}

class ChartBlock extends ViewBlock {
  const ChartBlock({
    required this.chart,
    required this.stacked,
    required this.title,
    required this.xLabel,
    required this.yLabel,
    required this.series,
  });

  /// `bar`, `line` or `pie`.
  final String chart;

  /// Whether a bar chart's series are stacked in one bar per x.
  final bool stacked;
  final String? title;
  final String? xLabel;
  final String? yLabel;
  final List<ChartSeries> series;

  factory ChartBlock._parse(Map json) {
    final block = _object(json, 'a chart', const {
      'kind',
      'chart',
      'stacked',
      'title',
      'x_label',
      'y_label',
      'series',
    });
    final chart = block['chart'];
    if (chart != 'bar' && chart != 'line' && chart != 'pie') {
      throw const FormatException('chart is bar, line or pie');
    }
    final stacked = block['stacked'] ?? false;
    if (stacked is! bool) {
      throw const FormatException('stacked is true or false');
    }
    if (stacked && chart != 'bar') {
      throw const FormatException('only a bar chart is stacked');
    }
    final series = block['series'];
    if (series is! List || series.isEmpty || series.length > _limits.series) {
      throw FormatException('a chart has 1 to ${_limits.series} series');
    }
    final parsed = [
      for (final item in series)
        () {
          final line = _object(item, 'a series', const {'name', 'points'});
          final points = line['points'];
          if (points is! List ||
              points.isEmpty ||
              points.length > _limits.points) {
            throw FormatException('a series has 1 to ${_limits.points} points');
          }
          final seen = <String>{};
          return ChartSeries(_short(line['name'], 'a series name'), [
            for (final point in points)
              () {
                final xy = _object(point, 'a point', const {'x', 'y'});
                final y = xy['y'];
                if (y is! num || !y.isFinite) {
                  throw const FormatException('y must be a finite number');
                }
                final x = _scalar(xy['x'], 'x');
                // Bars and slices are placed by their x, so a series names each x once.
                if (chart != 'line' && !seen.add(jsonEncode(x))) {
                  throw FormatException(
                    'a series names each x once, not ${jsonEncode(x)} twice',
                  );
                }
                return (x, y.toDouble());
              }(),
          ]);
        }(),
    ];
    final values = [
      for (final line in parsed)
        for (final (_, y) in line.points) y,
    ];
    if (chart == 'pie') {
      if (parsed.length != 1) {
        throw const FormatException('a pie has one series');
      }
      if (parsed.single.points.length > _limits.pieSlices) {
        throw FormatException('a pie has at most ${_limits.pieSlices} slices');
      }
      if (values.any((y) => y < 0)) {
        throw const FormatException("a pie's values are 0 or more");
      }
      if (values.every((y) => y == 0)) {
        throw const FormatException('a pie has a value above 0');
      }
    }
    if (stacked && values.any((y) => y < 0)) {
      throw const FormatException("a stacked bar's values are 0 or more");
    }
    return ChartBlock(
      chart: chart as String,
      stacked: stacked,
      title: _optionalShort(block, 'title'),
      xLabel: _optionalShort(block, 'x_label'),
      yLabel: _optionalShort(block, 'y_label'),
      series: parsed,
    );
  }
}

/// A link to a page of the app: an app route in its link form (`app:/<page>?...`), read as
/// Universal Exports reads it.
String _appLink(Object? value, String what) {
  final link = _short(value, what);
  if (!link.startsWith('app:/')) {
    throw FormatException(
      '$what must point into the app (app:/...); outside links are not shown',
    );
  }
  try {
    OoRoute.parseUri(link);
  } on FormatException catch (error) {
    throw FormatException('$what is not an app route: ${error.message}');
  }
  return link;
}

String? _optionalLink(Map<String, Object?> json, String key) =>
    json[key] == null ? null : _appLink(json[key], 'a link');

final _rfc3339 = RegExp(
  r'^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(\.\d+)?([Zz]|([+-])(\d{2}):(\d{2}))$',
);

/// The UTC time of an RFC 3339 text (`2026-10-29T08:00:00Z`, `...T08:00:00.5+07:00`), or
/// null when [text] is not one; read as Universal Exports reads it.
DateTime? parseRfc3339(String text) {
  final match = _rfc3339.firstMatch(text);
  if (match == null) return null;
  int part(int group) => int.parse(match.group(group)!);
  final (year, month, day) = (part(1), part(2), part(3));
  final (hour, minute, second) = (part(4), part(5), part(6));
  final leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
  final days = switch (month) {
    1 || 3 || 5 || 7 || 8 || 10 || 12 => 31,
    4 || 6 || 9 || 11 => 30,
    2 => leap ? 29 : 28,
    _ => 0,
  };
  if (day < 1 || day > days || hour > 23 || minute > 59 || second > 60) {
    return null;
  }
  var offset = Duration.zero;
  if (match.group(9) != null) {
    final (hours, minutes) = (part(10), part(11));
    if (hours > 23 || minutes > 59) return null;
    offset = Duration(hours: hours, minutes: minutes);
    if (match.group(9) == '-') offset = -offset;
  }
  final fraction = match.group(7) == null
      ? 0
      : (double.parse('0${match.group(7)}') * 1000000).round();
  return DateTime.utc(
    year,
    month,
    day,
    hour,
    minute,
    second,
    0,
    fraction,
  ).subtract(offset);
}

/// A schedule's time: an RFC 3339 text or a finite number of the person's choosing.
sealed class ScheduleTime {
  const ScheduleTime();

  /// The position on the time axis: Unix seconds for a date, else the number.
  double get value;
}

class ScheduleDate extends ScheduleTime {
  const ScheduleDate(this.at);

  final DateTime at;

  @override
  double get value => at.microsecondsSinceEpoch / 1e6;
}

class ScheduleNumber extends ScheduleTime {
  const ScheduleNumber(this.number);

  final double number;

  @override
  double get value => number;
}

ScheduleTime _scheduleTime(Object? value) => switch (value) {
  final String text => switch (parseRfc3339(text)) {
    final DateTime at => ScheduleDate(at),
    null => throw FormatException(
      '${jsonEncode(text)} is not an RFC 3339 time',
    ),
  },
  final num number when number.isFinite => ScheduleNumber(number.toDouble()),
  num() => throw const FormatException('a time is a finite number'),
  _ => throw const FormatException(
    'start and end are RFC 3339 times or numbers',
  ),
};

@immutable
class ScheduleRow {
  const ScheduleRow({
    required this.label,
    required this.start,
    required this.end,
    required this.state,
    required this.link,
  });

  final String label;
  final ScheduleTime start;
  final ScheduleTime end;
  final String? state;
  final String? link;
}

/// Bars over time, one row each.
class ScheduleBlock extends ViewBlock {
  const ScheduleBlock({required this.title, required this.rows});

  final String? title;
  final List<ScheduleRow> rows;

  /// Whether the times are RFC 3339 dates rather than numbers.
  bool get dates => rows.first.start is ScheduleDate;

  factory ScheduleBlock._parse(Map json) {
    final block = _object(json, 'a schedule', const {'kind', 'title', 'rows'});
    final rows = block['rows'];
    if (rows is! List || rows.isEmpty || rows.length > _limits.scheduleRows) {
      throw FormatException('a schedule has 1 to ${_limits.scheduleRows} rows');
    }
    bool? dates;
    return ScheduleBlock(
      title: _optionalShort(block, 'title'),
      rows: [
        for (final item in rows)
          () {
            final row = _object(item, 'a row', const {
              'label',
              'start',
              'end',
              'state',
              'link',
            });
            final label = _short(row['label'], 'a label');
            final state = _optionalShort(row, 'state');
            final link = _optionalLink(row, 'link');
            final start = _scheduleTime(row['start']);
            final end = _scheduleTime(row['end']);
            final isDate = start is ScheduleDate;
            if ((end is ScheduleDate) != isDate ||
                (dates ??= isDate) != isDate) {
              throw const FormatException(
                "a schedule's times are all RFC 3339 times or all numbers",
              );
            }
            if (start.value >= end.value) {
              throw FormatException(
                'every row starts before it ends (${jsonEncode(label)})',
              );
            }
            return ScheduleRow(
              label: label,
              start: start,
              end: end,
              state: state,
              link: link,
            );
          }(),
      ],
    );
  }
}

/// The kinds a graph node may be.
const graphNodeKinds = {
  'operation',
  'station',
  'theater',
  'mission',
  'session',
  'agent',
  'other',
};

@immutable
class GraphNode {
  const GraphNode({
    required this.id,
    required this.label,
    required this.kind,
    required this.state,
    required this.link,
  });

  final String id;
  final String label;
  final String? kind;
  final String? state;
  final String? link;
}

@immutable
class GraphEdge {
  const GraphEdge(this.from, this.to, this.label);

  final String from;
  final String to;
  final String? label;
}

/// Nodes and the edges between them.
class GraphBlock extends ViewBlock {
  const GraphBlock({
    required this.title,
    required this.nodes,
    required this.edges,
    required this.tree,
  });

  final String? title;
  final List<GraphNode> nodes;
  final List<GraphEdge> edges;

  /// Whether it is laid out as a tree rather than in layers.
  final bool tree;

  factory GraphBlock._parse(Map json) {
    final block = _object(json, 'a graph', const {
      'kind',
      'title',
      'nodes',
      'edges',
      'layout',
    });
    final layout = block['layout'] ?? 'layered';
    if (layout != 'layered' && layout != 'tree') {
      throw const FormatException('layout is layered or tree');
    }
    final nodes = block['nodes'];
    if (nodes is! List || nodes.isEmpty || nodes.length > _limits.graphNodes) {
      throw FormatException('a graph has 1 to ${_limits.graphNodes} nodes');
    }
    final edges = block['edges'] ?? const [];
    if (edges is! List || edges.length > _limits.graphEdges) {
      throw FormatException('a graph has at most ${_limits.graphEdges} edges');
    }
    final parsedNodes = <GraphNode>[];
    final ids = <String>{};
    for (final item in nodes) {
      final node = _object(item, 'a node', const {
        'id',
        'label',
        'kind',
        'state',
        'link',
      });
      final id = _identifier(node['id'], 'a node id');
      if (!ids.add(id)) {
        throw FormatException('the node id ${jsonEncode(id)} is used twice');
      }
      final label = _short(node['label'], 'a node label');
      final kind = node['kind'];
      if (kind != null && !graphNodeKinds.contains(kind)) {
        throw const FormatException(
          "a node's kind is operation, station, theater, mission, session, agent or other",
        );
      }
      parsedNodes.add(
        GraphNode(
          id: id,
          label: label,
          kind: kind as String?,
          state: _optionalShort(node, 'state'),
          link: _optionalLink(node, 'link'),
        ),
      );
    }
    final parents = <String, String>{};
    final parsedEdges = <GraphEdge>[];
    for (final item in edges) {
      final edge = _object(item, 'an edge', const {'from', 'to', 'label'});
      final from = edge['from'];
      final to = edge['to'];
      for (final end in [from, to]) {
        if (end is! String || !ids.contains(end)) {
          throw FormatException(
            'an edge names an unknown node ${jsonEncode(end)}',
          );
        }
      }
      if (from == to) {
        throw FormatException(
          'an edge joins two nodes, not ${jsonEncode(from)} to itself',
        );
      }
      final label = _optionalShort(edge, 'label');
      if (layout == 'tree' && parents.containsKey(to)) {
        throw FormatException(
          'in a tree each node has at most one parent; ${jsonEncode(to)} has two',
        );
      }
      parents[to as String] = from as String;
      parsedEdges.add(GraphEdge(from, to, label));
    }
    if (layout == 'tree') {
      for (final start in ids) {
        var at = start;
        for (
          var step = 0;
          step < ids.length && parents.containsKey(at);
          step++
        ) {
          at = parents[at]!;
        }
        if (parents.containsKey(at)) {
          throw const FormatException('a tree has no cycle');
        }
      }
      final roots = ids.where((id) => !parents.containsKey(id)).length;
      if (roots != 1) {
        throw FormatException('a tree has one root, not $roots');
      }
    }
    return GraphBlock(
      title: _optionalShort(block, 'title'),
      nodes: parsedNodes,
      edges: parsedEdges,
      tree: layout == 'tree',
    );
  }
}

/// A drawing in SVG. Universal Exports admits only its allow-list and sends the canonical
/// form; the app checks the size and the root element and draws the rest as sent.
class SvgBlock extends ViewBlock {
  const SvgBlock({required this.title, required this.svg});

  final String title;
  final String svg;

  factory SvgBlock._parse(Map json) {
    final block = _object(json, 'an SVG', const {'kind', 'title', 'svg'});
    final title = _short(block['title'], 'title');
    if (title.trim().isEmpty) throw const FormatException('an SVG has a title');
    final svg = block['svg'];
    if (svg is! String || utf8.encode(svg).length > _limits.svgBytes) {
      throw FormatException('an SVG is at most ${_limits.svgBytes} bytes');
    }
    final body = svg
        .trimLeft()
        .replaceFirst(RegExp(r'^<\?xml[^>]*\?>'), '')
        .trimLeft();
    if (!RegExp(r'^<svg[\s/>]').hasMatch(body)) {
      throw const FormatException('the root element of an SVG is <svg>');
    }
    return SvgBlock(title: title, svg: svg);
  }
}

class CardBlock extends ViewBlock {
  const CardBlock({
    required this.card,
    required this.id,
    required this.operationId,
    required this.state,
    required this.fields,
  });

  /// `mission`, `station` or `theater`.
  final String card;
  final String id;
  final String? operationId;
  final String? state;
  final List<(String, Object?)> fields;

  /// The app page the card opens.
  String get link => _cardRoute(card, id, operationId).toUri();

  factory CardBlock._parse(Map json) {
    final block = _object(json, 'a card', const {
      'kind',
      'card',
      'id',
      'operation_id',
      'state',
      'fields',
    });
    final card = block['card'];
    if (card != 'mission' && card != 'station' && card != 'theater') {
      throw const FormatException('card is mission, station or theater');
    }
    final operation = block['operation_id'] == null
        ? null
        : _identifier(block['operation_id'], 'operation_id');
    if (card == 'mission' && operation == null) {
      throw const FormatException('a Mission card names its operation_id');
    }
    final id = _identifier(block['id'], 'id');
    _cardRoute(card as String, id, operation);
    return CardBlock(
      card: card,
      id: id,
      operationId: operation,
      state: _optionalShort(block, 'state'),
      fields: block['fields'] == null
          ? const []
          : _pairs(block['fields'], 'a card', _limits.fields),
    );
  }
}

/// The page a card opens: a Mission on Missions, a Station on Stations, a Theater on the
/// Stations page's Theaters tab (a card names no Station, which selecting a Theater needs).
OoRoute _cardRoute(String card, String id, String? operationId) =>
    switch (card) {
      'mission' => const OoRoute(
        OoPage.missions,
      ).withField('operation_id', operationId).withField('mission_id', id),
      'station' => const OoRoute(OoPage.stations).withField('station_id', id),
      _ => const OoRoute(OoPage.stations).withField('tab', 'theaters'),
    };

class TimelineBlock extends ViewBlock {
  const TimelineBlock(this.items);

  final List<(double, String, String?)> items;

  factory TimelineBlock._parse(Map json) {
    final block = _object(json, 'a timeline', const {'kind', 'items'});
    final items = block['items'];
    if (items is! List || items.isEmpty || items.length > _limits.items) {
      throw FormatException('a timeline has 1 to ${_limits.items} items');
    }
    return TimelineBlock([
      for (final item in items)
        () {
          final moment = _object(item, 'an item', const {
            'at',
            'label',
            'detail',
          });
          final at = moment['at'];
          if (at is! num || !at.isFinite || at < 0) {
            throw const FormatException('at is Unix seconds');
          }
          return (
            at.toDouble(),
            _short(moment['label'], 'a label'),
            _optionalShort(moment, 'detail'),
          );
        }(),
    ]);
  }
}

class DiffBlock extends ViewBlock {
  const DiffBlock({
    required this.title,
    required this.before,
    required this.after,
  });

  final String? title;
  final String before;
  final String after;

  factory DiffBlock._parse(Map json) {
    final block = _object(json, 'a diff', const {
      'kind',
      'title',
      'before',
      'after',
    });
    final before = block['before'];
    final after = block['after'];
    if (before is! String ||
        after is! String ||
        utf8.encode(before).length > _limits.diffBytes ||
        utf8.encode(after).length > _limits.diffBytes) {
      throw FormatException(
        'each side of a diff is text of at most ${_limits.diffBytes} bytes',
      );
    }
    return DiffBlock(
      title: _optionalShort(block, 'title'),
      before: before,
      after: after,
    );
  }
}

@immutable
class ViewFormField {
  const ViewFormField({
    required this.name,
    required this.label,
    required this.type,
    required this.value,
    required this.choices,
  });

  final String name;
  final String label;

  /// `text`, `number`, `boolean`, `choice` or `json`.
  final String type;
  final Object? value;
  final List<String> choices;
}

class FormBlock extends ViewBlock {
  const FormBlock({
    required this.tool,
    required this.title,
    required this.fixed,
    required this.fields,
  });

  /// The `propose_*` tool the form drafts.
  final String tool;
  final String title;
  final Map<String, Object?> fixed;
  final List<ViewFormField> fields;

  factory FormBlock._parse(Map json) {
    final block = _object(json, 'a form', const {
      'kind',
      'tool',
      'title',
      'fixed',
      'fields',
    });
    final tool = block['tool'];
    if (tool is! String || !tool.startsWith('propose_')) {
      throw FormatException(
        'a form drafts one of the propose_* tools, not ${jsonEncode(tool)}',
      );
    }
    final fixed = block['fixed'] ?? const <String, Object?>{};
    if (fixed is! Map) throw const FormatException('fixed must be an object');
    final fields = block['fields'];
    if (fields is! List || fields.isEmpty || fields.length > _limits.fields) {
      throw FormatException('a form has 1 to ${_limits.fields} fields');
    }
    return FormBlock(
      tool: tool,
      title: _short(block['title'], 'title'),
      fixed: fixed.cast<String, Object?>(),
      fields: [
        for (final item in fields)
          () {
            final field = _object(item, 'a field', const {
              'name',
              'label',
              'type',
              'value',
              'choices',
            });
            final type = field['type'];
            if (!const [
              'text',
              'number',
              'boolean',
              'choice',
              'json',
            ].contains(type)) {
              throw const FormatException(
                "a field's type is text, number, boolean, choice or json",
              );
            }
            final choices = [
              for (final choice in field['choices'] as List? ?? const [])
                _short(choice, 'a choice'),
            ];
            if ((type == 'choice') == choices.isEmpty) {
              throw const FormatException(
                'a choice field, and only one, lists choices',
              );
            }
            final name = _identifier(field['name'], 'a field name');
            if (fixed.containsKey(name)) {
              throw FormatException('$name is both fixed and a field');
            }
            return ViewFormField(
              name: name,
              label: _short(field['label'], 'a field label'),
              type: type as String,
              value: field['value'],
              choices: choices,
            );
          }(),
      ],
    );
  }
}

/// What a value shows as in a cell, a key-value list or a card.
String showValue(Object? value) => switch (value) {
  null => '—',
  bool() => value ? 'yes' : 'no',
  num() => value.toString(),
  _ => revealInvisible(value.toString()),
};

/// Renders a view, or, when it is not the schema, says so and shows nothing of it.
class AssistantViewCard extends StatelessWidget {
  const AssistantViewCard({
    required this.json,
    this.exposed = false,
    this.limits = const ViewLimits(),
    this.onOpen,
    this.onSubmit,
    super.key,
  });

  final Object? json;

  /// Whether she built it after reading text from outside AI6; the card says so above it.
  final bool exposed;

  /// The bounds Universal Exports sent with the conversation.
  final ViewLimits limits;

  /// Opens an app page (`app:/<page>?...`).
  final void Function(String link)? onOpen;

  /// Submits the view's form with the person's values.
  final void Function(Map<String, Object?> values)? onSubmit;

  @override
  Widget build(BuildContext context) {
    final AssistantView view;
    try {
      view = AssistantView.parse(json, limits: limits);
    } on FormatException catch (error) {
      return RefusedView(reason: error.message);
    }
    final theme = Theme.of(context);
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (exposed) const OutsideTextStrip(),
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Row(
                spacing: 8,
                children: [
                  const EntryKindIcon(EntryKind.view),
                  Expanded(
                    child: view.title == null
                        ? Text(
                            EntryKind.view.label,
                            style: theme.textTheme.bodySmall,
                          )
                        : Text(
                            revealInvisible(view.title!),
                            style: theme.textTheme.titleMedium,
                          ),
                  ),
                ],
              ),
            ),
            for (final block in view.blocks)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: switch (block) {
                  TextBlock() => MarkdownText(block.markdown, onOpen: onOpen),
                  TableBlock() => AssistantTable(block.columns, block.rows),
                  KeyValuesBlock() => _PairsView(block.items),
                  ChartBlock() => ChartView(block),
                  CardBlock() => _CardView(block, onOpen: onOpen),
                  TimelineBlock() => _TimelineView(block),
                  DiffBlock() => _DiffView(block),
                  FormBlock() => _FormView(block, onSubmit: onSubmit),
                  ScheduleBlock() => ScheduleView(block, onOpen: onOpen),
                  GraphBlock() => GraphBlockView(block, onOpen: onOpen),
                  SvgBlock() => SvgView(block),
                },
              ),
          ],
        ),
      ),
    );
  }
}

/// The strip over a view she built after reading text from outside AI6.
class OutsideTextStrip extends StatelessWidget {
  const OutsideTextStrip({this.sources = const [], super.key});

  /// What exposed the conversation, when it is known: `tool:missions`, or the source a Station
  /// reported.
  final List<String> sources;

  /// What the strip says.
  static const message =
      'Made after reading text from outside AI6. Check it before acting on it.';

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      decoration: BoxDecoration(
        color: Color.alphaBlend(
          tokens.warning.withValues(alpha: 0.14),
          tokens.raised,
        ),
        border: Border(left: BorderSide(color: tokens.warning, width: 3)),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        spacing: 8,
        children: [
          Icon(Icons.warning_amber_rounded, color: tokens.warning, size: 18),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(message, style: TextStyle(color: tokens.text)),
                if (sources.isNotEmpty)
                  Text(
                    revealInvisible('Read from: ${sources.join(', ')}'),
                    style: TextStyle(color: tokens.muted, fontSize: 12),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A view the app would not render, with why.
class RefusedView extends StatelessWidget {
  const RefusedView({required this.reason, super.key});

  final String reason;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Card(
      margin: EdgeInsets.zero,
      color: colors.errorContainer,
      child: ListTile(
        leading: Icon(Icons.block, color: colors.onErrorContainer),
        title: const Text('View refused'),
        subtitle: Text(revealInvisible(reason)),
      ),
    );
  }
}

/// The Markdown subset of a text block: `#` headings, `-` lists, paragraphs, `**bold**`,
/// `*italic*`, `` `code` `` and links into the app. Anything else is shown as written.
class MarkdownText extends StatelessWidget {
  const MarkdownText(this.markdown, {this.onOpen, super.key});

  final String markdown;
  final void Function(String link)? onOpen;

  static final _inline = RegExp(
    r'\*\*(.+?)\*\*|\*(.+?)\*|`([^`]+)`|\[([^\]]+)\]\((app:/[^)\s]*)\)',
  );

  List<InlineSpan> _spans(BuildContext context, String line) {
    final theme = Theme.of(context);
    final spans = <InlineSpan>[];
    var at = 0;
    for (final match in _inline.allMatches(line)) {
      if (match.start > at) {
        spans.add(TextSpan(text: line.substring(at, match.start)));
      }
      if (match.group(1) != null) {
        spans.add(
          TextSpan(
            text: match.group(1),
            style: const TextStyle(fontWeight: FontWeight.bold),
          ),
        );
      } else if (match.group(2) != null) {
        spans.add(
          TextSpan(
            text: match.group(2),
            style: const TextStyle(fontStyle: FontStyle.italic),
          ),
        );
      } else if (match.group(3) != null) {
        spans.add(
          TextSpan(
            text: match.group(3),
            style: TextStyle(
              fontFamily: 'monospace',
              backgroundColor: theme.colorScheme.surfaceContainerHighest,
            ),
          ),
        );
      } else {
        final link = match.group(5)!;
        spans.add(
          WidgetSpan(
            alignment: PlaceholderAlignment.baseline,
            baseline: TextBaseline.alphabetic,
            child: InkWell(
              onTap: onOpen == null ? null : () => onOpen!(link),
              child: Text(
                match.group(4)!,
                style: TextStyle(
                  color: theme.colorScheme.primary,
                  decoration: TextDecoration.underline,
                ),
              ),
            ),
          ),
        );
      }
      at = match.end;
    }
    if (at < line.length) spans.add(TextSpan(text: line.substring(at)));
    return spans;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final children = <Widget>[];
    for (final raw in revealInvisible(markdown).split('\n')) {
      final line = raw.trimRight();
      if (line.isEmpty) continue;
      final heading = RegExp(r'^(#{1,3}) (.*)$').firstMatch(line);
      if (heading != null) {
        children.add(
          Text.rich(
            TextSpan(children: _spans(context, heading.group(2)!)),
            style: heading.group(1)!.length == 1
                ? theme.textTheme.titleLarge
                : theme.textTheme.titleMedium,
          ),
        );
      } else if (line.startsWith('- ') || line.startsWith('* ')) {
        children.add(
          Text.rich(
            TextSpan(
              children: [
                const TextSpan(text: '•  '),
                ..._spans(context, line.substring(2)),
              ],
            ),
          ),
        );
      } else {
        children.add(Text.rich(TextSpan(children: _spans(context, line))));
      }
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      spacing: 4,
      children: children,
    );
  }
}

/// A table of plain values: a table block, or a chart, schedule or graph shown as one.
class AssistantTable extends StatelessWidget {
  const AssistantTable(this.columns, this.rows, {super.key});

  final List<String> columns;
  final List<List<Object?>> rows;

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    scrollDirection: Axis.horizontal,
    child: DataTable(
      columns: [
        for (final column in columns)
          DataColumn(label: Text(revealInvisible(column))),
      ],
      rows: [
        for (final row in rows)
          DataRow(
            cells: [for (final cell in row) DataCell(Text(showValue(cell)))],
          ),
      ],
    ),
  );
}

class _PairsView extends StatelessWidget {
  const _PairsView(this.items);

  final List<(String, Object?)> items;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      for (final (key, value) in items)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 2),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: 160,
                child: Text(
                  revealInvisible(key),
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
              ),
              Expanded(child: Text(showValue(value))),
            ],
          ),
        ),
    ],
  );
}

class _CardView extends StatelessWidget {
  const _CardView(this.block, {this.onOpen});

  final CardBlock block;
  final void Function(String link)? onOpen;

  @override
  Widget build(BuildContext context) {
    final icon = switch (block.card) {
      'mission' => Icons.assignment_outlined,
      'station' => Icons.hub_outlined,
      _ => Icons.dns_outlined,
    };
    final name = switch (block.card) {
      'mission' => 'Mission ${block.id} · Operation ${block.operationId}',
      'station' => 'Station ${block.id}',
      _ => 'Theater ${block.id}',
    };
    return Card(
      margin: EdgeInsets.zero,
      child: ListTile(
        leading: Icon(icon),
        title: Text(revealInvisible(name)),
        subtitle: Text(
          [
            if (block.state != null) revealInvisible(block.state!),
            for (final (key, value) in block.fields)
              '${revealInvisible(key)}: ${showValue(value)}',
          ].join(' · '),
        ),
        trailing: const Icon(Icons.open_in_new),
        onTap: onOpen == null ? null : () => onOpen!(block.link),
      ),
    );
  }
}

class _TimelineView extends StatelessWidget {
  const _TimelineView(this.block);

  final TimelineBlock block;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      for (final (at, label, detail) in block.items)
        ListTile(
          dense: true,
          leading: const Icon(Icons.circle, size: 10),
          title: Text(revealInvisible(label)),
          subtitle: Text(
            [
              DateTime.fromMillisecondsSinceEpoch(
                (at * 1000).round(),
                isUtc: true,
              ).toIso8601String().replaceFirst(RegExp(r'\.\d+Z$'), 'Z'),
              if (detail != null) revealInvisible(detail),
            ].join(' · '),
          ),
        ),
    ],
  );
}

/// The lines of a diff: `' '` kept, `'-'` removed, `'+'` added, by the longest common
/// subsequence of lines (at most 400 a side; past that every line shows as changed).
List<(String, String)> diffLines(String before, String after) {
  final a = before.split('\n');
  final b = after.split('\n');
  if (a.length > 400 || b.length > 400) {
    return [
      for (final line in a) ('-', line),
      for (final line in b) ('+', line),
    ];
  }
  final lengths = List.generate(
    a.length + 1,
    (_) => List.filled(b.length + 1, 0),
  );
  for (var i = a.length - 1; i >= 0; i--) {
    for (var j = b.length - 1; j >= 0; j--) {
      lengths[i][j] = a[i] == b[j]
          ? lengths[i + 1][j + 1] + 1
          : math.max(lengths[i + 1][j], lengths[i][j + 1]);
    }
  }
  final lines = <(String, String)>[];
  var i = 0;
  var j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] == b[j]) {
      lines.add((' ', a[i]));
      i++;
      j++;
    } else if (lengths[i + 1][j] >= lengths[i][j + 1]) {
      lines.add(('-', a[i++]));
    } else {
      lines.add(('+', b[j++]));
    }
  }
  while (i < a.length) {
    lines.add(('-', a[i++]));
  }
  while (j < b.length) {
    lines.add(('+', b[j++]));
  }
  return lines;
}

class _DiffView extends StatelessWidget {
  const _DiffView(this.block);

  final DiffBlock block;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (block.title != null)
          Text(
            revealInvisible(block.title!),
            style: theme.textTheme.titleSmall,
          ),
        for (final (mark, line) in diffLines(block.before, block.after))
          Container(
            color: switch (mark) {
              '-' => theme.colorScheme.errorContainer,
              '+' => theme.colorScheme.tertiaryContainer,
              _ => null,
            },
            child: Text(
              '$mark ${revealInvisible(line)}',
              style: const TextStyle(fontFamily: 'monospace'),
            ),
          ),
      ],
    );
  }
}

class _FormView extends StatefulWidget {
  const _FormView(this.block, {this.onSubmit});

  final FormBlock block;
  final void Function(Map<String, Object?> values)? onSubmit;

  @override
  State<_FormView> createState() => _FormViewState();
}

class _FormViewState extends State<_FormView> {
  late final Map<String, TextEditingController> _text = {
    for (final field in widget.block.fields)
      if (field.type == 'text' ||
          field.type == 'number' ||
          field.type == 'json')
        field.name: TextEditingController(
          text: field.value == null
              ? ''
              : field.value is String
              ? field.value as String
              : jsonEncode(field.value),
        ),
  };
  late final Map<String, Object?> _picked = {
    for (final field in widget.block.fields)
      if (field.type == 'boolean')
        field.name: field.value == true
      else if (field.type == 'choice')
        field.name: field.choices.contains(field.value)
            ? field.value
            : field.choices.first,
  };
  String? _problem;

  @override
  void dispose() {
    for (final controller in _text.values) {
      controller.dispose();
    }
    super.dispose();
  }

  void _submit() {
    final values = <String, Object?>{};
    for (final field in widget.block.fields) {
      switch (field.type) {
        case 'number':
          final number = num.tryParse(_text[field.name]!.text.trim());
          if (number == null) {
            setState(() => _problem = '${field.label} must be a number.');
            return;
          }
          values[field.name] = number;
        case 'text' || 'json':
          values[field.name] = _text[field.name]!.text;
        default:
          values[field.name] = _picked[field.name];
      }
    }
    setState(() => _problem = null);
    widget.onSubmit?.call(values);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 8,
      children: [
        Text(
          revealInvisible(widget.block.title),
          style: theme.textTheme.titleSmall,
        ),
        for (final entry in widget.block.fixed.entries)
          Text(
            '${revealInvisible(entry.key)}: ${showValue(entry.value is String || entry.value is num || entry.value is bool || entry.value == null ? entry.value : jsonEncode(entry.value))}',
          ),
        for (final field in widget.block.fields)
          switch (field.type) {
            'boolean' => SwitchListTile(
              title: Text(revealInvisible(field.label)),
              value: _picked[field.name] == true,
              onChanged: (value) => setState(() => _picked[field.name] = value),
            ),
            'choice' => DropdownButtonFormField<String>(
              initialValue: _picked[field.name] as String?,
              decoration: InputDecoration(
                labelText: revealInvisible(field.label),
              ),
              items: [
                for (final choice in field.choices)
                  DropdownMenuItem(
                    value: choice,
                    child: Text(revealInvisible(choice)),
                  ),
              ],
              onChanged: (value) => setState(() => _picked[field.name] = value),
            ),
            _ => TextField(
              controller: _text[field.name],
              maxLines: field.type == 'json' ? null : 1,
              keyboardType: field.type == 'number'
                  ? TextInputType.number
                  : TextInputType.text,
              decoration: InputDecoration(
                labelText: revealInvisible(field.label),
              ),
            ),
          },
        if (_problem != null)
          Text(_problem!, style: TextStyle(color: theme.colorScheme.error)),
        Align(
          alignment: Alignment.centerRight,
          child: FilledButton.tonal(
            onPressed: widget.onSubmit == null ? null : _submit,
            child: const Text('Draft a proposal'),
          ),
        ),
      ],
    );
  }
}
