import 'dart:ui' show lerpDouble;

import 'package:material_ui/material_ui.dart';

/// How much of an agent's work a transcript shows.
enum ChatDensity {
  /// A finished turn folds into "Worked for 1m 29s"; thoughts stay out of sight until it is opened, and a burst of finished tool
  /// calls is one line. What the person came for, the answer, is what is left.
  normal('Normal'),

  /// As [normal], with the agent's thoughts shown (folded) and no turn folded away.
  thinking('Thinking'),

  /// Nothing is folded: every tool call is a card of its own and every thought is there.
  verbose('Verbose');

  const ChatDensity(this.label);

  /// What a settings page calls it.
  final String label;

  static ChatDensity? named(String name) {
    for (final density in values) {
      if (density.name == name) return density;
    }
    return null;
  }
}

/// How a line of code is drawn: the [TextSpan] for [line], set in [style], of the file at [path] (null where the package has none, or
/// where one block holds several files). An app gives one to colour code; the package ships no highlighter.
typedef CodeSpans = TextSpan Function(String line, String? path, TextStyle style);

/// What a chat draws with: semantic tokens and sizes in one [ThemeExtension], so an app's look is a preset over it and no widget
/// has a colour of its own. [ChatLook.fromScheme] derives every token from a [ColorScheme]; a preset gives any of them.
@immutable
final class ChatLook extends ThemeExtension<ChatLook> {
  const ChatLook({
    required this.bg,
    required this.surface,
    required this.raised,
    required this.text,
    required this.muted,
    required this.accent,
    required this.border,
    required this.success,
    required this.warning,
    required this.danger,
    this._user,
    this._agent,
    this._notice,
    this._composer,
    this._code,
    this._added,
    this._removed,
    this.messageWidth = 780,
    this.gap = 22,
    this.padding = 12,
    this.radius = 12,
    this.bodySize = 16,
    this.density = ChatDensity.normal,
    this.codeFont = 'monospace',
    this.agentInBubble = false,
    this.userToTheRight = true,
    this.monospace = false,
    this.userPrefix,
    this.centred = false,
    this.bubbleBorder,
    this.showThinking = true,
    this.codeSpans,
  });

  /// The tokens, from the scheme: the page, a card on it, a raised control, the text and its quiet form, the accent, a line, and
  /// the three states.
  factory ChatLook.fromScheme(
    ColorScheme scheme, {
    ChatDensity density = ChatDensity.normal,
    double messageWidth = 780,
    double gap = 22,
    double padding = 12,
    double radius = 12,
    double bodySize = 16,
    String codeFont = 'monospace',
    bool showThinking = true,
  }) => ChatLook(
    bg: scheme.surface,
    surface: scheme.surfaceContainerLow,
    raised: scheme.surfaceContainerHigh,
    text: scheme.onSurface,
    muted: scheme.onSurfaceVariant,
    accent: scheme.primary,
    border: scheme.outlineVariant,
    success: scheme.tertiary,
    warning: scheme.secondary,
    danger: scheme.error,
    density: density,
    messageWidth: messageWidth,
    gap: gap,
    padding: padding,
    radius: radius,
    bodySize: bodySize,
    codeFont: codeFont,
    showThinking: showThinking,
  );

  /// The page, a card on it, and a control raised above the card.
  final Color bg, surface, raised;

  /// The text, its quiet form for what is secondary, the accent, and the colour of a line.
  final Color text, muted, accent, border;

  /// A thing that went well, a thing to look at, and a thing that went wrong.
  final Color success, warning, danger;

  final Color? _user, _agent, _notice, _composer, _code, _added, _removed;

  /// Where the person's words sit, the agent's when it is in a bubble, a notice, the composer, a code block, and a line of a
  /// diff added or removed. Each follows the tokens above unless a preset gives its own.
  Color get user => _user ?? raised;
  Color get agent => _agent ?? surface;
  Color get notice => _notice ?? surface;
  Color get composer => _composer ?? raised;
  Color get code => _code ?? Color.alphaBlend(bg.withValues(alpha: 0.5), surface);
  Color get added => _added ?? Color.alphaBlend(success.withValues(alpha: 0.25), bg);
  Color get removed => _removed ?? Color.alphaBlend(danger.withValues(alpha: 0.25), bg);

  /// The widest a message grows, the space between entries, inside a bubble, the corners, and the body text.
  final double messageWidth, gap, padding, radius, bodySize;

  final ChatDensity density;

  /// The family code and diffs are set in.
  final String codeFont;

  /// With this off, an agent's words sit on the page and only the person's are set apart.
  final bool agentInBubble;
  final bool userToTheRight;
  final bool monospace;

  /// Put before what the person wrote, in place of setting it apart with a colour.
  final String? userPrefix;

  /// With this on, the conversation's column of [messageWidth] sits in the middle of a wider space rather than at its left.
  final bool centred;

  /// The colour of a line round a bubble; none without.
  final Color? bubbleBorder;

  /// Whether streamed agent thinking is shown live while a turn runs.
  final bool showThinking;

  /// Draws each code line of a diff, [ChangesPane] and a permission card's edit; null draws it as plain text in [codeFont].
  final CodeSpans? codeSpans;

  @override
  ChatLook copyWith({
    Color? bg,
    Color? surface,
    Color? raised,
    Color? text,
    Color? muted,
    Color? accent,
    Color? border,
    Color? success,
    Color? warning,
    Color? danger,
    Color? user,
    Color? agent,
    Color? notice,
    Color? composer,
    Color? code,
    Color? added,
    Color? removed,
    double? messageWidth,
    double? gap,
    double? padding,
    double? radius,
    double? bodySize,
    ChatDensity? density,
    String? codeFont,
    bool? agentInBubble,
    bool? userToTheRight,
    bool? monospace,
    String? userPrefix,
    bool? centred,
    Color? bubbleBorder,
    bool? showThinking,
    CodeSpans? codeSpans,
  }) => ChatLook(
    bg: bg ?? this.bg,
    surface: surface ?? this.surface,
    raised: raised ?? this.raised,
    text: text ?? this.text,
    muted: muted ?? this.muted,
    accent: accent ?? this.accent,
    border: border ?? this.border,
    success: success ?? this.success,
    warning: warning ?? this.warning,
    danger: danger ?? this.danger,
    user: user ?? _user,
    agent: agent ?? _agent,
    notice: notice ?? _notice,
    composer: composer ?? _composer,
    code: code ?? _code,
    added: added ?? _added,
    removed: removed ?? _removed,
    messageWidth: messageWidth ?? this.messageWidth,
    gap: gap ?? this.gap,
    padding: padding ?? this.padding,
    radius: radius ?? this.radius,
    bodySize: bodySize ?? this.bodySize,
    density: density ?? this.density,
    codeFont: codeFont ?? this.codeFont,
    agentInBubble: agentInBubble ?? this.agentInBubble,
    userToTheRight: userToTheRight ?? this.userToTheRight,
    monospace: monospace ?? this.monospace,
    userPrefix: userPrefix ?? this.userPrefix,
    centred: centred ?? this.centred,
    bubbleBorder: bubbleBorder ?? this.bubbleBorder,
    showThinking: showThinking ?? this.showThinking,
    codeSpans: codeSpans ?? this.codeSpans,
  );

  @override
  ChatLook lerp(ChatLook? other, double t) {
    if (other == null) return this;
    Color mix(Color a, Color b) => Color.lerp(a, b, t)!;
    Color? mixed(Color? a, Color? b) => a == null || b == null ? (t < 0.5 ? a : b) : Color.lerp(a, b, t);
    // A width with no bound (infinity) cannot be blended: the nearer one stands.
    double size(double a, double b) => a.isFinite && b.isFinite ? lerpDouble(a, b, t)! : (t < 0.5 ? a : b);
    final near = t < 0.5 ? this : other;
    return ChatLook(
      bg: mix(bg, other.bg),
      surface: mix(surface, other.surface),
      raised: mix(raised, other.raised),
      text: mix(text, other.text),
      muted: mix(muted, other.muted),
      accent: mix(accent, other.accent),
      border: mix(border, other.border),
      success: mix(success, other.success),
      warning: mix(warning, other.warning),
      danger: mix(danger, other.danger),
      user: mixed(_user, other._user),
      agent: mixed(_agent, other._agent),
      notice: mixed(_notice, other._notice),
      composer: mixed(_composer, other._composer),
      code: mixed(_code, other._code),
      added: mixed(_added, other._added),
      removed: mixed(_removed, other._removed),
      messageWidth: size(messageWidth, other.messageWidth),
      gap: size(gap, other.gap),
      padding: size(padding, other.padding),
      radius: size(radius, other.radius),
      bodySize: size(bodySize, other.bodySize),
      density: near.density,
      codeFont: near.codeFont,
      agentInBubble: near.agentInBubble,
      userToTheRight: near.userToTheRight,
      monospace: near.monospace,
      userPrefix: near.userPrefix,
      centred: near.centred,
      bubbleBorder: mixed(bubbleBorder, other.bubbleBorder),
      showThinking: near.showThinking,
      codeSpans: near.codeSpans,
    );
  }
}

/// The [ChatLook] in force: the theme's, else one made from its colour scheme.
extension ChatLookOf on BuildContext {
  ChatLook get chatLook => Theme.of(this).extension<ChatLook>() ?? ChatLook.fromScheme(Theme.of(this).colorScheme);
}

/// How an entry sits in the conversation's column.
extension ChatLookColumn on ChatLook {
  /// [child] in the column of [messageWidth]: at its left, or in the middle where the look is [centred]; [toTheRight] puts a
  /// shorter child at the column's right edge. [wide] gives it the whole width instead.
  Widget column(Widget child, {bool toTheRight = false, bool wide = false}) {
    if (wide) return child;
    if (centred) {
      return Center(
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: messageWidth),
          child: Align(alignment: toTheRight ? Alignment.centerRight : Alignment.centerLeft, child: child),
        ),
      );
    }
    return Align(
      alignment: toTheRight ? Alignment.centerRight : Alignment.centerLeft,
      child: ConstrainedBox(constraints: BoxConstraints(maxWidth: messageWidth), child: child),
    );
  }
}

/// A line of code in [style]: through the look's [ChatLook.codeSpans] when it has one, with [prefix] (a number, a `+`) before it in the
/// plain style.
Text codeLine(ChatLook look, String prefix, String line, TextStyle style, {String? path}) {
  final spans = look.codeSpans;
  if (spans == null) return Text('$prefix$line', style: style, softWrap: false);
  return Text.rich(TextSpan(style: style, children: [TextSpan(text: prefix), spans(line, path, style)]), softWrap: false);
}
