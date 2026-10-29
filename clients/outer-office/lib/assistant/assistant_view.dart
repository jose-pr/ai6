import 'dart:convert';
import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../routes/oo_route.dart';
import '../shell/oo_shell.dart' show OoPage;

/// The view schema version this app renders. Universal Exports refuses any other view
/// before it reaches the app; the app checks again and shows a refusal, never a guess.
const assistantViewVersion = 1;

const _maxBlocks = 24;
const _maxText = 8 * 1024;
const _maxShort = 512;
const _maxColumns = 12;
const _maxRows = 200;
const _maxItems = 100;
const _maxSeries = 8;
const _maxPoints = 200;
const _maxDiff = 16 * 1024;
const _maxFields = 20;

/// A view Moneypenny showed, checked against the schema.
@immutable
class AssistantView {
  const AssistantView({required this.title, required this.blocks});

  final String? title;
  final List<ViewBlock> blocks;

  /// Parses a view; a [FormatException] names the first thing that is not the schema.
  factory AssistantView.parse(Object? json) {
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
    if (blocks is! List || blocks.isEmpty || blocks.length > _maxBlocks) {
      throw const FormatException('a view has 1 to $_maxBlocks blocks');
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
      utf8.encode(value).length > _maxShort ||
      value.runes.any((rune) => rune < 0x20 && rune != 0x0A)) {
    throw FormatException(
      '$what is text of at most $_maxShort bytes without control characters',
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
        utf8.encode(markdown).length > _maxText) {
      throw const FormatException('text is 1 to $_maxText bytes');
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
    if (columns is! List || columns.isEmpty || columns.length > _maxColumns) {
      throw const FormatException('a table has 1 to $_maxColumns columns');
    }
    final rows = block['rows'];
    if (rows is! List || rows.length > _maxRows) {
      throw const FormatException('a table has at most $_maxRows rows');
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
    final items = _pairs(block['items'], 'a key-value list', _maxItems);
    if (items.isEmpty) {
      throw const FormatException('a key-value list has 1 to $_maxItems items');
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
    required this.line,
    required this.title,
    required this.xLabel,
    required this.yLabel,
    required this.series,
  });

  final bool line;
  final String? title;
  final String? xLabel;
  final String? yLabel;
  final List<ChartSeries> series;

  factory ChartBlock._parse(Map json) {
    final block = _object(json, 'a chart', const {
      'kind',
      'chart',
      'title',
      'x_label',
      'y_label',
      'series',
    });
    final chart = block['chart'];
    if (chart != 'bar' && chart != 'line') {
      throw const FormatException('chart is bar or line');
    }
    final series = block['series'];
    if (series is! List || series.isEmpty || series.length > _maxSeries) {
      throw const FormatException('a chart has 1 to $_maxSeries series');
    }
    return ChartBlock(
      line: chart == 'line',
      title: _optionalShort(block, 'title'),
      xLabel: _optionalShort(block, 'x_label'),
      yLabel: _optionalShort(block, 'y_label'),
      series: [
        for (final item in series)
          () {
            final line = _object(item, 'a series', const {'name', 'points'});
            final points = line['points'];
            if (points is! List ||
                points.isEmpty ||
                points.length > _maxPoints) {
              throw const FormatException(
                'a series has 1 to $_maxPoints points',
              );
            }
            return ChartSeries(_short(line['name'], 'a series name'), [
              for (final point in points)
                () {
                  final xy = _object(point, 'a point', const {'x', 'y'});
                  final y = xy['y'];
                  if (y is! num || !y.isFinite) {
                    throw const FormatException('y must be a finite number');
                  }
                  return (_scalar(xy['x'], 'x'), y.toDouble());
                }(),
            ]);
          }(),
      ],
    );
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
          : _pairs(block['fields'], 'a card', _maxFields),
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
    if (items is! List || items.isEmpty || items.length > _maxItems) {
      throw const FormatException('a timeline has 1 to $_maxItems items');
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
        utf8.encode(before).length > _maxDiff ||
        utf8.encode(after).length > _maxDiff) {
      throw const FormatException(
        'each side of a diff is text of at most $_maxDiff bytes',
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
    if (fields is! List || fields.isEmpty || fields.length > _maxFields) {
      throw const FormatException('a form has 1 to $_maxFields fields');
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
    this.onOpen,
    this.onSubmit,
    super.key,
  });

  final Object? json;

  /// Opens an app page (`app:/<page>?...`).
  final void Function(String link)? onOpen;

  /// Submits the view's form with the person's values.
  final void Function(Map<String, Object?> values)? onSubmit;

  @override
  Widget build(BuildContext context) {
    final AssistantView view;
    try {
      view = AssistantView.parse(json);
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
            if (view.title != null)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Text(
                  revealInvisible(view.title!),
                  style: theme.textTheme.titleMedium,
                ),
              ),
            for (final block in view.blocks)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: switch (block) {
                  TextBlock() => MarkdownText(block.markdown, onOpen: onOpen),
                  TableBlock() => _TableView(block),
                  KeyValuesBlock() => _PairsView(block.items),
                  ChartBlock() => _ChartView(block),
                  CardBlock() => _CardView(block, onOpen: onOpen),
                  TimelineBlock() => _TimelineView(block),
                  DiffBlock() => _DiffView(block),
                  FormBlock() => _FormView(block, onSubmit: onSubmit),
                },
              ),
          ],
        ),
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

class _TableView extends StatelessWidget {
  const _TableView(this.block);

  final TableBlock block;

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    scrollDirection: Axis.horizontal,
    child: DataTable(
      columns: [
        for (final column in block.columns)
          DataColumn(label: Text(revealInvisible(column))),
      ],
      rows: [
        for (final row in block.rows)
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

class _ChartView extends StatelessWidget {
  const _ChartView(this.block);

  final ChartBlock block;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final palette = [
      theme.colorScheme.primary,
      theme.colorScheme.tertiary,
      theme.colorScheme.secondary,
      theme.colorScheme.error,
    ];
    final kind = block.line ? 'line chart' : 'bar chart';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (block.title != null)
          Text(
            revealInvisible(block.title!),
            style: theme.textTheme.titleSmall,
          ),
        Semantics(
          label: '$kind${block.title == null ? '' : ': ${block.title}'}',
          child: SizedBox(
            height: 180,
            child: CustomPaint(
              size: Size.infinite,
              painter: _ChartPainter(block, palette, theme.dividerColor),
            ),
          ),
        ),
        Wrap(
          spacing: 12,
          children: [
            for (var index = 0; index < block.series.length; index++)
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Container(
                    width: 10,
                    height: 10,
                    color: palette[index % palette.length],
                  ),
                  const SizedBox(width: 4),
                  Text(revealInvisible(block.series[index].name)),
                ],
              ),
            if (block.xLabel != null)
              Text('x: ${revealInvisible(block.xLabel!)}'),
            if (block.yLabel != null)
              Text('y: ${revealInvisible(block.yLabel!)}'),
          ],
        ),
      ],
    );
  }
}

class _ChartPainter extends CustomPainter {
  _ChartPainter(this.block, this.palette, this.axis);

  final ChartBlock block;
  final List<Color> palette;
  final Color axis;

  @override
  void paint(Canvas canvas, Size size) {
    final values = [
      for (final series in block.series)
        for (final (_, y) in series.points) y,
    ];
    final top = values.fold<double>(0, math.max);
    final bottom = values.fold<double>(0, math.min);
    final span = (top - bottom) == 0 ? 1.0 : top - bottom;
    double place(double y) => size.height - (y - bottom) / span * size.height;
    canvas.drawLine(
      Offset(0, place(0)),
      Offset(size.width, place(0)),
      Paint()..color = axis,
    );
    final count = block.series
        .map((series) => series.points.length)
        .fold(0, math.max);
    if (count == 0) return;
    final slot = size.width / count;
    for (var index = 0; index < block.series.length; index++) {
      final series = block.series[index];
      final paint = Paint()
        ..color = palette[index % palette.length]
        ..strokeWidth = 2
        ..style = block.line ? PaintingStyle.stroke : PaintingStyle.fill;
      if (block.line) {
        final path = Path();
        for (var at = 0; at < series.points.length; at++) {
          final point = Offset(slot * (at + 0.5), place(series.points[at].$2));
          at == 0
              ? path.moveTo(point.dx, point.dy)
              : path.lineTo(point.dx, point.dy);
        }
        canvas.drawPath(path, paint);
      } else {
        final width = slot / (block.series.length + 1);
        for (var at = 0; at < series.points.length; at++) {
          final left = slot * at + width * (index + 0.5);
          final y = place(series.points[at].$2);
          final zero = place(0);
          canvas.drawRect(
            Rect.fromLTRB(
              left,
              math.min(y, zero),
              left + width,
              math.max(y, zero),
            ),
            paint,
          );
        }
      }
    }
  }

  @override
  bool shouldRepaint(_ChartPainter old) => old.block != block;
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
