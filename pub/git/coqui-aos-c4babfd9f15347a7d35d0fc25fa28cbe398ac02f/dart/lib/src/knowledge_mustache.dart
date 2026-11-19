/// The one place a `mustache` file's text is rendered, so the engine can be swapped without
/// touching how a file is found, counted or served. The engine is the `mustache_template`
/// package, which follows Mustache's specification; this file bounds its output and gives its
/// partials the overlay's served files.
library;

import 'package:mustache_template/mustache_template.dart' as m;
// The package resolves every partial of a render through one resolver and tells it neither
// which template holds the tag nor how deep it is, which `@path`'s folder and depth need. A
// marker node placed before each partial tag says it, so the package's node classes are used,
// and `pubspec.yaml` pins its version.
// ignore: implementation_imports
import 'package:mustache_template/src/node.dart' as node;
// ignore: implementation_imports
import 'package:mustache_template/src/template.dart' as stock;

import 'knowledge_core.dart';
import 'text.dart';

/// A partial the overlay holds: the [path] of the served file and the [text] a partial of it
/// is read from.
typedef MustachePartial = ({String path, String text});

/// Finds the partial a tag in the file at `fromPath` names, or null when it names none.
typedef MustachePartials =
    MustachePartial? Function(String name, String fromPath);

class _Frame {
  _Frame(this.path, this.depth);
  final String path;
  final int depth;
}

class _Run {
  _Run(this.current);
  _Frame current;
}

class _Mark extends node.Node {
  _Mark(this.run, this.frame) : super(0, 0);
  final _Run run;
  final _Frame frame;

  @override
  void accept(node.Visitor visitor) => run.current = frame;
}

void _mark(List<node.Node> nodes, _Run run, _Frame frame) {
  for (var at = 0; at < nodes.length; at += 1) {
    final each = nodes[at];
    if (each is node.SectionNode) _mark(each.children, run, frame);
    if (each is node.PartialNode) {
      nodes.insert(at, _Mark(run, frame));
      at += 1;
    }
  }
}

class _Bounded implements StringSink {
  _Bounded(this.most);
  final int? most;
  final StringBuffer buffer = StringBuffer();
  int size = 0;

  @override
  void write(Object? object) {
    final text = '$object';
    size += utf8Length(text);
    if (most != null && size > most!) {
      throw KnowledgeError('too-large', 'the rendered text');
    }
    buffer.write(text);
  }

  @override
  void writeAll(Iterable<Object?> objects, [String separator = '']) =>
      write(objects.join(separator));

  @override
  void writeCharCode(int charCode) => write(String.fromCharCode(charCode));

  @override
  void writeln([Object? object = '']) => write('$object\n');
}

/// [source], the text of the file at [path], rendered as Mustache over [data] (the roots
/// `overlay`, `project`, `caller`, `env`, `options` and `context`).
///
/// `{{x}}` is HTML escaped unless [escape] is `none`. A partial is found by [partials] from the
/// folder of the file that holds the tag; one it does not find renders as nothing. Partials
/// nest at most `maxIncludeDepth` deep (`include-error`) and the output is at most
/// `maxRenderedBytes` (`too-large`); a template the library refuses is `template-error`.
String renderMustache(
  String source,
  Map<String, Object?> data, {
  required String path,
  required MustachePartials partials,
  String escape = 'html',
  KnowledgeSettings? settings,
}) {
  final limit = settings ?? KnowledgeSettings();
  final run = _Run(_Frame(path, 0));
  final sink = _Bounded(limit.maxRenderedBytes);

  late final stock.Template Function(String, _Frame) build;

  m.Template? resolve(String name) {
    final from = run.current;
    final found = partials(name, from.path);
    if (found == null) return null;
    final depth = from.depth + 1;
    final most = limit.maxIncludeDepth;
    if (most != null && depth > most) {
      throw KnowledgeError('include-error', 'partials are nested too deep');
    }
    return build(found.text, _Frame(found.path, depth));
  }

  build = (String text, _Frame frame) {
    final template = stock.Template.fromSource(
      text,
      lenient: true,
      htmlEscapeValues: escape == 'html',
      partialResolver: resolve,
    );
    _mark(stock.getTemplateNodes(template), run, frame);
    return template;
  };

  try {
    build(source, run.current).render(data, sink);
  } on m.TemplateException catch (error) {
    throw KnowledgeError('template-error', error.message);
  }
  return sink.buffer.toString();
}
