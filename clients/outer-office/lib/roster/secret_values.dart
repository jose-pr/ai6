/// Writing secret values and scope keys through the gateway (`/v1/user/secrets`).
///
/// The gateway cannot encrypt, so a write is a request that a Station answers. The form
/// sends it, then reads the request until it is done or failed. A value is held only by the
/// masked field until it is sent; nothing the form shows is a value.
library;

import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';

import '../main.dart';
import '../shell/operation_names.dart';
import '../theme/oo_theme.dart';

/// A change the gateway refused, in its words.
class SecretsRefused implements Exception {
  const SecretsRefused(this.reason);

  final String reason;

  @override
  String toString() => reason;
}

/// A value the gateway lists: its name, scope and revision, never the value.
@immutable
class SecretValueRow {
  const SecretValueRow({
    required this.name,
    required this.scope,
    required this.revision,
    required this.keyId,
  });

  final String name;
  final String scope;
  final int revision;
  final String keyId;

  factory SecretValueRow.fromJson(Map<String, dynamic> json) => SecretValueRow(
    name: json['name'] as String,
    scope: json['scope'] as String,
    revision: (json['revision'] as num?)?.toInt() ?? 0,
    keyId: json['key_id'] as String? ?? '',
  );
}

/// A scope key the gateway lists, with the Stations it is wrapped for.
@immutable
class SecretScopeRow {
  const SecretScopeRow({
    required this.scope,
    required this.stations,
    required this.rotating,
  });

  final String scope;
  final List<String> stations;
  final bool rotating;

  factory SecretScopeRow.fromJson(Map<String, dynamic> json) => SecretScopeRow(
    scope: json['scope'] as String,
    stations: ((json['stations'] as List<dynamic>?) ?? const []).cast<String>(),
    rotating: json['rotating'] as bool? ?? false,
  );
}

/// The values and scope keys the Registry holds.
@immutable
class SecretListing {
  const SecretListing(this.values, this.scopes);

  final List<SecretValueRow> values;
  final List<SecretScopeRow> scopes;
}

/// Where a request stands: `pending`, `done` or `failed`, with the words about it.
@immutable
class SecretAnswer {
  const SecretAnswer(this.state, {this.lines = const [], this.error});

  final String state;

  /// What was done, as sentences with no value in them (a `done` answer).
  final List<String> lines;

  /// Why it failed (a `failed` answer).
  final String? error;

  bool get finished => state != 'pending';

  factory SecretAnswer.fromJson(Map<String, dynamic> json) {
    final state = json['state'] as String? ?? 'pending';
    final outcome = json['outcome'];
    return SecretAnswer(
      state,
      lines: outcome is Map<String, dynamic>
          ? _outcomeWords(outcome)
          : const [],
      error: json['error'] as String?,
    );
  }
}

List<String> _outcomeWords(Map<String, dynamic> outcome) {
  final verb = outcome['verb'] as String? ?? 'request';
  final name = outcome['name'] as String?;
  final scope = outcome['scope'] as String?;
  final revision = outcome['revision'];
  final keyId = outcome['key_id'] as String?;
  final stations = ((outcome['stations'] as List<dynamic>?) ?? const [])
      .cast<String>();
  return [
    'Done: $verb${name == null ? '' : ' $name'}'
        '${scope == null ? '' : ' in $scope'}'
        '${revision == null ? '' : ', revision $revision'}.',
    if (keyId != null && keyId.isNotEmpty) 'Scope key: $keyId.',
    if (stations.isNotEmpty)
      'Stations that hold the key: ${stations.join(', ')}.',
    for (final note in (outcome['notes'] as List<dynamic>?) ?? const [])
      note as String,
  ];
}

/// The scope a value is written to when the person picks none: the scope a value of that name
/// already has (the gateway refuses another), else the chosen Operation's, else `global`.
String defaultSecretScope({
  String name = '',
  String? operationId,
  List<SecretValueRow> values = const [],
}) {
  for (final value in values) {
    if (value.name == name.trim()) return value.scope;
  }
  if (operationId != null && operationId.isNotEmpty) {
    return 'operation:$operationId';
  }
  return 'global';
}

/// The gateway's secret routes, through the board's own signed-in requests.
class SecretsApi {
  const SecretsApi(this.board);

  final MissionBoard board;

  static const _base = '/v1/user/secrets';

  SecretsRefused _refused(http.Response response) {
    var reason = 'The gateway refused it (HTTP ${response.statusCode}).';
    try {
      final body = jsonDecode(response.body);
      if (body is Map && body['error'] is String) {
        reason = body['error'] as String;
      }
    } on FormatException {
      // The gateway's plain refusals have no body.
    }
    return SecretsRefused(reason);
  }

  Future<SecretListing> list() async {
    final response = await board.accessRequest('GET', _base);
    if (response.statusCode != 200) throw _refused(response);
    final body = jsonDecode(response.body) as Map<String, dynamic>;
    return SecretListing(
      [
        for (final row in body['values'] as List<dynamic>? ?? const [])
          SecretValueRow.fromJson(row as Map<String, dynamic>),
      ],
      [
        for (final row in body['scopes'] as List<dynamic>? ?? const [])
          SecretScopeRow.fromJson(row as Map<String, dynamic>),
      ],
    );
  }

  /// Sends [request] (it has a `verb`) and returns the request's ID.
  Future<String> send(Map<String, Object?> request) async {
    final response = await board.accessRequest(
      'POST',
      '$_base/requests',
      body: request,
    );
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw _refused(response);
    }
    return (jsonDecode(response.body) as Map<String, dynamic>)['id'] as String;
  }

  Future<SecretAnswer> answer(String id) async {
    final response = await board.accessRequest(
      'GET',
      '$_base/requests/${Uri.encodeComponent(id)}',
    );
    if (response.statusCode != 200) throw _refused(response);
    return SecretAnswer.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }
}

/// What the form asks for.
enum SecretVerb {
  put('put', 'Put a value'),
  rotate('rotate', 'Rotate a value'),
  grant('grant', 'Grant a Station'),
  revoke('revoke', 'Revoke a Station'),
  rotateKey('rotate-key', 'Rotate a scope key');

  const SecretVerb(this.wire, this.label);

  /// The request's `verb`.
  final String wire;
  final String label;

  bool get hasValue => this == put || this == rotate;
  bool get hasName => this == put || this == rotate;
  bool get hasScope => this != rotate;
  bool get hasStation => this == grant || this == revoke;
}

/// The values and scope keys the Registry holds, and the form that writes them. The value box
/// is masked and never read back; it is emptied the moment the request is sent.
class SecretValuesPanel extends StatefulWidget {
  const SecretValuesPanel({
    required this.board,
    this.pollInterval = const Duration(seconds: 1),
    this.initiallyExpanded = false,
    super.key,
  });

  final MissionBoard board;

  /// How long to wait between reads of a request that is still pending.
  final Duration pollInterval;
  final bool initiallyExpanded;

  @override
  State<SecretValuesPanel> createState() => _SecretValuesPanelState();
}

class _SecretValuesPanelState extends State<SecretValuesPanel> {
  final _name = TextEditingController();
  final _scope = TextEditingController();
  final _station = TextEditingController();
  final _stations = TextEditingController();
  final _value = TextEditingController();

  SecretListing _listing = const SecretListing([], []);
  SecretVerb _verb = SecretVerb.put;
  String? _operationId;
  bool _scopeChosen = false;
  bool _busy = false;
  String? _error;
  List<String> _result = const [];
  String? _waiting;
  int _round = 0;

  SecretsApi get _api => SecretsApi(widget.board);

  bool get _canWrite => widget.board.canManageRecords('secrets');

  @override
  void initState() {
    super.initState();
    _scope.text = defaultSecretScope();
    unawaited(_load());
  }

  @override
  void dispose() {
    _round++;
    _name.dispose();
    _scope.dispose();
    _station.dispose();
    _stations.dispose();
    _value.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final listing = await _api.list();
      if (mounted) {
        setState(() {
          _listing = listing;
        });
        _suggestScope();
      }
    } on SecretsRefused catch (error) {
      if (mounted) setState(() => _error = error.reason);
    } on Object {
      if (mounted) setState(() => _error = 'Secrets are unavailable.');
    }
  }

  void _suggestScope() {
    if (_scopeChosen || _verb == SecretVerb.rotate) return;
    _scope.text = defaultSecretScope(
      name: _name.text,
      operationId: _operationId,
      values: _listing.values,
    );
  }

  void _pick(SecretValueRow row) {
    setState(() {
      _name.text = row.name;
      _scope.text = row.scope;
      _scopeChosen = true;
      if (_verb != SecretVerb.put && _verb != SecretVerb.rotate) {
        _verb = SecretVerb.rotate;
      }
    });
  }

  static List<String> _split(String text) => [
    for (final part in text.split(RegExp(r'[\n,]')))
      if (part.trim().isNotEmpty) part.trim(),
  ];

  /// The request body for the form as it stands. [value] is passed in so no field of this
  /// state ever holds it.
  Map<String, Object?> _body(String value) {
    final scope = _scope.text.trim();
    return switch (_verb) {
      SecretVerb.put => {
        'verb': 'put',
        'name': _name.text.trim(),
        if (scope.isNotEmpty) 'scope': scope,
        'value': value,
        if (_split(_stations.text) case final stations when stations.isNotEmpty)
          'stations': stations,
      },
      SecretVerb.rotate => {
        'verb': 'rotate',
        'name': _name.text.trim(),
        'value': value,
      },
      SecretVerb.grant => {
        'verb': 'grant',
        'scope': scope,
        'station': _station.text.trim(),
      },
      SecretVerb.revoke => {
        'verb': 'revoke',
        'scope': scope,
        'station': _station.text.trim(),
      },
      SecretVerb.rotateKey => {'verb': 'rotate-key', 'scope': scope},
    };
  }

  Future<void> _submit() async {
    final value = _value.text;
    final Map<String, Object?> body = _body(value);
    _value.clear();
    final round = ++_round;
    setState(() {
      _busy = true;
      _error = null;
      _result = const [];
      _waiting = 'Sending the request.';
    });
    try {
      final id = await _api.send(body);
      while (mounted && round == _round) {
        setState(
          () => _waiting = 'Waiting for a Station to answer request $id.',
        );
        final answer = await _api.answer(id);
        if (!mounted || round != _round) return;
        if (answer.finished) {
          setState(() {
            _waiting = null;
            _busy = false;
            if (answer.state == 'done') {
              _result = answer.lines;
            } else {
              _error =
                  'Failed: ${answer.error ?? 'the Station gave no reason'}';
            }
          });
          await _load();
          return;
        }
        await Future<void>.delayed(widget.pollInterval);
      }
    } on SecretsRefused catch (error) {
      if (mounted && round == _round) {
        setState(() {
          _busy = false;
          _waiting = null;
          _error = error.reason;
        });
      }
    } on Object {
      if (mounted && round == _round) {
        setState(() {
          _busy = false;
          _waiting = null;
          _error = 'The gateway could not be reached.';
        });
      }
    }
  }

  bool get _ready {
    final verb = _verb;
    return (!verb.hasName || _name.text.trim().isNotEmpty) &&
        (!verb.hasValue || _value.text.isNotEmpty) &&
        (!verb.hasScope ||
            _scope.text.trim().isNotEmpty ||
            verb == SecretVerb.put) &&
        (!verb.hasStation || _station.text.trim().isNotEmpty);
  }

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    final muted = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: tokens.muted);
    final operations = widget.board.userSession?.operations ?? const [];
    return ExpansionTile(
      initiallyExpanded: widget.initiallyExpanded,
      title: const Text('Secret values'),
      subtitle: Text(
        'Values are written once, encrypted by a Station, and never shown.',
        style: muted,
      ),
      childrenPadding: const EdgeInsets.fromLTRB(24, 0, 24, 12),
      expandedCrossAxisAlignment: CrossAxisAlignment.start,
      children: [
        ConstrainedBox(
          constraints: BoxConstraints(
            maxHeight: MediaQuery.sizeOf(context).height * 0.6,
          ),
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (_listing.values.isEmpty)
                  Text('No values yet.', style: muted)
                else
                  for (final row in _listing.values)
                    ListTile(
                      dense: true,
                      contentPadding: EdgeInsets.zero,
                      title: Text(row.name),
                      subtitle: Text(
                        '${row.scope}, revision ${row.revision}',
                        style: muted,
                      ),
                      onTap: _canWrite ? () => _pick(row) : null,
                    ),
                for (final scope in _listing.scopes)
                  Text(
                    'Scope key ${scope.scope}: '
                    '${scope.stations.isEmpty ? 'no Stations' : scope.stations.join(', ')}'
                    '${scope.rotating ? ' (rotating)' : ''}',
                    style: muted,
                  ),
                if (_canWrite) ...[
                  const SizedBox(height: 12),
                  DropdownButtonFormField<SecretVerb>(
                    key: const ValueKey('secret-verb'),
                    initialValue: _verb,
                    decoration: const InputDecoration(labelText: 'Action'),
                    items: [
                      for (final verb in SecretVerb.values)
                        DropdownMenuItem(value: verb, child: Text(verb.label)),
                    ],
                    onChanged: _busy
                        ? null
                        : (verb) => setState(() {
                            _verb = verb ?? _verb;
                            _suggestScope();
                          }),
                  ),
                  if (_verb.hasName)
                    TextField(
                      key: const ValueKey('secret-name'),
                      controller: _name,
                      decoration: const InputDecoration(labelText: 'Name'),
                      onChanged: (_) => setState(_suggestScope),
                    ),
                  if (_verb == SecretVerb.put && operations.isNotEmpty)
                    DropdownButtonFormField<String?>(
                      key: const ValueKey('secret-operation'),
                      initialValue: _operationId,
                      decoration: const InputDecoration(
                        labelText: 'Operation (sets the default scope)',
                      ),
                      items: [
                        const DropdownMenuItem<String?>(
                          value: null,
                          child: Text('None'),
                        ),
                        for (final operation in operations)
                          DropdownMenuItem<String?>(
                            value: operation.operationId,
                            child: Text(
                              OperationNames.of(operation.operationId),
                            ),
                          ),
                      ],
                      onChanged: (id) => setState(() {
                        _operationId = id;
                        _scopeChosen = false;
                        _suggestScope();
                      }),
                    ),
                  if (_verb.hasScope)
                    TextField(
                      key: const ValueKey('secret-scope'),
                      controller: _scope,
                      decoration: const InputDecoration(
                        labelText: 'Scope',
                        helperText: 'global, operation:ID or compartment:ID',
                      ),
                      onChanged: (_) => setState(() => _scopeChosen = true),
                    ),
                  if (_verb.hasStation)
                    TextField(
                      key: const ValueKey('secret-station'),
                      controller: _station,
                      decoration: const InputDecoration(labelText: 'Station'),
                      onChanged: (_) => setState(() {}),
                    ),
                  if (_verb == SecretVerb.put)
                    TextField(
                      key: const ValueKey('secret-stations'),
                      controller: _stations,
                      decoration: const InputDecoration(
                        labelText: 'Stations for a new scope key',
                        helperText:
                            'Comma separated; only a scope with no key yet',
                      ),
                    ),
                  if (_verb.hasValue)
                    TextField(
                      key: const ValueKey('secret-value'),
                      controller: _value,
                      obscureText: true,
                      enableSuggestions: false,
                      autocorrect: false,
                      enableInteractiveSelection: false,
                      enableIMEPersonalizedLearning: false,
                      autofillHints: null,
                      decoration: const InputDecoration(
                        labelText: 'Value (sent once, never shown again)',
                      ),
                      onChanged: (_) => setState(() {}),
                    ),
                  const SizedBox(height: 8),
                  Align(
                    alignment: Alignment.centerLeft,
                    child: FilledButton(
                      key: const ValueKey('secret-send'),
                      onPressed: _busy || !_ready ? null : _submit,
                      child: Text(_verb.label),
                    ),
                  ),
                ],
                if (_waiting != null)
                  Semantics(
                    liveRegion: true,
                    child: Text(_waiting!, style: muted),
                  ),
                for (final line in _result)
                  Semantics(liveRegion: true, child: Text(line)),
                if (_error != null)
                  Semantics(
                    liveRegion: true,
                    child: Text(
                      _error!,
                      style: TextStyle(color: tokens.danger),
                    ),
                  ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}
