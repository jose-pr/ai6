import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';

/// One model a harness says an account may use.
@immutable
class DiscoveredModel {
  const DiscoveredModel({required this.id, this.name, this.description});

  /// What a roster names it (`model = "..."`).
  final String id;
  final String? name;
  final String? description;

  factory DiscoveredModel.fromJson(Map<String, dynamic> json) =>
      DiscoveredModel(
        id: json['id'] as String,
        name: json['name'] as String?,
        description: json['description'] as String?,
      );
}

/// What one Station found of one account or route: the models its handler listed, or why it could
/// not say.
@immutable
class DiscoveredModels {
  const DiscoveredModels({
    required this.station,
    required this.source,
    required this.handler,
    required this.known,
    this.unknownBecause,
    this.models = const [],
  });

  final String station;

  /// `pool/account`, or `agent/handler/model` for a route no pool serves.
  final String source;
  final String handler;

  /// False: the handler cannot list, and `models` is empty. Never a guess.
  final bool known;
  final String? unknownBecause;
  final List<DiscoveredModel> models;

  factory DiscoveredModels.fromJson(Map<String, dynamic> json) {
    final listing = json['listing'] as Map<String, dynamic>? ?? const {};
    return DiscoveredModels(
      station: json['station'] as String,
      source: json['source'] as String,
      handler: json['handler'] as String,
      known: listing['known'] as bool? ?? false,
      unknownBecause: listing['unknown_because'] as String?,
      models: [
        for (final model in listing['models'] as List<dynamic>? ?? const [])
          DiscoveredModel.fromJson(model as Map<String, dynamic>),
      ],
    );
  }

  /// `opus, haiku`, or `Unknown: why`.
  String get label => known
      ? (models.isEmpty ? 'Lists no model' : models.map((m) => m.id).join(', '))
      : 'Unknown: ${unknownBecause ?? 'the handler did not say'}';
}

/// The models each account and route may use, as the Stations found them through their handlers.
/// It informs a person choosing a route's model; it makes no route.
class ModelsSection extends StatefulWidget {
  const ModelsSection({required this.board, super.key});

  final MissionBoard board;

  @override
  State<ModelsSection> createState() => _ModelsSectionState();
}

class _ModelsSectionState extends State<ModelsSection> {
  List<DiscoveredModels>? _found;
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final found = await widget.board.loadModels();
      if (mounted) {
        setState(() {
          _found = found;
          _error = null;
        });
      }
    } on StateError catch (error) {
      if (mounted) setState(() => _error = error.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final found = _found;
    final theme = Theme.of(context);
    return Card(
      child: ExpansionTile(
        title: const Text('Models by account'),
        subtitle: Text(
          found == null
              ? (_error ?? 'Asking the Stations…')
              : 'What each account or route may use, as its handler says. '
                    'A handler that cannot list says unknown.',
        ),
        childrenPadding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
        expandedCrossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (found != null && found.isEmpty)
            const Text('No Station has reported any yet.'),
          for (final entry in found ?? const <DiscoveredModels>[])
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '${entry.source} · ${entry.handler} · ${entry.station}',
                    style: theme.textTheme.titleSmall,
                  ),
                  Text(entry.label, style: theme.textTheme.bodySmall),
                ],
              ),
            ),
        ],
      ),
    );
  }
}
