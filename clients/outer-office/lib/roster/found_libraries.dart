/// Provider libraries a Station found on its plugin paths and did not load, and the approval
/// that makes it load them (`GET /v1/user/stations/{id}/libraries`, `POST .../approve`).
library;

import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../main.dart';
import 'record_editor.dart';
import 'roster_page.dart';

/// One library a Station found and did not load, as it read it without running it.
@immutable
class FoundLibrary {
  const FoundLibrary({
    required this.file,
    required this.digest,
    required this.kinds,
    required this.verdict,
    this.declared = const [],
    this.signed = false,
    this.publisher,
  });

  /// The file name a `plugin` record is keyed by.
  final String file;

  /// `sha256:HEX` of its bytes.
  final String digest;

  /// The Guatiao kinds it declares; the provider IDs inside are known once it loads.
  final List<String> kinds;

  /// Everything else it declares, as `key=value`.
  final List<String> declared;

  /// `unpinned`, or `refused: why` when a record names the file and not these bytes.
  final String verdict;

  /// Whether a signature file sits beside it.
  final bool signed;

  /// The `ed25519:HEX` key, among those the Station's records name, that signed it.
  final String? publisher;

  factory FoundLibrary.fromJson(Map<String, dynamic> json) => FoundLibrary(
    file: json['file'] as String,
    digest: json['digest'] as String,
    kinds: ((json['kinds'] as List<dynamic>?) ?? const []).cast<String>(),
    declared: ((json['declared'] as List<dynamic>?) ?? const []).cast<String>(),
    verdict: json['verdict'] as String? ?? 'unpinned',
    signed: json['signed'] as bool? ?? false,
    publisher: json['publisher'] as String?,
  );
}

/// What the gateway said about an approval: held with what it would do, or stored.
@immutable
class LibraryApproval {
  const LibraryApproval({
    required this.applied,
    this.changes = const [],
    this.weakened = const [],
    this.revision,
  });

  final bool applied;

  /// What will load that did not, in the gateway's words (a held answer only).
  final List<String> changes;

  /// What the record then gives up that the stored ones do not.
  final List<String> weakened;

  /// The configuration revision it was stored at.
  final String? revision;

  factory LibraryApproval.fromJson(
    Map<String, dynamic> json,
  ) => LibraryApproval(
    applied: json['applied'] as bool? ?? json['revision'] != null,
    changes: ((json['changes'] as List<dynamic>?) ?? const []).cast<String>(),
    weakened: ((json['weakened'] as List<dynamic>?) ?? const []).cast<String>(),
    revision: json['revision'] as String?,
  );
}

/// The libraries each Station found and did not load, each with the approval that admits it.
/// The approval writes that Station's own `plugin` record, which it reads at its next start.
class FoundLibrariesPanel extends StatefulWidget {
  const FoundLibrariesPanel({
    required this.board,
    required this.onApproved,
    super.key,
  });

  final MissionBoard board;

  /// Called after a record was stored, so the page reads its records again.
  final VoidCallback onApproved;

  @override
  State<FoundLibrariesPanel> createState() => _FoundLibrariesPanelState();
}

class _FoundLibrariesPanelState extends State<FoundLibrariesPanel> {
  Map<String, List<FoundLibrary>> _found = const {};

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    final found = <String, List<FoundLibrary>>{};
    await Future.wait([
      for (final station in widget.board.stationBoard.stations)
        widget.board.loadFoundLibraries(station.stationId).then<void>(
          (libraries) {
            if (libraries.isNotEmpty) found[station.stationId] = libraries;
          },
          // A Station the person may not read reports nothing to them.
          onError: (Object _) {},
        ),
    ]);
    if (mounted) setState(() => _found = found);
  }

  Future<void> _approve(
    String station,
    FoundLibrary library, {
    required bool byPublisher,
  }) async {
    final LibraryApproval held;
    try {
      held = await widget.board.approveLibrary(
        station,
        library,
        byPublisher: byPublisher,
      );
    } on RosterRefused catch (refused) {
      _say('Refused: ${refused.reason}');
      return;
    } on Object catch (error) {
      _say(error is StateError ? error.message : error.toString());
      return;
    }
    if (!mounted) return;
    final approved = await showDialog<bool>(
      context: context,
      builder: (context) => _ApproveDialog(
        station: station,
        library: library,
        held: held,
        byPublisher: byPublisher,
      ),
    );
    if (approved != true || !mounted) return;
    try {
      await widget.board.approveLibrary(
        station,
        library,
        byPublisher: byPublisher,
        confirm: true,
      );
    } on RosterRefused catch (refused) {
      _say('Refused: ${refused.reason}');
      return;
    } on RosterChanged {
      _say('Someone changed this record meanwhile; nothing was stored.');
      await _load();
      return;
    } on Object catch (error) {
      _say(error is StateError ? error.message : error.toString());
      return;
    }
    _say(
      'Approved ${library.file} for $station; it loads at that Station\'s next start.',
    );
    await _load();
    widget.onApproved();
  }

  void _say(String text) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  @override
  Widget build(BuildContext context) {
    if (_found.isEmpty) return const SizedBox.shrink();
    final theme = Theme.of(context);
    final canApprove = widget.board.canManagePlugins;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
      child: Card(
        margin: EdgeInsets.zero,
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            key: const Key('found-libraries'),
            crossAxisAlignment: CrossAxisAlignment.start,
            spacing: 4,
            children: [
              Text(
                'Libraries found and not loaded',
                style: theme.textTheme.titleSmall,
              ),
              Text(
                canApprove
                    ? 'A Station loads only the libraries its records accept. Approving one '
                          'lets that Station run it from its next start.'
                    : 'Approving a library takes the plugin.manage action.',
                style: theme.textTheme.bodySmall,
              ),
              for (final MapEntry(key: station, value: libraries)
                  in _found.entries) ...[
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Text(
                    'Station $station',
                    style: theme.textTheme.labelLarge,
                  ),
                ),
                for (final library in libraries)
                  Padding(
                    key: Key('found-$station-${library.file}'),
                    padding: const EdgeInsets.symmetric(vertical: 4),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(library.file, style: theme.textTheme.bodyMedium),
                        Text(
                          [
                            library.kinds.join(', '),
                            library.digest,
                            if (library.publisher != null)
                              'signed by ${library.publisher}'
                            else if (library.signed)
                              'signed by a key no record names',
                            library.verdict,
                          ].where((part) => part.isNotEmpty).join(' · '),
                          style: theme.textTheme.bodySmall,
                        ),
                        if (canApprove)
                          Wrap(
                            spacing: 4,
                            children: [
                              TextButton(
                                onPressed: () => unawaited(
                                  _approve(
                                    station,
                                    library,
                                    byPublisher: false,
                                  ),
                                ),
                                child: Text('Approve ${library.file}'),
                              ),
                              if (library.publisher != null)
                                TextButton(
                                  onPressed: () => unawaited(
                                    _approve(
                                      station,
                                      library,
                                      byPublisher: true,
                                    ),
                                  ),
                                  child: Text(
                                    'Trust publisher of ${library.file}',
                                  ),
                                ),
                            ],
                          ),
                      ],
                    ),
                  ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// What approving one library does, as the gateway words it, for the person to confirm.
class _ApproveDialog extends StatelessWidget {
  const _ApproveDialog({
    required this.station,
    required this.library,
    required this.held,
    required this.byPublisher,
  });

  final String station;
  final FoundLibrary library;
  final LibraryApproval held;
  final bool byPublisher;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return AlertDialog(
      title: Text('Let $station run ${library.file}?'),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            spacing: 6,
            children: [
              const Text(
                'This changes what code the Station runs. It is audited with '
                'your name, the file and its digest.',
              ),
              Text('File: ${library.file}'),
              const Text('Digest:'),
              SelectableText(library.digest),
              Text(
                'Kinds: ${library.kinds.isEmpty ? 'none declared' : library.kinds.join(', ')}',
              ),
              if (library.publisher != null)
                SelectableText('Publisher: ${library.publisher}')
              else
                Text(
                  library.signed
                      ? 'Publisher: signed by a key no record names'
                      : 'Publisher: not signed',
                ),
              for (final change in held.changes) Text(change),
              if (held.weakened.isNotEmpty)
                Card(
                  color: colors.errorContainer,
                  child: Padding(
                    padding: const EdgeInsets.all(8),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'This also gives up:',
                          style: TextStyle(color: colors.onErrorContainer),
                        ),
                        for (final line in held.weakened)
                          Text(
                            line,
                            style: TextStyle(color: colors.onErrorContainer),
                          ),
                      ],
                    ),
                  ),
                ),
              const Text('The Station reads its records at its next start.'),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(true),
          child: Text(byPublisher ? 'Trust publisher' : 'Approve digest'),
        ),
      ],
    );
  }
}
