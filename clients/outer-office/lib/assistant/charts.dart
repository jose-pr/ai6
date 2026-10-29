import 'dart:math' as math;

import 'package:fl_chart/fl_chart.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../theme/oo_decor.dart';
import '../theme/oo_icon.dart';
import '../theme/oo_tokens.dart';
import 'assistant_view.dart';
import 'graph_layout.dart';

// fl_chart builds on Flutter's own Material library, not the app's, so every
// colour and text style it draws with is passed from OoTokens here.

/// [value] as a table shows it: a whole number without a decimal point, the same on the web
/// and the desktop.
num exactNumber(double value) =>
    value == value.truncateToDouble() && value.abs() < 1e15
    ? value.toInt()
    : value;

/// A round step that cuts [span] into about [ticks] parts: 1, 2, 2.5 or 5 times a power of 10.
double niceStep(double span, {int ticks = 5}) {
  if (!(span > 0)) return 1;
  final raw = span / ticks;
  final power = math.pow(10, (math.log(raw) / math.ln10).floor()).toDouble();
  for (final factor in const [1, 2, 2.5, 5]) {
    if (raw <= factor * power) return factor * power;
  }
  return 10 * power;
}

/// A round step of time, in seconds, that cuts [span] seconds into about four parts.
double timeStep(double span) {
  const steps = [
    60, 120, 300, 600, 900, 1800, //
    3600, 7200, 10800, 21600, 43200, //
    86400, 172800, 604800, 1209600, 2592000,
  ];
  for (final step in steps) {
    if (span / step <= 5) return step.toDouble();
  }
  return niceStep(span / 86400, ticks: 4) * 86400;
}

/// An axis label at [value], or nothing at an end of the axis the step does not land on,
/// where it would crowd the label beside it.
Widget _tick(TitleMeta meta, double value, String text, TextStyle style) {
  final step = meta.appliedInterval;
  final onStep =
      step <= 0 ||
      ((value - meta.min) / step - ((value - meta.min) / step).round()).abs() <
          1e-6;
  if (!onStep && (value == meta.max || value == meta.min)) {
    return const SizedBox.shrink();
  }
  return SideTitleWidget(
    meta: meta,
    child: Text(
      text,
      style: style,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
    ),
  );
}

/// A compact number for an axis: `1.2k`, `3M`, `0.25`.
String axisNumber(double value) {
  final size = value.abs();
  String trim(double number, int digits) {
    final text = number.toStringAsFixed(digits);
    return text.contains('.') ? text.replaceFirst(RegExp(r'\.?0+$'), '') : text;
  }

  if (size >= 1e9) return '${trim(value / 1e9, 1)}G';
  if (size >= 1e6) return '${trim(value / 1e6, 1)}M';
  if (size >= 1e4) return '${trim(value / 1e3, 1)}k';
  if (value == value.roundToDouble()) return value.round().toString();
  return trim(value, size >= 10 ? 1 : 2);
}

/// A chart, schedule or graph with a "Show as table" switch that draws the same values in a
/// table, and an accessible summary of what the picture shows.
class _Switchable extends StatefulWidget {
  const _Switchable({
    required this.title,
    required this.noun,
    required this.summary,
    required this.picture,
    required this.columns,
    required this.rows,
  });

  final String? title;

  /// What the picture is, for "Show as ...".
  final String noun;
  final String summary;
  final WidgetBuilder picture;
  final List<String> columns;
  final List<List<Object?>> rows;

  @override
  State<_Switchable> createState() => _SwitchableState();
}

class _SwitchableState extends State<_Switchable> {
  bool _table = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 6,
      children: [
        Row(
          children: [
            Expanded(
              child: widget.title == null
                  ? const SizedBox.shrink()
                  : Text(
                      revealInvisible(widget.title!),
                      style: theme.textTheme.titleSmall,
                    ),
            ),
            TextButton.icon(
              onPressed: () => setState(() => _table = !_table),
              icon: Icon(
                _table ? Icons.insights_outlined : Icons.table_rows_outlined,
                size: 18,
              ),
              label: Text(_table ? 'Show as ${widget.noun}' : 'Show as table'),
            ),
          ],
        ),
        if (_table)
          AssistantTable(widget.columns, widget.rows)
        else
          Semantics(
            container: true,
            label: widget.summary,
            child: widget.picture(context),
          ),
      ],
    );
  }
}

/// The colour square and name of each series.
class _Legend extends StatelessWidget {
  const _Legend(this.items);

  final List<(Color, String)> items;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Wrap(
      spacing: 12,
      runSpacing: 4,
      children: [
        for (final (color, name) in items)
          Row(
            mainAxisSize: MainAxisSize.min,
            spacing: 4,
            children: [
              Container(
                width: 10,
                height: 10,
                decoration: BoxDecoration(
                  color: color,
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
              Text(revealInvisible(name), style: TextStyle(color: tokens.text)),
            ],
          ),
      ],
    );
  }
}

/// A bar (side by side or stacked), line or pie chart, coloured with `OoTokens.series`.
class ChartView extends StatelessWidget {
  const ChartView(this.block, {super.key});

  final ChartBlock block;

  /// The x values of every series in the order they first appear.
  List<Object?> get _categories {
    final seen = <String>{};
    return [
      for (final series in block.series)
        for (final (x, _) in series.points)
          if (seen.add(showValue(x))) x,
    ];
  }

  /// Whether every value is a whole number.
  bool get _wholeNumbers => block.series.every(
    (series) =>
        series.points.every((point) => point.$2 == point.$2.roundToDouble()),
  );

  String get _noun => switch (block.chart) {
    'pie' => 'pie chart',
    'line' => 'line chart',
    _ => block.stacked ? 'stacked bar chart' : 'bar chart',
  };

  String get _summary {
    final values = [
      for (final series in block.series)
        for (final (_, y) in series.points) y,
    ];
    final points = values.length;
    final low = values.reduce(math.min);
    final high = values.reduce(math.max);
    final title = block.title == null ? '' : ': ${block.title}';
    final series = block.series.length;
    return '$_noun$title. $series series, $points '
        'value${points == 1 ? '' : 's'} from ${axisNumber(low)} to ${axisNumber(high)}.';
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final categories = _categories;
    final xName = block.xLabel ?? 'x';
    final (columns, rows) = switch (block.chart) {
      'pie' => () {
        final total = block.series.single.points.fold<double>(
          0,
          (sum, point) => sum + point.$2,
        );
        return (
          [block.xLabel ?? 'Slice', block.series.single.name, 'Share'],
          [
            for (final (x, y) in block.series.single.points)
              [x, exactNumber(y), '${(y / total * 100).toStringAsFixed(1)}%'],
          ],
        );
      }(),
      'line' when categories.every((x) => x is num) => (
        [xName, for (final series in block.series) series.name],
        [
          for (final x in [
            ...categories,
          ]..sort((a, b) => (a as num).compareTo(b as num)))
            [
              x,
              for (final series in block.series)
                series.points
                    .where((point) => point.$1 == x)
                    .map((point) => exactNumber(point.$2))
                    .firstOrNull,
            ],
        ],
      ),
      _ => (
        [xName, for (final series in block.series) series.name],
        [
          for (final x in categories)
            [
              x,
              for (final series in block.series)
                series.points
                    .where((point) => showValue(point.$1) == showValue(x))
                    .map((point) => exactNumber(point.$2))
                    .firstOrNull,
            ],
        ],
      ),
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 6,
      children: [
        _Switchable(
          title: block.title,
          noun: _noun,
          summary: _summary,
          columns: columns,
          rows: rows,
          picture: (context) => SizedBox(
            height: 200,
            child: switch (block.chart) {
              'pie' => _pie(tokens),
              'line' => _line(tokens, categories),
              _ => _bar(tokens, categories),
            },
          ),
        ),
        _Legend([
          if (block.chart == 'pie')
            for (final (index, (x, y)) in block.series.single.points.indexed)
              (
                tokens.series[index % tokens.series.length],
                '${showValue(x)}: ${axisNumber(y)}',
              )
          else
            for (final (index, series) in block.series.indexed)
              (tokens.series[index % tokens.series.length], series.name),
        ]),
        if (block.xLabel != null || block.yLabel != null)
          Wrap(
            spacing: 12,
            children: [
              if (block.xLabel != null && block.chart != 'pie')
                Text(
                  'x: ${revealInvisible(block.xLabel!)}',
                  style: TextStyle(color: tokens.muted),
                ),
              if (block.yLabel != null)
                Text(
                  'y: ${revealInvisible(block.yLabel!)}',
                  style: TextStyle(color: tokens.muted),
                ),
            ],
          ),
      ],
    );
  }

  static FlTitlesData _titles(
    OoTokens tokens, {
    required String Function(double value) bottom,
    double bottomInterval = 1,
    double reservedLeft = 40,
    bool wholeNumbers = false,
  }) {
    final style = TextStyle(color: tokens.muted, fontSize: 11);
    const none = AxisTitles(sideTitles: SideTitles(showTitles: false));
    return FlTitlesData(
      topTitles: none,
      rightTitles: none,
      leftTitles: AxisTitles(
        sideTitles: SideTitles(
          showTitles: true,
          reservedSize: reservedLeft,
          // Counts read 0, 1, 2: a fraction between them would be a value no point has.
          getTitlesWidget: (value, meta) =>
              wholeNumbers && value != value.roundToDouble()
              ? const SizedBox.shrink()
              : _tick(meta, value, axisNumber(value), style),
        ),
      ),
      bottomTitles: AxisTitles(
        sideTitles: SideTitles(
          showTitles: true,
          reservedSize: 24,
          interval: bottomInterval,
          getTitlesWidget: (value, meta) =>
              _tick(meta, value, bottom(value), style),
        ),
      ),
    );
  }

  static FlGridData _grid(OoTokens tokens) => FlGridData(
    drawVerticalLine: false,
    getDrawingHorizontalLine: (_) =>
        FlLine(color: tokens.border, strokeWidth: 1),
  );

  /// Shows every label up to 12 categories, then every n-th.
  static String Function(double) _every(List<Object?> categories) {
    final step = math.max(1, (categories.length / 12).ceil());
    return (value) {
      final index = value.round();
      if (index < 0 || index >= categories.length || index % step != 0) {
        return '';
      }
      return revealInvisible(showValue(categories[index]));
    };
  }

  Widget _bar(OoTokens tokens, List<Object?> categories) {
    final colors = [
      for (var index = 0; index < block.series.length; index++)
        tokens.series[index % tokens.series.length],
    ];
    double? valueAt(ChartSeries series, Object? x) => series.points
        .where((point) => showValue(point.$1) == showValue(x))
        .map((point) => point.$2)
        .firstOrNull;
    final perGroup = block.stacked ? 1 : block.series.length;
    final width = (160 / (categories.length * perGroup)).clamp(3.0, 22.0);
    final groups = [
      for (final (index, x) in categories.indexed)
        BarChartGroupData(
          x: index,
          barsSpace: 2,
          barRods: block.stacked
              ? [
                  () {
                    var top = 0.0;
                    final items = <BarChartRodStackItem>[];
                    for (final (at, series) in block.series.indexed) {
                      final y = valueAt(series, x) ?? 0;
                      items.add(BarChartRodStackItem(top, top + y, colors[at]));
                      top += y;
                    }
                    return BarChartRodData(
                      toY: top,
                      width: width,
                      color: Colors.transparent,
                      rodStackItems: items,
                      borderRadius: BorderRadius.zero,
                    );
                  }(),
                ]
              : [
                  for (final (at, series) in block.series.indexed)
                    BarChartRodData(
                      toY: valueAt(series, x) ?? 0,
                      width: width,
                      color: colors[at],
                      borderRadius: const BorderRadius.vertical(
                        top: Radius.circular(2),
                      ),
                    ),
                ],
        ),
    ];
    return BarChart(
      BarChartData(
        barGroups: groups,
        alignment: BarChartAlignment.spaceAround,
        titlesData: _titles(
          tokens,
          bottom: _every(categories),
          wholeNumbers: _wholeNumbers,
        ),
        gridData: _grid(tokens),
        borderData: FlBorderData(show: false),
        barTouchData: BarTouchData(enabled: false),
      ),
      duration: Duration.zero,
    );
  }

  Widget _line(OoTokens tokens, List<Object?> categories) {
    final numeric = categories.every((x) => x is num);
    double place(Object? x) => numeric
        ? (x as num).toDouble()
        : categories
              .indexWhere((category) => showValue(category) == showValue(x))
              .toDouble();
    final bars = [
      for (final (index, series) in block.series.indexed)
        () {
          final color = tokens.series[index % tokens.series.length];
          final spots = [
            for (final (x, y) in series.points) FlSpot(place(x), y),
          ]..sort((a, b) => a.x.compareTo(b.x));
          return LineChartBarData(
            spots: spots,
            color: color,
            barWidth: 2,
            dotData: FlDotData(
              show: spots.length <= 40,
              getDotPainter: (spot, percent, bar, at) => FlDotCirclePainter(
                radius: 3,
                color: color,
                strokeWidth: 0,
                strokeColor: color,
              ),
            ),
          );
        }(),
    ];
    final xs = [for (final bar in bars) ...bar.spots.map((spot) => spot.x)];
    final low = xs.reduce(math.min);
    final high = xs.reduce(math.max);
    final span = high - low;
    return LineChart(
      LineChartData(
        lineBarsData: bars,
        minX: span == 0 ? low - 1 : low,
        maxX: span == 0 ? high + 1 : high,
        titlesData: _titles(
          tokens,
          bottom: numeric ? axisNumber : _every(categories),
          bottomInterval: numeric ? niceStep(span, ticks: 4) : 1,
        ),
        gridData: _grid(tokens),
        borderData: FlBorderData(show: false),
        lineTouchData: const LineTouchData(enabled: false),
      ),
      duration: Duration.zero,
    );
  }

  Widget _pie(OoTokens tokens) => PieChart(
    PieChartData(
      sections: [
        for (final (index, (_, y)) in block.series.single.points.indexed)
          PieChartSectionData(
            value: y,
            color: tokens.series[index % tokens.series.length],
            radius: 64,
            showTitle: false,
          ),
      ],
      centerSpaceRadius: 30,
      sectionsSpace: 2,
      pieTouchData: PieTouchData(enabled: false),
    ),
    duration: Duration.zero,
  );
}

/// A schedule as horizontal bars from each row's start to its end.
class ScheduleView extends StatelessWidget {
  const ScheduleView(this.block, {this.onOpen, super.key});

  final ScheduleBlock block;
  final void Function(String link)? onOpen;

  static String _date(DateTime at) => at
      .toIso8601String()
      .replaceFirst('T', ' ')
      .replaceFirst(RegExp(r'(\.0+)?Z$'), 'Z');

  static String _time(ScheduleTime time) => switch (time) {
    ScheduleDate(:final at) => _date(at),
    ScheduleNumber(:final number) => axisNumber(number),
  };

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final low = block.rows.map((row) => row.start.value).reduce(math.min);
    final high = block.rows.map((row) => row.end.value).reduce(math.max);
    final step = block.dates
        ? timeStep(high - low)
        : niceStep(high - low, ticks: 4);
    // The axis runs from and to whole steps, so its ticks fall on round times.
    final from = (low / step).floorToDouble() * step;
    final to = (high / step).ceilToDouble() * step;
    final span = high - low;
    String axis(double value) {
      if (!block.dates) return axisNumber(low + value);
      final at = DateTime.fromMicrosecondsSinceEpoch(
        ((low + value) * 1e6).round(),
        isUtc: true,
      );
      String two(int number) => number.toString().padLeft(2, '0');
      return span < 2 * 86400
          ? '${two(at.hour)}:${two(at.minute)}'
          : '${two(at.month)}-${two(at.day)}';
    }

    final title = block.title == null ? '' : ': ${block.title}';
    final style = TextStyle(color: tokens.muted, fontSize: 11);
    const none = AxisTitles(sideTitles: SideTitles(showTitles: false));
    return _Switchable(
      title: block.title,
      noun: 'schedule',
      summary:
          'schedule$title. ${block.rows.length} '
          'row${block.rows.length == 1 ? '' : 's'} from ${_time(block.rows.map((row) => row.start).reduce((a, b) => a.value <= b.value ? a : b))} '
          'to ${_time(block.rows.map((row) => row.end).reduce((a, b) => a.value >= b.value ? a : b))}.',
      columns: const ['Row', 'Start', 'End', 'State'],
      rows: [
        for (final row in block.rows)
          [row.label, _time(row.start), _time(row.end), row.state],
      ],
      // Room on the right for the label of the last tick, centred on the axis's end.
      picture: (context) => Padding(
        padding: const EdgeInsets.only(right: 18),
        child: SizedBox(
          height: block.rows.length * 28.0 + 36,
          child: BarChart(
            BarChartData(
              rotationQuarterTurns: 1,
              minY: from - low,
              maxY: to - low,
              alignment: BarChartAlignment.spaceAround,
              barGroups: [
                for (final (index, row) in block.rows.indexed)
                  BarChartGroupData(
                    x: index,
                    barRods: [
                      BarChartRodData(
                        fromY: row.start.value - low,
                        toY: row.end.value - low,
                        width: 14,
                        color: row.state == null
                            ? tokens.series.first
                            : tokens.stateColor(row.state!),
                        borderRadius: BorderRadius.circular(3),
                      ),
                    ],
                  ),
              ],
              titlesData: FlTitlesData(
                topTitles: none,
                rightTitles: none,
                leftTitles: AxisTitles(
                  sideTitles: SideTitles(
                    showTitles: true,
                    reservedSize: 22,
                    interval: step,
                    getTitlesWidget: (value, meta) =>
                        _tick(meta, value, axis(value), style),
                  ),
                ),
                bottomTitles: AxisTitles(
                  sideTitles: SideTitles(
                    showTitles: true,
                    reservedSize: 120,
                    interval: 1,
                    getTitlesWidget: (value, meta) {
                      final index = value.round();
                      if (index < 0 || index >= block.rows.length) {
                        return const SizedBox.shrink();
                      }
                      final row = block.rows[index];
                      final label = Text(
                        revealInvisible(row.label),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: row.link == null ? tokens.text : tokens.accent,
                          fontSize: 12,
                          decoration: row.link == null
                              ? null
                              : TextDecoration.underline,
                        ),
                      );
                      return SideTitleWidget(
                        meta: meta,
                        child: SizedBox(
                          width: 110,
                          child: row.link == null || onOpen == null
                              ? label
                              : InkWell(
                                  onTap: () => onOpen!(row.link!),
                                  child: label,
                                ),
                        ),
                      );
                    },
                  ),
                ),
              ),
              gridData: FlGridData(
                drawHorizontalLine: false,
                drawVerticalLine: false,
              ),
              borderData: FlBorderData(show: false),
              barTouchData: BarTouchData(enabled: false),
            ),
            duration: Duration.zero,
          ),
        ),
      ),
    );
  }
}

/// A graph laid out in layers by [GraphLayout], each node a card, the edges drawn behind.
class GraphBlockView extends StatelessWidget {
  const GraphBlockView(this.block, {this.onOpen, super.key});

  final GraphBlock block;
  final void Function(String link)? onOpen;

  static const _nodeWidth = 140.0;
  static const _nodeHeight = 56.0;
  static const _gap = 16.0;
  static const _layerGap = 44.0;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final index = {for (final (at, node) in block.nodes.indexed) node.id: at};
    final layout = GraphLayout.of(block.nodes.length, [
      for (final edge in block.edges) (index[edge.from]!, index[edge.to]!),
    ]);
    Offset topLeft(int node) => Offset(
      layout.x[node] * (_nodeWidth + _gap),
      layout.layerOf[node] * (_nodeHeight + _layerGap),
    );
    final size = Size(
      layout.width * (_nodeWidth + _gap) - _gap,
      layout.layers.length * (_nodeHeight + _layerGap) - _layerGap,
    );
    final byId = {for (final node in block.nodes) node.id: node};
    final title = block.title == null ? '' : ': ${block.title}';
    String targets(GraphNode node) => [
      for (final edge in block.edges)
        if (edge.from == node.id)
          edge.label == null
              ? byId[edge.to]!.label
              : '${byId[edge.to]!.label} (${edge.label})',
    ].join(', ');
    return _Switchable(
      title: block.title,
      noun: 'graph',
      summary:
          'graph$title. ${block.nodes.length} '
          'node${block.nodes.length == 1 ? '' : 's'} and ${block.edges.length} '
          'edge${block.edges.length == 1 ? '' : 's'}: '
          '${[for (final edge in block.edges) '${byId[edge.from]!.label} to ${byId[edge.to]!.label}'].join(', ')}.',
      columns: const ['Node', 'Kind', 'State', 'Leads to'],
      rows: [
        for (final node in block.nodes)
          [node.label, node.kind, node.state, targets(node)],
      ],
      picture: (context) => SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: SizedBox.fromSize(
          size: size,
          child: Stack(
            children: [
              Positioned.fill(
                child: CustomPaint(
                  painter: _EdgePainter(
                    edges: [
                      for (final edge in block.edges)
                        (
                          topLeft(index[edge.from]!),
                          topLeft(index[edge.to]!),
                          edge.label == null
                              ? null
                              : revealInvisible(edge.label!),
                        ),
                    ],
                    node: const Size(_nodeWidth, _nodeHeight),
                    line: tokens.muted,
                    labelText: tokens.text,
                    labelFill: tokens.surface,
                  ),
                ),
              ),
              for (final (at, node) in block.nodes.indexed)
                Positioned(
                  left: topLeft(at).dx,
                  top: topLeft(at).dy,
                  width: _nodeWidth,
                  height: _nodeHeight,
                  child: _GraphNodeCard(node, onOpen: onOpen),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The edges of a graph: a curve from the bottom of one node to the top of the next, an
/// arrowhead at its end and its label at its middle.
class _EdgePainter extends CustomPainter {
  _EdgePainter({
    required this.edges,
    required this.node,
    required this.line,
    required this.labelText,
    required this.labelFill,
  });

  /// Each edge's two nodes' top-left corners and its label.
  final List<(Offset, Offset, String?)> edges;
  final Size node;
  final Color line;
  final Color labelText;
  final Color labelFill;

  @override
  void paint(Canvas canvas, Size size) {
    final stroke = Paint()
      ..color = line
      ..strokeWidth = 1.5
      ..style = PaintingStyle.stroke;
    final head = Paint()..color = line;
    for (final (from, to, label) in edges) {
      final downward = to.dy > from.dy;
      final start = from + Offset(node.width / 2, downward ? node.height : 0);
      final end = to + Offset(node.width / 2, downward ? 0 : node.height);
      final bend = (end.dy - start.dy) / 2;
      final path = Path()
        ..moveTo(start.dx, start.dy)
        ..cubicTo(
          start.dx,
          start.dy + bend,
          end.dx,
          end.dy - bend,
          end.dx,
          end.dy,
        );
      canvas.drawPath(path, stroke);
      final tip = downward ? -1.0 : 1.0;
      canvas.drawPath(
        Path()
          ..moveTo(end.dx, end.dy)
          ..lineTo(end.dx - 4, end.dy + 7 * tip)
          ..lineTo(end.dx + 4, end.dy + 7 * tip)
          ..close(),
        head,
      );
      if (label != null) {
        final text = TextPainter(
          text: TextSpan(
            text: label,
            style: TextStyle(color: labelText, fontSize: 11),
          ),
          textDirection: TextDirection.ltr,
          maxLines: 1,
          ellipsis: '…',
        )..layout(maxWidth: node.width);
        final middle = Offset((start.dx + end.dx) / 2, (start.dy + end.dy) / 2);
        final box = Rect.fromCenter(
          center: middle,
          width: text.width + 8,
          height: text.height + 2,
        );
        canvas.drawRRect(
          RRect.fromRectAndRadius(box, const Radius.circular(3)),
          Paint()..color = labelFill,
        );
        text.paint(canvas, box.topLeft + const Offset(4, 1));
        text.dispose();
      }
    }
  }

  @override
  bool shouldRepaint(_EdgePainter old) =>
      old.edges != edges ||
      old.line != line ||
      old.labelText != labelText ||
      old.labelFill != labelFill;
}

/// A node of a graph: its kind's icon, label and state, opening its page when it has one.
class _GraphNodeCard extends StatelessWidget {
  const _GraphNodeCard(this.node, {this.onOpen});

  final GraphNode node;
  final void Function(String link)? onOpen;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final icon = switch (node.kind) {
      'operation' => OoIconName.operations,
      'station' => OoIconName.stations,
      'theater' => OoIconName.theaters,
      'mission' => OoIconName.missions,
      'session' => OoIconName.sessions,
      'agent' => OoIconName.agents,
      _ => null,
    };
    final card = Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: tokens.raised,
        border: Border.all(
          color: node.link == null ? tokens.border : tokens.accent,
        ),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        spacing: 6,
        children: [
          if (icon != null)
            IconTheme.merge(
              data: IconThemeData(color: tokens.accent, size: 16),
              child: OoIcon(icon),
            ),
          Flexible(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  revealInvisible(node.label),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: tokens.text, fontSize: 13),
                ),
                if (node.state != null)
                  Row(
                    mainAxisSize: MainAxisSize.min,
                    spacing: 4,
                    children: [
                      OoStatusDot(tokens.stateColor(node.state!), size: 7),
                      Text(
                        revealInvisible(node.state!),
                        style: TextStyle(color: tokens.muted, fontSize: 11),
                      ),
                    ],
                  ),
              ],
            ),
          ),
        ],
      ),
    );
    if (node.link == null || onOpen == null) return card;
    return InkWell(
      borderRadius: BorderRadius.circular(8),
      onTap: () => onOpen!(node.link!),
      child: card,
    );
  }
}

/// A drawing Universal Exports admitted, drawn by `flutter_svg` on a surface panel at most
/// 480 px high, with `currentColor` the theme's text colour.
class SvgView extends StatelessWidget {
  const SvgView(this.block, {super.key});

  final SvgBlock block;

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 6,
      children: [
        Text(
          revealInvisible(block.title),
          style: Theme.of(context).textTheme.titleSmall,
        ),
        Container(
          constraints: const BoxConstraints(maxHeight: 480),
          padding: const EdgeInsets.all(8),
          decoration: BoxDecoration(
            color: tokens.surface,
            border: Border.all(color: tokens.border),
            borderRadius: BorderRadius.circular(8),
          ),
          child: SvgPicture.string(
            block.svg,
            theme: SvgTheme(currentColor: tokens.text),
            semanticsLabel: block.title,
            errorBuilder: (context, error, stack) => Text(
              'This drawing could not be drawn.',
              style: TextStyle(color: tokens.muted),
            ),
          ),
        ),
      ],
    );
  }
}
