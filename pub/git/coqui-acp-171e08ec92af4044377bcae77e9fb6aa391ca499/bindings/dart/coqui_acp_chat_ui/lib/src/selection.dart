import 'package:material_ui/material_ui.dart';
import 'package:flutter/rendering.dart' show SelectedContent;

/// Blocks of a conversation that the selection copies on lines of their own: what a [SelectionArea] copies from the text widgets
/// under it is their text laid end to end, so a paragraph, a code block and the next message would run together.
///
/// The text of the selected children is joined with [join], and [lead] comes first. The area's own copy takes the whole text's
/// ends off, so a lead before the first block or after the last one never reaches the clipboard.
class SelectionBlock extends StatefulWidget {
  const SelectionBlock({super.key, this.lead = '', this.join = '\n', required this.child});

  /// Written before the block's text.
  final String lead;

  /// Written between one selected child's text and the next one's.
  final String join;
  final Widget child;

  @override
  State<SelectionBlock> createState() => _SelectionBlockState();
}

class _SelectionBlockState extends State<SelectionBlock> {
  late final _BlockDelegate _delegate = _BlockDelegate(widget.lead, widget.join);

  @override
  void dispose() {
    _delegate.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => SelectionContainer(delegate: _delegate, child: widget.child);
}

class _BlockDelegate extends StaticSelectionContainerDelegate {
  _BlockDelegate(this.lead, this.join);

  final String lead;
  final String join;

  @override
  SelectedContent? getSelectedContent() {
    final parts = <String>[
      for (final selectable in selectables)
        if (selectable.getSelectedContent() case final SelectedContent content) content.plainText,
    ];
    if (parts.isEmpty) return null;
    return SelectedContent(plainText: lead + parts.join(join));
  }
}
