import 'dart:convert';

import 'package:flutter/foundation.dart' show immutable;
import 'package:http/http.dart' as http;

import '../main.dart';

/// The actions that mean something on an Operation: what a role bound at an Operation, or an
/// Operation's own role, can hold. Universal Exports' `OPERATION_ACTIONS`.
const operationActions = [
  'access.manage',
  'access.read',
  'agent.configure',
  'agent.read',
  'approval.class',
  'approval.decide',
  'audit.read',
  'cable.read',
  'cable.send',
  'mission.control',
  'mission.create',
  'mission.read',
  'pool.manage',
  'pool.read',
  'session.input',
  'session.read',
];

/// A change the gateway refused, in its words.
class AccessRefused implements Exception {
  const AccessRefused(this.reason, {this.stale = false});

  final String reason;

  /// Whether the policy changed since it was read: reload before editing again.
  final bool stale;

  @override
  String toString() => reason;
}

@immutable
class AccessRole {
  const AccessRole({
    required this.name,
    required this.actions,
    this.builtIn = false,
    this.editable = true,
    this.description,
  });

  final String name;
  final List<String> actions;

  /// `chief`, `controller`, `operator` and `auditor`: names an Operation may not reuse.
  final bool builtIn;

  /// `chief` cannot be edited or deleted.
  final bool editable;
  final String? description;

  factory AccessRole.fromJson(Map<String, dynamic> json) => AccessRole(
    name: json['name'] as String,
    actions: (json['actions'] as List<dynamic>).cast<String>(),
    builtIn: json['built_in'] as bool? ?? false,
    editable: json['editable'] as bool? ?? true,
    description: json['description'] as String?,
  );
}

@immutable
class AccessRule {
  const AccessRule({
    required this.subject,
    required this.scope,
    required this.role,
    this.propagate = true,
    this.editor,
  });

  final String subject;
  final String scope;
  final String role;
  final bool propagate;

  /// Who wrote it last, as the gateway recorded.
  final String? editor;

  factory AccessRule.fromJson(Map<String, dynamic> json) => AccessRule(
    subject: json['subject'] as String,
    scope: json['scope'] as String,
    role: json['role'] as String,
    propagate: json['propagate'] as bool? ?? true,
    editor: json['editor'] as String?,
  );

  Map<String, Object> toJson() => {
    'subject': subject,
    'scope': scope,
    'role': role,
    if (!propagate) 'propagate': false,
  };

  AccessRule copyWith({
    String? subject,
    String? scope,
    String? role,
    bool? propagate,
  }) => AccessRule(
    subject: subject ?? this.subject,
    scope: scope ?? this.scope,
    role: role ?? this.role,
    propagate: propagate ?? this.propagate,
  );
}

/// What the gateway shows of a policy: the global one, or one Operation's.
@immutable
class AccessView {
  const AccessView({
    required this.revision,
    required this.writable,
    required this.roles,
    required this.rules,
    required this.delegable,
    this.managedBy,
    this.invalid,
    this.delegableIsDefault = true,
    this.weakenings = const [],
    this.chiefStored = const [],
    this.chiefHost = const [],
    this.orphaned = const [],
    this.inherited = const [],
  });

  final String revision;
  final bool writable;
  final List<AccessRole> roles;
  final List<AccessRule> rules;

  /// The Operation actions an Operation's roles may hold.
  final List<String> delegable;
  final bool delegableIsDefault;

  /// What the cap allows that the default withholds.
  final List<String> weakenings;
  final List<String> chiefStored;

  /// From `unex.cfg`: always chief, never changed here.
  final List<String> chiefHost;
  final String? managedBy;
  final String? invalid;

  /// Rules and roles of an Operation's record that are not applied, with the reason.
  final List<(String, String)> orphaned;

  /// Roles an Operation inherits from readable ancestors: (role, from).
  final List<(String, String)> inherited;

  factory AccessView.fromJson(Map<String, dynamic> json) {
    final cap = json['delegable'];
    return AccessView(
      revision: json['revision'] as String,
      writable: json['writable'] as bool? ?? false,
      managedBy: json['managed_by'] as String?,
      invalid: json['invalid'] as String?,
      roles: [
        for (final role in json['roles'] as List<dynamic>)
          AccessRole.fromJson(role as Map<String, dynamic>),
      ],
      rules: [
        for (final rule in json['rules'] as List<dynamic>)
          AccessRule.fromJson(rule as Map<String, dynamic>),
      ],
      delegable: cap is Map<String, dynamic>
          ? (cap['actions'] as List<dynamic>).cast<String>()
          : (cap as List<dynamic>? ?? const []).cast<String>(),
      delegableIsDefault: cap is Map<String, dynamic>
          ? cap['default'] as bool? ?? true
          : true,
      weakenings: cap is Map<String, dynamic>
          ? (cap['weakenings'] as List<dynamic>? ?? const []).cast<String>()
          : const [],
      chiefStored: ((json['chief'] as Map?)?['stored'] as List<dynamic>? ?? [])
          .cast<String>(),
      chiefHost: ((json['chief'] as Map?)?['host'] as List<dynamic>? ?? [])
          .cast<String>(),
      orphaned: [
        for (final item in json['orphaned'] as List<dynamic>? ?? const [])
          (
            (item as Map<String, dynamic>)['item'] as String,
            item['reason'] as String,
          ),
      ],
      inherited: [
        for (final item in json['inherited'] as List<dynamic>? ?? const [])
          (
            (item as Map<String, dynamic>)['role'] as String,
            item['from'] as String,
          ),
      ],
    );
  }
}

@immutable
class AccountEntry {
  const AccountEntry({
    required this.name,
    required this.groups,
    required this.disabled,
  });

  final String name;
  final List<String> groups;
  final bool disabled;

  factory AccountEntry.fromJson(Map<String, dynamic> json) => AccountEntry(
    name: json['name'] as String,
    groups: (json['groups'] as List<dynamic>? ?? const []).cast<String>(),
    disabled: json['disabled'] as bool? ?? false,
  );
}

/// The answer of the check route: whether, and by which tier, rule and role.
@immutable
class AccessAnswer {
  const AccessAnswer({
    required this.allowed,
    this.tier,
    this.ruleIndex,
    this.role,
  });

  final bool allowed;
  final String? tier;
  final int? ruleIndex;
  final String? role;

  factory AccessAnswer.fromJson(Map<String, dynamic> json) => AccessAnswer(
    allowed: json['allowed'] as bool,
    tier: json['tier'] as String?,
    ruleIndex: json['rule_index'] as int?,
    role: json['role'] as String?,
  );
}

/// The calls of the Access page over one policy: the global one, or [operationId]'s.
class AccessApi {
  const AccessApi(this.board, {this.operationId});

  final MissionBoard board;
  final String? operationId;

  String get _base => operationId == null
      ? '/v1/user/access'
      : '/v1/user/operations/${Uri.encodeComponent(operationId!)}/access';

  Map<String, dynamic> _ok(http.Response response) {
    if (response.statusCode >= 200 && response.statusCode < 300) {
      return response.body.isEmpty
          ? const {}
          : jsonDecode(response.body) as Map<String, dynamic>;
    }
    var reason = 'The gateway refused it (HTTP ${response.statusCode}).';
    try {
      final body = jsonDecode(response.body);
      if (body is Map && body['error'] is String) {
        reason = body['error'] as String;
      }
    } on FormatException {
      // The gateway's plain refusals have no body.
    }
    throw AccessRefused(
      reason,
      stale:
          response.statusCode == 409 &&
              (reason.contains('changed since') || reason.contains('reload')) ||
          response.statusCode == 428,
    );
  }

  Future<AccessView> load() async =>
      AccessView.fromJson(_ok(await board.accessRequest('GET', _base)));

  Future<String> _write(
    String method,
    String tail,
    String revision,
    Object? body,
  ) async =>
      _ok(
            await board.accessRequest(
              method,
              '$_base$tail',
              body: body,
              revision: revision,
            ),
          )['revision']
          as String;

  /// Each write returns the revision it moved to.
  Future<String> setRole(
    String revision,
    String name,
    List<String> actions, {
    String? description,
  }) => _write('PUT', '/roles/${Uri.encodeComponent(name)}', revision, {
    'actions': actions,
    'description': ?description,
  });

  Future<String> removeRole(String revision, String name) =>
      _write('DELETE', '/roles/${Uri.encodeComponent(name)}', revision, null);

  Future<String> setRules(String revision, List<AccessRule> rules) =>
      _write('PUT', '/rules', revision, {
        'rules': [for (final rule in rules) rule.toJson()],
      });

  Future<String> setDelegable(String revision, List<String>? actions) =>
      _write('PUT', '/delegable', revision, {'actions': actions});

  Future<String> setChief(String revision, List<String> subjects) =>
      _write('PUT', '/chief', revision, {'chief': subjects});

  Future<AccessAnswer> check(
    String subject,
    String action,
    String resource,
  ) async {
    final query = Uri(
      queryParameters: {
        'subject': subject,
        'action': action,
        'resource': resource,
      },
    ).query;
    return AccessAnswer.fromJson(
      _ok(await board.accessRequest('GET', '/v1/user/access/check?$query')),
    );
  }

  Future<List<AccountEntry>> accounts() async {
    final response = await board.accessRequest('GET', '/v1/user/accounts');
    if (response.statusCode != 200) _ok(response);
    return [
      for (final account in jsonDecode(response.body) as List<dynamic>)
        AccountEntry.fromJson(account as Map<String, dynamic>),
    ];
  }

  Future<void> putAccount(
    String name, {
    bool? disabled,
    List<String>? groups,
  }) async {
    _ok(
      await board.accessRequest(
        'PUT',
        '/v1/user/accounts/${Uri.encodeComponent(name)}',
        body: {'disabled': ?disabled, 'groups': ?groups},
      ),
    );
  }

  /// The new password, shown once by the caller and kept nowhere.
  Future<String> resetPassword(String name) async {
    final answer = _ok(
      await board.accessRequest(
        'POST',
        '/v1/user/accounts/${Uri.encodeComponent(name)}/secret',
      ),
    );
    return answer['secret'] as String;
  }
}

/// Why a person cannot give a role [action], or `null` when they can: the server's guard
/// stays the authority, this only greys what it would refuse.
String? actionBlock(
  String action, {
  required Set<String> held,
  required bool operationScope,
  required Set<String> cap,
}) {
  if (operationScope) {
    if (!operationActions.contains(action)) {
      return 'An Operation role holds only Operation actions.';
    }
    if (!cap.contains(action)) {
      return 'The global policy does not let Operation roles hold this '
          '(delegation cap).';
    }
  }
  if (!held.contains(action)) {
    return 'You do not hold this action, so you cannot give it.';
  }
  return null;
}
