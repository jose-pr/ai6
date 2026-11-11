import 'package:material_ui/material_ui.dart';

import '../forms/form_host.dart';
import '../forms/form_tools.dart';

/// What narrows a record's form: a box for words in a setting's name or help, a switch for the
/// settings that differ from their defaults (with how many, and a way back to the default for
/// each), and one chip for each section of the form that folds it away.
class FormToolbar extends StatefulWidget {
  const FormToolbar({required this.model, super.key});

  final OoFormModel model;

  @override
  State<FormToolbar> createState() => _FormToolbarState();
}

class _FormToolbarState extends State<FormToolbar> {
  final _query = TextEditingController();

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  String _shown(Object? value) => switch (value) {
    String text => text,
    List items => items.map(_shown).join(', '),
    Map entries =>
      entries.entries
          .map((entry) => '${entry.key}: ${_shown(entry.value)}')
          .join(', '),
    _ => '$value',
  };

  @override
  Widget build(BuildContext context) {
    final model = widget.model;
    final theme = Theme.of(context);
    return ListenableBuilder(
      listenable: model,
      builder: (context, _) {
        final modified = model.modified;
        final groups = [
          for (final group in model.layout())
            if (group.title.isNotEmpty || model.layout().length > 1) group,
        ];
        return Padding(
          padding: const EdgeInsets.only(bottom: 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            spacing: 8,
            children: [
              Wrap(
                spacing: 8,
                runSpacing: 8,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  ConstrainedBox(
                    constraints: const BoxConstraints(
                      minWidth: 200,
                      maxWidth: 320,
                    ),
                    child: TextField(
                      key: const Key('form-filter'),
                      controller: _query,
                      decoration: InputDecoration(
                        isDense: true,
                        prefixIcon: const Icon(Icons.search),
                        hintText: 'Search settings',
                        border: const OutlineInputBorder(),
                        suffixIcon: _query.text.isEmpty
                            ? null
                            : IconButton(
                                tooltip: 'Clear the search',
                                icon: const Icon(Icons.clear),
                                onPressed: () {
                                  _query.clear();
                                  model.query = '';
                                  setState(() {});
                                },
                              ),
                      ),
                      onChanged: (text) {
                        model.query = text;
                        setState(() {});
                      },
                    ),
                  ),
                  FilterChip(
                    key: const Key('form-modified-only'),
                    label: Text('Modified only (${modified.length})'),
                    selected: model.modifiedOnly,
                    onSelected: (on) => model.modifiedOnly = on,
                  ),
                  if (modified.isNotEmpty)
                    TextButton(
                      key: const Key('form-reset-all'),
                      onPressed: model.resetAll,
                      child: const Text('Reset all to defaults'),
                    ),
                ],
              ),
              if (groups.length > 1)
                Wrap(
                  spacing: 6,
                  runSpacing: 4,
                  children: [
                    for (final group in groups)
                      ActionChip(
                        key: Key('form-section-chip-${group.id}'),
                        avatar: Icon(
                          model.isFolded(group.id)
                              ? Icons.chevron_right
                              : Icons.expand_more,
                          size: 16,
                        ),
                        label: Text(
                          [
                            group.title.isEmpty ? 'General' : group.title,
                            if (model.modifiedIn(group.id) case final n
                                when n > 0)
                              '$n modified',
                          ].join(' · '),
                        ),
                        tooltip: model.isFolded(group.id)
                            ? 'Show this section'
                            : 'Fold this section away',
                        onPressed: () => model.toggleFolded(group.id),
                        visualDensity: VisualDensity.compact,
                      ),
                  ],
                ),
              if (model.modifiedOnly)
                Card(
                  margin: EdgeInsets.zero,
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 8,
                    ),
                    child: modified.isEmpty
                        ? Text(
                            'Every setting is at its default.',
                            style: theme.textTheme.bodySmall,
                          )
                        : Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              for (final field in modified)
                                Row(
                                  key: Key('form-modified-${field.path}'),
                                  children: [
                                    Expanded(
                                      child: Text(
                                        '${field.group.isEmpty || field.group == field.title ? '' : '${field.group} › '}'
                                        '${field.title}: ${field.sensitive ? '(hidden)' : _shown(field.value)}',
                                        maxLines: 2,
                                        overflow: TextOverflow.ellipsis,
                                      ),
                                    ),
                                    TextButton(
                                      key: Key('form-reset-${field.path}'),
                                      onPressed: () => model.reset(field.path),
                                      child: const Text('Reset to default'),
                                    ),
                                  ],
                                ),
                            ],
                          ),
                  ),
                ),
            ],
          ),
        );
      },
    );
  }
}
