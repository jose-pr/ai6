// ignore_for_file: invalid_use_of_internal_member
// ignore_for_file: implementation_imports

import 'dart:async';
import 'dart:convert';
import 'dart:ui' show AppExitType;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/src/widgets/_window.dart';
import 'package:material_ui/material_ui.dart';
import 'package:http/http.dart' as http;
import 'package:shared_preferences/shared_preferences.dart';
import 'package:guatiao/guatiao.dart' as guatiao;

import 'controls/mission_actions.dart';
import 'green_ink/green_ink_page.dart';
import 'terminal/session_connection.dart';
import 'terminal/session_terminal.dart';
import 'timeline/timeline_page.dart';

const _apiOverride = String.fromEnvironment('MONEYPENNY_API');
const _demoMode = bool.fromEnvironment('MONEYPENNY_DEMO');

Uri _apiOrigin() => _apiOverride.isNotEmpty
    ? Uri.parse(_apiOverride)
    : kIsWeb
    ? Uri.base
    : Uri.parse('http://127.0.0.1:7082');

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  if (kIsWeb) {
    unawaited(_startWeb());
    return;
  }
  _startDesktop();
}

Future<void> _startWeb() async {
  String? terminalRuntimeError;
  if (!_demoMode) {
    try {
      await guatiao.loadModule(Uri.base.resolve('ai6_guatiao.wasm'));
    } on Object {
      terminalRuntimeError = 'The terminal protocol could not be initialized.';
    }
  }
  final preferences = await SharedPreferences.getInstance();
  final board = MissionBoard(
    apiOrigin: _apiOrigin(),
    demo: _demoMode,
    terminalRuntimeError: terminalRuntimeError,
    lightTheme: preferences.getBool('moneypenny.lightTheme') ?? false,
    onThemeChanged: (lightTheme) =>
        preferences.setBool('moneypenny.lightTheme', lightTheme),
  );

  runApp(MoneypennyApp(board: board, desktopWindows: false));
}

void _startDesktop() {
  final preferences = SharedPreferences.getInstance();
  final board = MissionBoard(
    apiOrigin: _apiOrigin(),
    demo: _demoMode,
    onThemeChanged: (lightTheme) async {
      await (await preferences).setBool('moneypenny.lightTheme', lightTheme);
    },
  );
  runWidget(_DesktopWindowHost(board: board));
  unawaited(
    preferences
        .then((preferences) {
          board.restoreTheme(
            preferences.getBool('moneypenny.lightTheme') ?? false,
          );
        })
        .catchError((Object _) {}),
  );
}

class MissionBoard extends ChangeNotifier {
  MissionBoard({
    required this.apiOrigin,
    this.demo = false,
    this.terminalRuntimeError,
    this.lightTheme = false,
    this.onThemeChanged,
    http.Client? client,
    Duration? pollInterval = const Duration(seconds: 3),
  }) : _client = client ?? http.Client() {
    if (demo) {
      sessions = const [
        SessionSummary(
          operationId: 'G',
          missionId: 'mission-007',
          sessionId: 'preview-session-007',
          state: 'working',
          sequence: 18,
        ),
        SessionSummary(
          operationId: 'G',
          missionId: 'mission-007',
          sessionId: 'preview-session-008',
          state: 'blocked',
          sequence: 7,
          exposure: ['tool:WebFetch'],
        ),
        SessionSummary(
          operationId: 'L',
          missionId: 'mission-021',
          sessionId: 'preview-session-021',
          state: 'done',
          sequence: 32,
        ),
      ];
      missionBriefs = const [
        MissionBriefSummary(
          operationId: 'G',
          missionId: 'mission-007',
          prompt: 'Review the requested change and report findings.',
          agentId: 'technical-officer',
          roleId: 'case-officer',
          handlerProvider: 'claude',
          terminalProvider: 'pty',
          workingDirectory: 'C:/operations/field',
          model: 'claude-sonnet',
          effort: 'high',
          environmentKeys: ['AI6_MISSION_ID'],
          stationId: 'station-control-01',
          theaterId: 'local',
        ),
      ];
      missionAttempts = const {
        ('G', 'mission-007'): [
          MissionAttemptSummary(
            attempt: 1,
            station: 'station-remote-02',
            theater: 'local',
            status: 'lost',
            lost:
                'Station "station-remote-02" went dark with Theater "local"; '
                'its harness ran on that Station\'s own host',
            checkpointBranch: 'mission-007-review',
          ),
          MissionAttemptSummary(
            attempt: 2,
            station: 'station-control-01',
            theater: 'local',
            status: 'active',
          ),
        ],
      };
      auditEntries = const [
        AuditEntrySummary(
          id: 'preview-audit-001',
          actor: 'operator-007',
          action: 'mission.create.requested',
          operation: 'G',
          target: 'moneypenny--mission-007',
          at: 1_798_000_000,
        ),
      ];
      userSession = const UserSessionSummary(
        personId: 'preview-operator',
        name: 'preview-operator',
        displayName: 'Preview Operator',
        groups: ['preview'],
        expiresAt: 0,
        operations: [
          UserOperationAccess(
            operationId: 'G',
            actions: [
              'mission.read',
              'mission.control',
              'session.read',
              'audit.read',
              'agent.read',
            ],
          ),
        ],
        stations: [
          UserStationAccess(
            stationId: 'station-control-01',
            actions: ['station.read', 'station.configure'],
          ),
        ],
      );
      stationBoard = const StationBoardSummary(
        headquartersStation: 'station-control-01',
        stations: [
          StationSummary(
            stationId: 'station-control-01',
            healthy: true,
            headquarters: true,
            seen: 1_798_000_000,
            roles: ['control', 'execution'],
            running: 2,
            capacity: 4,
            operations: ['G'],
            theaters: [
              TheaterSummary(id: 'local', access: 'local'),
              TheaterSummary(
                id: 'build-box',
                access: 'ssh',
                host: 'build.example.net',
                labels: {'os': 'linux'},
              ),
            ],
          ),
          StationSummary(
            stationId: 'station-remote-02',
            healthy: false,
            headquarters: false,
            seen: 1_798_000_000,
            roles: ['execution'],
            running: 0,
            capacity: 4,
            operations: ['L'],
          ),
        ],
      );
      loading = false;
      return;
    }
    unawaited(refresh());
    if (pollInterval != null) {
      _poll = Timer.periodic(pollInterval, (_) => unawaited(refresh()));
    }
  }

  final Uri apiOrigin;
  final bool demo;
  final String? terminalRuntimeError;
  final Future<void> Function(bool lightTheme)? onThemeChanged;
  final http.Client _client;
  Timer? _poll;
  List<SessionSummary> sessions = const [];
  List<MissionBriefSummary> missionBriefs = const [];

  /// Every attempt at each Mission, by (Operation, Mission), first first.
  Map<(String, String), List<MissionAttemptSummary>> missionAttempts = const {};
  List<AuditEntrySummary> auditEntries = const [];
  UserSessionSummary? userSession;
  StationBoardSummary stationBoard = const StationBoardSummary(
    headquartersStation: null,
    stations: [],
  );
  String? error;
  String? loginError;
  String? accessToken;
  bool lightTheme = false;
  bool loading = true;

  void toggleTheme() {
    restoreTheme(!lightTheme);
    final persist = onThemeChanged;
    if (persist != null) {
      unawaited(persist(lightTheme).catchError((Object _) {}));
    }
  }

  void restoreTheme(bool lightTheme) {
    if (this.lightTheme == lightTheme) return;
    this.lightTheme = lightTheme;
    notifyListeners();
  }

  Future<UserSessionSummary> loadUserSession() async {
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before viewing account details.');
    }
    final response = await _client.get(
      apiOrigin.resolve('/v1/user/profile'),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused account details.');
    }
    final profile = UserSessionSummary.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
    userSession = profile;
    notifyListeners();
    return profile;
  }

  bool canReadMissions(String operationId) =>
      userSession?.operations.any(
        (operation) =>
            operation.operationId == operationId &&
            operation.actions.contains('mission.read'),
      ) ??
      false;

  /// Operations of the Station board whose Missions this account may read, so a
  /// placement preview can be asked for them.
  List<String> get previewOperations =>
      (stationBoard.stations.expand((station) => station.operations).toSet()
            ..removeWhere((operation) => !canReadMissions(operation)))
          .toList()
        ..sort();

  Future<PlacementPreview> loadPlacementPreview(String operationId) async {
    if (demo) {
      return const PlacementPreview(
        operationId: 'G',
        policyStation: 'station-control-01',
        choices: [
          PlacementChoice(
            station: 'station-control-01',
            theater: 'local',
            agent: '007',
          ),
        ],
        rejections: [
          PlacementRejection(
            station: 'station-remote-02',
            reason: 'it has not checked in for 120 seconds (gone dark)',
          ),
        ],
      );
    }
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before previewing placement.');
    }
    final response = await _client.get(
      apiOrigin.resolve(
        '/v1/user/operations/${Uri.encodeComponent(operationId)}/placement-preview',
      ),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused the placement preview.');
    }
    return PlacementPreview.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  bool canCreateMission(String operationId) =>
      userSession?.operations.any(
        (operation) =>
            operation.operationId == operationId &&
            operation.actions.contains('mission.create'),
      ) ??
      false;

  bool canControlMission(String operationId) =>
      userSession?.operations.any(
        (operation) =>
            operation.operationId == operationId &&
            operation.actions.contains('mission.control'),
      ) ??
      false;

  /// The latest control asked for each Mission, by Operation and Mission ID.
  Map<(String, String), ControlStatusSummary> controls = const {};
  int _controlSerial = 0;

  void _recordControl(ControlStatusSummary status) {
    controls = Map.unmodifiable({
      ...controls,
      (status.operationId, status.missionId): status,
    });
    notifyListeners();
  }

  /// Asks the Station holding a Mission to cancel, retry, resume or interrupt it, and
  /// follows the control for up to [wait] until the Station reports its outcome.
  Future<ControlStatusSummary> requestControl({
    required String operationId,
    required String missionId,
    required String action,
    String? agent,
    String? message,
    String? sessionId,
    Duration wait = const Duration(seconds: 10),
    Duration pollEvery = const Duration(milliseconds: 500),
  }) async {
    if (demo) {
      throw StateError('Mission controls are unavailable in preview.');
    }
    if (!canControlMission(operationId)) {
      throw StateError(
        'This account cannot control Missions in this Operation.',
      );
    }
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before controlling a Mission.');
    }
    final path =
        '/v1/user/operations/${Uri.encodeComponent(operationId)}'
        '/missions/${Uri.encodeComponent(missionId)}/actions';
    final response = await _client.post(
      apiOrigin.resolve(path),
      headers: {
        'authorization': 'Bearer $token',
        'content-type': 'application/json',
      },
      body: jsonEncode({
        'action': action,
        'idempotency_key':
            'moneypenny-${DateTime.now().microsecondsSinceEpoch}-${_controlSerial++}',
        'agent': ?agent,
        'message': ?message,
        'session_id': ?sessionId,
      }),
    );
    if (response.statusCode != 202 && response.statusCode != 200) {
      throw StateError(switch (response.statusCode) {
        403 => 'This account cannot control Missions in this Operation.',
        404 =>
          'Universal Exports does not know this Mission in this Operation.',
        409 =>
          'The Mission moved on to another Session; refresh and try again.',
        503 => 'No Station can take the control now; try again shortly.',
        _ => 'The gateway refused the control.',
      });
    }
    var status = ControlStatusSummary.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
    _recordControl(status);
    final deadline = DateTime.now().add(wait);
    while (status.isOpen && DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(pollEvery);
      final polled = await _client.get(
        apiOrigin.resolve('$path/${Uri.encodeComponent(status.controlId)}'),
        headers: {'authorization': 'Bearer $token'},
      );
      if (polled.statusCode != 200) break;
      status = ControlStatusSummary.fromJson(
        jsonDecode(polled.body) as Map<String, dynamic>,
      );
      _recordControl(status);
    }
    await refresh();
    return status;
  }

  List<String> get proposalOperations => demo
      ? const ['G']
      : userSession?.operations
                .where((operation) => canCreateMission(operation.operationId))
                .map((operation) => operation.operationId)
                .toList(growable: false) ??
            const [];

  Future<List<MissionProposalSummary>> loadMissionProposals(
    String operationId,
  ) async {
    if (demo) {
      return const [
        MissionProposalSummary(
          id: 'proposal-preview-001',
          operationId: 'G',
          parentMissionId: 'mission-007',
          proposerAgentId: 'coordinator-007',
          proposerSessionId: 'preview-session-007',
          title: 'Review the role catalog',
          body: 'Report which roles have no qualified Agent.',
          role: 'analyst',
          tags: ['configuration', 'review'],
          priority: 2,
          createdAt: 1_798_000_000,
        ),
      ];
    }
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before reviewing Mission proposals.');
    }
    final response = await _client.get(
      apiOrigin.resolve(
        '/v1/user/operations/${Uri.encodeComponent(operationId)}/mission-proposals',
      ),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused Mission proposals.');
    }
    return (jsonDecode(response.body) as List<dynamic>)
        .map(
          (proposal) =>
              MissionProposalSummary.fromJson(proposal as Map<String, dynamic>),
        )
        .toList(growable: false);
  }

  Future<void> decideMissionProposal({
    required String operationId,
    required String proposalId,
    required bool approve,
  }) async {
    if (demo) {
      throw StateError('Proposal decisions are unavailable in preview.');
    }
    if (!canCreateMission(operationId)) {
      throw StateError(
        'This account cannot authorize Missions in this Operation.',
      );
    }
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before deciding on a Mission proposal.');
    }
    final action = approve ? 'approve' : 'reject';
    final response = await _client.post(
      apiOrigin.resolve(
        '/v1/user/operations/${Uri.encodeComponent(operationId)}'
        '/mission-proposals/${Uri.encodeComponent(proposalId)}/$action',
      ),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != (approve ? 202 : 200)) {
      throw StateError('The gateway refused the Mission proposal decision.');
    }
    await refresh();
  }

  /// Operations whose activity timeline this account may read (`audit.read`).
  List<String> get timelineOperations =>
      (userSession?.operations
                .where((operation) => operation.actions.contains('audit.read'))
                .map((operation) => operation.operationId)
                .toList() ??
            <String>[])
        ..sort();

  /// One page of an Operation's activity timeline, newest first.
  Future<TimelinePageSummary> loadTimeline({
    required String operationId,
    String? missionId,
    String? cursor,
    int limit = 50,
  }) async {
    if (demo) {
      return TimelinePageSummary(
        items:
            [
                  TimelineItemSummary(
                    id: 'event:preview-2',
                    at: 1_798_000_120,
                    kind: 'control',
                    action: 'resume.applied',
                    operationId: operationId,
                    stationId: 'station-control-01',
                    missionId: 'mission-007',
                    actor: 'operator-007',
                    detail: const {'state': 'working'},
                  ),
                  TimelineItemSummary(
                    id: 'event:preview-1',
                    at: 1_798_000_060,
                    kind: 'lifecycle',
                    action: 'blocked',
                    operationId: operationId,
                    stationId: 'station-control-01',
                    missionId: 'mission-007',
                    sessionId: 'preview-session-008',
                  ),
                ]
                .where(
                  (item) => missionId == null || item.missionId == missionId,
                )
                .toList(growable: false),
      );
    }
    final token = accessToken;
    if (token == null) throw StateError('Sign in before viewing the timeline.');
    final response = await _client.get(
      apiOrigin
          .resolve(
            '/v1/user/operations/${Uri.encodeComponent(operationId)}/timeline',
          )
          .replace(
            queryParameters: {
              'limit': '$limit',
              'mission': ?missionId,
              'cursor': ?cursor,
            },
          ),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused the Operation timeline.');
    }
    return TimelinePageSummary.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  List<String> _operationsWith(String action) {
    if (demo) return const ['G'];
    return (userSession?.operations
              .where((operation) => operation.actions.contains(action))
              .map((operation) => operation.operationId)
              .toList() ??
          <String>[])
      ..sort();
  }

  /// Operations whose green ink requests this account may decide (`approval.decide`).
  List<String> get greenInkOperations => _operationsWith('approval.decide');

  /// Operations whose class authorisations this account manages (`approval.class`).
  List<String> get classAuthorizationOperations =>
      _operationsWith('approval.class');

  Future<http.Response> _userRequest(
    String method,
    String path, {
    Object? body,
  }) async {
    final token = accessToken;
    if (token == null) throw StateError('Sign in first.');
    final request = http.Request(method, apiOrigin.resolve(path))
      ..headers['authorization'] = 'Bearer $token';
    if (body != null) {
      request.headers['content-type'] = 'application/json';
      request.body = jsonEncode(body);
    }
    return http.Response.fromStream(await _client.send(request));
  }

  /// The steps waiting for green ink in an Operation, oldest first.
  Future<List<GreenInkRequestSummary>> loadGreenInk(String operationId) async {
    if (demo) {
      return [
        GreenInkRequestSummary(
          id: 'green-ink-preview',
          operationId: operationId,
          missionId: 'mission-007',
          sessionId: 'preview-session-007',
          agent: '007',
          kind: 'push',
          target: 'origin:refs/heads/main',
          reason: 'The release branch passed review; push it to main.',
          requestedAt: 1_798_000_000,
          expiresAt: 1_798_086_400,
        ),
      ];
    }
    final response = await _userRequest(
      'GET',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}/green-ink',
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused the green ink inbox.');
    }
    return [
      for (final request in jsonDecode(response.body) as List<dynamic>)
        GreenInkRequestSummary.fromJson(request as Map<String, dynamic>),
    ];
  }

  /// Signs or refuses one request; an approval lasts `expiresInSecs` (one hour when null).
  Future<void> decideGreenInk({
    required String operationId,
    required String requestId,
    required bool approve,
    int? expiresInSecs,
    String? note,
  }) async {
    if (demo) {
      throw StateError('Green ink decisions are unavailable in preview.');
    }
    final response = await _userRequest(
      'POST',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}'
          '/green-ink/${Uri.encodeComponent(requestId)}/'
          '${approve ? 'approve' : 'reject'}',
      body: approve
          ? {'expires_in_secs': ?expiresInSecs, 'note': ?note}
          : {'reason': ?note},
    );
    switch (response.statusCode) {
      case 200:
        await refresh();
      case 403:
        throw StateError(
          'Your account lacks approval.decide in this Operation.',
        );
      case 404 || 409:
        throw StateError('The request was already decided or has expired.');
      default:
        throw StateError('The gateway refused the green ink decision.');
    }
  }

  /// An Operation's class authorisations, oldest first.
  Future<List<ClassAuthorizationSummary>> loadClassAuthorizations(
    String operationId,
  ) async {
    if (demo) {
      return [
        ClassAuthorizationSummary(
          id: 'class-preview',
          kinds: const ['push'],
          targetPattern: 'origin:refs/heads/ai6/*',
          agents: const ['007'],
          roles: const [],
          clearanceMin: 'field',
          maxUses: 10,
          uses: 3,
          expiresAt: 1_798_086_400,
          signer: 'operator-007',
        ),
      ];
    }
    final response = await _userRequest(
      'GET',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}/class-authorizations',
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused class authorisations.');
    }
    return [
      for (final record in jsonDecode(response.body) as List<dynamic>)
        ClassAuthorizationSummary.fromJson(record as Map<String, dynamic>),
    ];
  }

  Future<void> revokeClassAuthorization({
    required String operationId,
    required String id,
  }) async {
    if (demo) throw StateError('Revoking is unavailable in preview.');
    final response = await _userRequest(
      'DELETE',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}'
          '/class-authorizations/${Uri.encodeComponent(id)}',
    );
    if (response.statusCode != 200) {
      throw StateError(
        'The gateway refused to revoke the class authorisation.',
      );
    }
  }

  bool get canReadAudit =>
      (userSession?.operations.any(
            (operation) => operation.actions.contains('audit.read'),
          ) ??
          false) ||
      (userSession?.stations.any(
            (station) => station.actions.contains('audit.read'),
          ) ??
          false);

  List<String> get readableStations =>
      userSession?.stations
          .where((station) => station.actions.contains('station.read'))
          .map((station) => station.stationId)
          .toList(growable: false) ??
      const [];

  bool canConfigureStation(String stationId) =>
      userSession?.stations.any(
        (station) =>
            station.stationId == stationId &&
            station.actions.contains('station.configure'),
      ) ??
      false;

  Future<StationSettingsSummary> loadStationSettings(String stationId) async {
    if (demo) {
      return StationSettingsSummary(
        stationId: stationId,
        maxConcurrentMissions: 4,
        configured: false,
        executionEnabled: true,
      );
    }
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before viewing Station settings.');
    }
    final response = await _client.get(
      apiOrigin.resolve('/v1/user/stations/$stationId/settings'),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused Station settings.');
    }
    return StationSettingsSummary.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  Future<void> saveStationSettings(
    String stationId,
    int maxConcurrentMissions,
  ) async {
    if (demo) {
      throw StateError('Station settings are unavailable in preview.');
    }
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before changing Station settings.');
    }
    final response = await _client.put(
      apiOrigin.resolve('/v1/user/stations/$stationId/settings'),
      headers: {
        'authorization': 'Bearer $token',
        'content-type': 'application/json',
      },
      body: jsonEncode({'max_concurrent_missions': maxConcurrentMissions}),
    );
    if (response.statusCode != 204) {
      throw StateError('The gateway refused the Station settings update.');
    }
  }

  bool canReadAgentRoles(String operationId) =>
      userSession?.operations.any(
        (operation) =>
            operation.operationId == operationId &&
            operation.actions.contains('agent.read'),
      ) ??
      false;

  bool canConfigureAgentRoles(String operationId) =>
      userSession?.operations.any(
        (operation) =>
            operation.operationId == operationId &&
            operation.actions.contains('agent.configure'),
      ) ??
      false;

  Future<List<AgentRoleSummary>> loadAgentRoles(String operationId) async {
    if (demo) {
      return const [
        AgentRoleSummary(
          id: 'case-officer',
          purpose: 'Coordinate routine Mission work.',
          policy: 'Follow the Operation policy.',
          operationOverride: false,
        ),
      ];
    }
    final token = accessToken;
    if (token == null) throw StateError('Sign in before managing Agent roles.');
    final response = await _client.get(
      apiOrigin.resolve('/v1/user/operations/$operationId/agent-roles'),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused Agent role settings.');
    }
    return (jsonDecode(response.body) as List<dynamic>)
        .map((role) => AgentRoleSummary.fromJson(role as Map<String, dynamic>))
        .toList(growable: false);
  }

  Future<void> saveAgentRole(String operationId, AgentRoleSummary role) async {
    if (demo) {
      throw StateError('Agent role changes are unavailable in preview.');
    }
    final token = accessToken;
    if (token == null) throw StateError('Sign in before managing Agent roles.');
    final response = await _client.put(
      apiOrigin.resolve(
        '/v1/user/operations/$operationId/agent-roles/${role.id}',
      ),
      headers: {
        'authorization': 'Bearer $token',
        'content-type': 'application/json',
      },
      body: jsonEncode(role.toJson()),
    );
    if (response.statusCode != 204) {
      throw StateError('The gateway refused the Agent role update.');
    }
  }

  Future<void> deleteAgentRole(String operationId, String roleId) async {
    if (demo) {
      throw StateError('Agent role changes are unavailable in preview.');
    }
    final token = accessToken;
    if (token == null) throw StateError('Sign in before managing Agent roles.');
    final response = await _client.delete(
      apiOrigin.resolve('/v1/user/operations/$operationId/agent-roles/$roleId'),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 204) {
      throw StateError('The gateway refused the Agent role removal.');
    }
  }

  Future<List<AuditEntrySummary>> loadAudit({int limit = 100}) async {
    if (demo) {
      return auditEntries;
    }
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before viewing the audit log.');
    }
    final response = await _client.get(
      apiOrigin
          .resolve('/v1/user/audit')
          .replace(queryParameters: {'limit': '$limit'}),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused audit records.');
    }
    final records = (jsonDecode(response.body) as List<dynamic>)
        .map(
          (record) =>
              AuditEntrySummary.fromJson(record as Map<String, dynamic>),
        )
        .toList(growable: false);
    auditEntries = records;
    notifyListeners();
    return records;
  }

  Future<void> refresh() async {
    if (demo || accessToken == null) {
      notifyListeners();
      return;
    }
    try {
      final responses = await Future.wait([
        _client.get(
          apiOrigin.resolve('/v1/board'),
          headers: {'authorization': 'Bearer $accessToken'},
        ),
        _client.get(
          apiOrigin.resolve('/v1/missions'),
          headers: {'authorization': 'Bearer $accessToken'},
        ),
        _client.get(
          apiOrigin.resolve('/v1/stations'),
          headers: {'authorization': 'Bearer $accessToken'},
        ),
        _client.get(
          apiOrigin.resolve('/v1/user/profile'),
          headers: {'authorization': 'Bearer $accessToken'},
        ),
      ]);
      if (responses.any((response) => response.statusCode != 200)) {
        throw StateError(
          'Universal Exports returned an unsuccessful response.',
        );
      }
      final body = jsonDecode(responses[0].body) as List<dynamic>;
      sessions = body
          .map((item) => SessionSummary.fromJson(item as Map<String, dynamic>))
          .toList(growable: false);
      final missions = jsonDecode(responses[1].body) as List<dynamic>;
      missionBriefs = missions
          .map(
            (item) => MissionBriefSummary.fromMissionJson(
              item as Map<String, dynamic>,
            ),
          )
          .whereType<MissionBriefSummary>()
          .toList(growable: false);
      missionAttempts = {
        for (final item in missions.cast<Map<String, dynamic>>())
          (item['operation_id'] as String, item['mission_id'] as String): [
            for (final attempt
                in (item['attempts'] as List<dynamic>? ?? const []))
              MissionAttemptSummary.fromJson(attempt as Map<String, dynamic>),
          ],
      };
      stationBoard = StationBoardSummary.fromJson(
        jsonDecode(responses[2].body) as Map<String, dynamic>,
      );
      userSession = UserSessionSummary.fromJson(
        jsonDecode(responses[3].body) as Map<String, dynamic>,
      );
      error = null;
    } on Object {
      error = 'Could not load Stations from Universal Exports.';
    } finally {
      loading = false;
      notifyListeners();
    }
  }

  List<MissionAttemptSummary> attempts(String operationId, String missionId) =>
      missionAttempts[(operationId, missionId)] ?? const [];

  MissionBriefSummary? missionBrief(String operationId, String missionId) =>
      missionBriefs
          .where(
            (mission) =>
                mission.operationId == operationId &&
                mission.missionId == missionId,
          )
          .firstOrNull;

  Future<String> createMission({
    required String operationId,
    required String title,
    required String prompt,
  }) async {
    if (demo) {
      throw StateError('Mission creation is unavailable in preview mode.');
    }
    final token = accessToken;
    if (token == null) throw StateError('Sign in before creating a Mission.');
    final response = await _client.post(
      apiOrigin.resolve('/v1/user/operations/$operationId/missions'),
      headers: {
        'authorization': 'Bearer $token',
        'content-type': 'application/json',
      },
      body: jsonEncode({'title': title, 'prompt': prompt}),
    );
    if (response.statusCode != 202) {
      throw StateError('The gateway refused the Mission brief.');
    }
    final body = jsonDecode(response.body) as Map<String, dynamic>;
    await refresh();
    return body['mission_id'] as String;
  }

  Future<void> login(String name, String secret) async {
    await _login({'name': name, 'secret': secret});
  }

  Future<void> loginViaProxy() async {
    await _login(const <String, Object>{});
  }

  Future<void> _login(Map<String, Object> credentials) async {
    loginError = null;
    notifyListeners();
    try {
      final response = await _client.post(
        apiOrigin.resolve('/v1/auth/login'),
        headers: {'content-type': 'application/json'},
        body: jsonEncode(credentials),
      );
      if (response.statusCode != 201) {
        loginError = credentials.isEmpty
            ? 'Organization sign-in was rejected. Open Moneypenny through your authenticated organization proxy and check its gateway configuration.'
            : 'Sign-in was rejected. Check the account and gateway.';
        notifyListeners();
        return;
      }
      final body = jsonDecode(response.body) as Map<String, dynamic>;
      accessToken = body['token'] as String;
      loading = true;
      await refresh();
    } on Object {
      accessToken = null;
      loginError = 'Could not connect to Universal Exports.';
      loading = false;
      notifyListeners();
    }
  }

  Future<void> logout() async {
    final token = accessToken;
    accessToken = null;
    sessions = const [];
    userSession = null;
    error = null;
    loginError = null;
    loading = false;
    notifyListeners();
    if (token != null) {
      try {
        await _client.post(
          apiOrigin.resolve('/v1/auth/logout'),
          headers: {'authorization': 'Bearer $token'},
        );
      } on Object {
        // The in-memory credential is cleared even when the gateway is offline.
      }
    }
  }

  Future<String?> prepareSessionSocket(
    SessionSummary session, {
    required bool write,
  }) async {
    final token = accessToken;
    if (token == null) throw StateError('Sign in before opening a Session.');
    if (kIsWeb && apiOrigin.origin != Uri.base.origin) {
      throw StateError('Web Session views require the same gateway origin.');
    }
    final grant = await _client.post(
      apiOrigin.resolve('/v1/user/attach-grants'),
      headers: {
        'authorization': 'Bearer $token',
        'content-type': 'application/json',
      },
      body: jsonEncode({
        'operation_id': session.operationId,
        'mission_id': session.missionId,
        'session_id': session.sessionId,
        'mode': write ? 'write' : 'read',
      }),
    );
    if (grant.statusCode != 201) {
      throw StateError('The gateway refused the Session attachment.');
    }
    final grantToken =
        (jsonDecode(grant.body) as Map<String, dynamic>)['token'] as String;
    final headers = <String, String>{'content-type': 'application/json'};
    if (!kIsWeb) {
      headers['origin'] = apiOrigin.origin;
      headers['x-forwarded-proto'] = 'https';
    }
    final exchange = await _client.post(
      apiOrigin.resolve('/v1/attach'),
      headers: headers,
      body: jsonEncode({'token': grantToken}),
    );
    if (exchange.statusCode != 204) {
      throw StateError('The gateway refused the Session attachment.');
    }
    if (kIsWeb) return null;
    final setCookie = exchange.headers['set-cookie'];
    final cookie = setCookie == null
        ? null
        : RegExp(r'(?:^|;\s*)ai6_attach=([^;]+)').firstMatch(setCookie);
    if (cookie == null) {
      throw StateError('The gateway did not issue a Session cookie.');
    }
    return 'ai6_attach=${cookie.group(1)}';
  }

  void applyLifecycle(
    String sessionId,
    String state,
    int sequence, {
    String? reason,
  }) {
    final index = sessions.indexWhere((item) => item.sessionId == sessionId);
    if (index < 0 || sequence <= sessions[index].sequence) return;
    final updated = sessions.toList();
    updated[index] = sessions[index].copyWith(
      state: state,
      sequence: sequence,
      reason: reason,
    );
    sessions = List.unmodifiable(updated);
    notifyListeners();
  }

  @override
  void dispose() {
    _poll?.cancel();
    _client.close();
    super.dispose();
  }
}

class StationBoardSummary {
  const StationBoardSummary({
    required this.headquartersStation,
    required this.stations,
  });

  final String? headquartersStation;
  final List<StationSummary> stations;

  factory StationBoardSummary.fromJson(Map<String, dynamic> json) =>
      StationBoardSummary(
        headquartersStation: json['headquarters_station'] as String?,
        stations: (json['stations'] as List<dynamic>)
            .map(
              (station) =>
                  StationSummary.fromJson(station as Map<String, dynamic>),
            )
            .toList(growable: false),
      );
}

class StationSummary {
  const StationSummary({
    required this.stationId,
    required this.healthy,
    required this.headquarters,
    required this.seen,
    required this.roles,
    required this.running,
    required this.capacity,
    required this.operations,
    this.theaters = const [],
  });

  final String stationId;
  final bool healthy;
  final bool headquarters;
  final double seen;
  final List<String> roles;
  final int running;
  final int capacity;
  final List<String> operations;
  final List<TheaterSummary> theaters;

  factory StationSummary.fromJson(Map<String, dynamic> json) => StationSummary(
    stationId: json['station_id'] as String,
    healthy: json['healthy'] as bool,
    headquarters: json['headquarters'] as bool,
    seen: (json['seen'] as num).toDouble(),
    roles: (json['roles'] as List<dynamic>).cast<String>(),
    running: json['running'] as int,
    capacity: json['capacity'] as int,
    operations: (json['operations'] as List<dynamic>).cast<String>(),
    theaters: ((json['theaters'] as List<dynamic>?) ?? const [])
        .map(
          (theater) => TheaterSummary.fromJson(theater as Map<String, dynamic>),
        )
        .toList(growable: false),
  );
}

/// What a Station's transport can do on a Theater.
class TheaterCapabilities {
  const TheaterCapabilities({
    this.terminal = false,
    this.run = false,
    this.files = false,
    this.forward = false,
    this.os,
    this.arch,
    this.notes = const [],
  });

  final bool terminal;
  final bool run;
  final bool files;
  final bool forward;
  final String? os;
  final String? arch;

  /// Facts the transport wants an operator to see, such as a host key to pin.
  final List<String> notes;

  factory TheaterCapabilities.fromJson(Map<String, dynamic> json) =>
      TheaterCapabilities(
        terminal: json['terminal'] as bool? ?? false,
        run: json['run'] as bool? ?? false,
        files: json['files'] as bool? ?? false,
        forward: json['forward'] as bool? ?? false,
        os: json['os'] as String?,
        arch: json['arch'] as String?,
        notes: ((json['notes'] as List<dynamic>?) ?? const []).cast<String>(),
      );

  /// The abilities that are present, then the host's platform.
  String get description {
    final can = [
      if (terminal) 'terminal',
      if (run) 'run',
      if (files) 'files',
      if (forward) 'forward',
    ];
    final platform = [os, arch].whereType<String>().join('/');
    return [
      can.isEmpty ? 'no abilities reported' : can.join(', '),
      if (platform.isNotEmpty) platform,
    ].join(' · ');
  }
}

/// A host or sandbox a Station reaches through a transport (`local`, `ssh` or another
/// provider), with what that transport can do there.
class TheaterSummary {
  const TheaterSummary({
    required this.id,
    required this.access,
    this.host,
    this.provider,
    this.labels = const {},
    this.capabilities,
    this.agents = 'auto',
    this.egress = 'direct',
    this.harnesses = const [],
    this.unavailable,
    this.weakened = const [],
  });

  final String id;
  final String access;
  final String? host;
  final String? provider;
  final Map<String, String> labels;
  final TheaterCapabilities? capabilities;

  /// Where the harness may run: `auto`, `local` or `remote-hands`.
  final String agents;

  /// How the Theater reaches the provider's API: `direct`, `proxy`, `station` or `none`.
  final String egress;
  final List<String> harnesses;

  /// Why the Station cannot reach the Theater now, when it cannot.
  final String? unavailable;

  /// Settings looser than their defaults.
  final List<String> weakened;

  factory TheaterSummary.fromJson(Map<String, dynamic> json) => TheaterSummary(
    id: json['id'] as String,
    access: json['access'] as String,
    host: json['host'] as String?,
    provider: json['provider'] as String?,
    labels: ((json['labels'] as Map<String, dynamic>?) ?? const {}).map(
      (key, value) => MapEntry(key, value as String),
    ),
    capabilities: json['capabilities'] == null
        ? null
        : TheaterCapabilities.fromJson(
            json['capabilities'] as Map<String, dynamic>,
          ),
    agents: json['agents'] as String? ?? 'auto',
    egress: json['egress'] as String? ?? 'direct',
    harnesses: ((json['harnesses'] as List<dynamic>?) ?? const [])
        .cast<String>(),
    unavailable: json['unavailable'] as String?,
    weakened: ((json['weakened'] as List<dynamic>?) ?? const []).cast<String>(),
  );

  String get description {
    final via = provider ?? host;
    final where = via == null ? access : '$access $via';
    final tags = labels.entries.map((e) => '${e.key}=${e.value}').join(', ');
    return [
      tags.isEmpty ? where : '$where · $tags',
      if (unavailable != null) 'unreachable: $unavailable',
      if (capabilities != null) capabilities!.description,
      ...?capabilities?.notes,
      'agents $agents · egress $egress',
      if (harnesses.isNotEmpty) 'harnesses ${harnesses.join(', ')}',
      ...weakened.map((setting) => 'weakened: $setting'),
    ].join('\n');
  }
}

class PlacementChoice {
  const PlacementChoice({
    required this.station,
    required this.theater,
    required this.agent,
    this.mode = 'field',
  });

  final String station;
  final String theater;
  final String agent;

  /// `field` runs the harness on the Theater; `remote-hands` runs it on the Station and
  /// acts on the Theater through the transport.
  final String mode;

  factory PlacementChoice.fromJson(Map<String, dynamic> json) =>
      PlacementChoice(
        station: json['station'] as String,
        theater: json['theater'] as String,
        agent: json['agent'] as String,
        mode: json['mode'] as String? ?? 'field',
      );

  String get description =>
      '$station \u00b7 $theater \u00b7 $agent'
      '${mode == 'field' ? '' : ' \u00b7 $mode'}';
}

class PlacementRejection {
  const PlacementRejection({
    required this.reason,
    this.station,
    this.theater,
    this.agent,
  });

  final String? station;
  final String? theater;
  final String? agent;
  final String reason;

  factory PlacementRejection.fromJson(Map<String, dynamic> json) =>
      PlacementRejection(
        station: json['station'] as String?,
        theater: json['theater'] as String?,
        agent: json['agent'] as String?,
        reason: json['reason'] as String,
      );

  /// What was ruled out: the Station and Theater it names, then the Agent.
  String get subject => [
    station,
    theater,
    if (agent != null) 'Agent $agent',
  ].whereType<String>().join(' / ');
}

/// Where a Mission for an Operation would run and why each other place would not.
class PlacementPreview {
  const PlacementPreview({
    required this.operationId,
    required this.policyStation,
    required this.choices,
    required this.rejections,
  });

  final String operationId;
  final String policyStation;
  final List<PlacementChoice> choices;
  final List<PlacementRejection> rejections;

  factory PlacementPreview.fromJson(Map<String, dynamic> json) =>
      PlacementPreview(
        operationId: json['operation_id'] as String,
        policyStation: json['policy_station'] as String,
        choices: (json['choices'] as List<dynamic>)
            .map((c) => PlacementChoice.fromJson(c as Map<String, dynamic>))
            .toList(growable: false),
        rejections: (json['rejections'] as List<dynamic>)
            .map((r) => PlacementRejection.fromJson(r as Map<String, dynamic>))
            .toList(growable: false),
      );
}

class UserSessionSummary {
  const UserSessionSummary({
    required this.personId,
    required this.name,
    required this.displayName,
    required this.groups,
    required this.expiresAt,
    required this.operations,
    this.stations = const [],
  });

  final String personId;
  final String name;
  final String? displayName;
  final List<String> groups;
  final double expiresAt;
  final List<UserOperationAccess> operations;
  final List<UserStationAccess> stations;

  factory UserSessionSummary.fromJson(Map<String, dynamic> json) =>
      UserSessionSummary(
        personId: json['person_id'] as String,
        name: json['name'] as String,
        displayName: json['display_name'] as String?,
        groups: (json['groups'] as List<dynamic>).cast<String>(),
        expiresAt: (json['expires_at'] as num).toDouble(),
        operations: (json['operations'] as List<dynamic>)
            .map(
              (operation) => UserOperationAccess.fromJson(
                operation as Map<String, dynamic>,
              ),
            )
            .toList(growable: false),
        stations: (json['stations'] as List<dynamic>? ?? const [])
            .map(
              (station) =>
                  UserStationAccess.fromJson(station as Map<String, dynamic>),
            )
            .toList(growable: false),
      );
}

class UserOperationAccess {
  const UserOperationAccess({required this.operationId, required this.actions});

  final String operationId;
  final List<String> actions;

  factory UserOperationAccess.fromJson(Map<String, dynamic> json) =>
      UserOperationAccess(
        operationId: json['operation_id'] as String,
        actions: (json['actions'] as List<dynamic>).cast<String>(),
      );
}

class UserStationAccess {
  const UserStationAccess({required this.stationId, required this.actions});

  final String stationId;
  final List<String> actions;

  factory UserStationAccess.fromJson(Map<String, dynamic> json) =>
      UserStationAccess(
        stationId: json['station_id'] as String,
        actions: (json['actions'] as List<dynamic>).cast<String>(),
      );
}

class StationSettingsSummary {
  const StationSettingsSummary({
    required this.stationId,
    required this.maxConcurrentMissions,
    required this.configured,
    required this.executionEnabled,
  });

  final String stationId;
  final int maxConcurrentMissions;
  final bool configured;
  final bool executionEnabled;

  factory StationSettingsSummary.fromJson(Map<String, dynamic> json) =>
      StationSettingsSummary(
        stationId: json['station_id'] as String,
        maxConcurrentMissions: json['max_concurrent_missions'] as int,
        configured: json['configured'] as bool,
        executionEnabled: json['execution_enabled'] as bool,
      );
}

class MissionProposalSummary {
  const MissionProposalSummary({
    required this.id,
    required this.operationId,
    required this.parentMissionId,
    required this.proposerAgentId,
    required this.proposerSessionId,
    required this.title,
    required this.body,
    required this.role,
    required this.tags,
    required this.priority,
    required this.createdAt,
  });

  final String id;
  final String operationId;
  final String parentMissionId;
  final String proposerAgentId;
  final String proposerSessionId;
  final String title;
  final String body;
  final String? role;
  final List<String> tags;
  final int priority;
  final double createdAt;

  factory MissionProposalSummary.fromJson(Map<String, dynamic> json) {
    final brief = json['brief'] as Map<String, dynamic>;
    return MissionProposalSummary(
      id: json['id'] as String,
      operationId: json['operation_id'] as String,
      parentMissionId: json['parent_mission_id'] as String,
      proposerAgentId: json['proposer_agent_id'] as String,
      proposerSessionId: json['proposer_session_id'] as String,
      title: brief['title'] as String,
      body: brief['body'] as String? ?? '',
      role: brief['role'] as String?,
      tags: (brief['tags'] as List<dynamic>? ?? const []).cast<String>(),
      priority: brief['priority'] as int? ?? 0,
      createdAt: (json['created_at'] as num).toDouble(),
    );
  }
}

class AgentRoleSummary {
  const AgentRoleSummary({
    required this.id,
    required this.purpose,
    required this.policy,
    this.operationOverride = false,
  });

  final String id;
  final String purpose;
  final String policy;
  final bool operationOverride;

  factory AgentRoleSummary.fromJson(Map<String, dynamic> json) =>
      AgentRoleSummary(
        id: json['id'] as String,
        purpose: json['purpose'] as String,
        policy: json['policy'] as String,
        operationOverride: json['operation_override'] as bool? ?? false,
      );

  Map<String, dynamic> toJson() => {
    'id': id,
    'purpose': purpose,
    'policy': policy,
  };
}

class _DesktopWindowHost extends StatefulWidget {
  const _DesktopWindowHost({required this.board});

  final MissionBoard board;

  @override
  State<_DesktopWindowHost> createState() => _DesktopWindowHostState();
}

class _DesktopWindowHostState extends State<_DesktopWindowHost> {
  late final WindowController _mainWindow;

  @override
  void initState() {
    super.initState();
    _mainWindow = WindowController(
      size: const Size(1280, 820),
      title: 'Moneypenny | AI6',
      delegate: _MainWindowDelegate(),
    );
  }

  @override
  void dispose() {
    _mainWindow.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => WindowManager(
    initialWindows: [
      WindowEntry(
        controller: _mainWindow,
        builder: (_) =>
            MoneypennyApp(board: widget.board, desktopWindows: true),
      ),
    ],
  );
}

class _MainWindowDelegate with WindowControllerDelegate {
  @override
  void onWindowDestroyed() {
    super.onWindowDestroyed();
    ServicesBinding.instance.exitApplication(AppExitType.required);
  }
}

@immutable
class SessionSummary {
  const SessionSummary({
    required this.operationId,
    required this.missionId,
    required this.sessionId,
    required this.state,
    required this.sequence,
    this.reason,
    this.updatedAt,
    this.exposure = const [],
  });

  final String operationId;
  final String missionId;
  final String sessionId;
  final String? state;

  /// Why AI6 holds the Session in its state: `green-ink` while a step waits for a person's
  /// approval, `budget` after a spending limit.
  final String? reason;
  final int sequence;

  /// When the gateway last received this Session's state, as Unix seconds; the most
  /// recently updated Session of a Mission carries the Mission's current state.
  final double? updatedAt;

  /// The untrusted input the Session has read (`brief:source`, `tool:WebFetch`, ...).
  final List<String> exposure;

  /// A new state replaces the reason with [reason]; without one the reason stays.
  SessionSummary copyWith({String? state, int? sequence, String? reason}) =>
      SessionSummary(
        operationId: operationId,
        missionId: missionId,
        sessionId: sessionId,
        state: state ?? this.state,
        reason: state == null ? this.reason : reason,
        sequence: sequence ?? this.sequence,
        updatedAt: state == null
            ? updatedAt
            : DateTime.now().millisecondsSinceEpoch / 1000,
        exposure: exposure,
      );

  factory SessionSummary.fromJson(Map<String, dynamic> json) => SessionSummary(
    operationId: json['operation_id'] as String,
    missionId: json['mission_id'] as String,
    sessionId: json['session_id'] as String,
    state: json['state'] as String?,
    reason: json['reason'] as String?,
    sequence: json['sequence'] as int,
    updatedAt: (json['updated_at'] as num?)?.toDouble(),
    exposure: [
      for (final source in (json['exposure'] as List<dynamic>? ?? const []))
        source as String,
    ],
  );
}

class MissionBriefSummary {
  const MissionBriefSummary({
    required this.operationId,
    required this.missionId,
    required this.prompt,
    required this.agentId,
    required this.roleId,
    required this.handlerProvider,
    required this.terminalProvider,
    required this.workingDirectory,
    required this.model,
    required this.effort,
    required this.environmentKeys,
    this.stationId,
    this.theaterId,
    this.mode,
  });

  final String operationId;
  final String missionId;
  final String prompt;
  final String? agentId;
  final String? roleId;
  final String handlerProvider;
  final String terminalProvider;
  final String workingDirectory;
  final String? model;
  final String? effort;
  final List<String> environmentKeys;
  final String? stationId;
  final String? theaterId;

  /// `field` when the harness runs on the Theater, `remote-hands` when it runs on the
  /// Station and acts on the Theater through its transport.
  final String? mode;

  static MissionBriefSummary? fromMissionJson(Map<String, dynamic> json) {
    final brief = json['brief'];
    if (brief is! Map<String, dynamic>) return null;
    return MissionBriefSummary(
      operationId: json['operation_id'] as String,
      missionId: json['mission_id'] as String,
      prompt: brief['prompt'] as String,
      agentId: brief['agent_id'] as String?,
      roleId: brief['role_id'] as String?,
      handlerProvider: brief['handler_provider'] as String,
      terminalProvider: brief['terminal_provider'] as String,
      workingDirectory: brief['working_directory'] as String,
      model: brief['model'] as String?,
      effort: brief['effort'] as String?,
      environmentKeys: (brief['environment_keys'] as List<dynamic>)
          .cast<String>(),
      stationId: brief['station_id'] as String?,
      theaterId: brief['theater_id'] as String?,
      mode: brief['mode'] as String?,
    );
  }
}

/// One attempt at a Mission: where it ran and how it stands.
class MissionAttemptSummary {
  const MissionAttemptSummary({
    required this.attempt,
    required this.station,
    required this.status,
    this.theater,
    this.lost,
    this.checkpointBranch,
  });

  final int attempt;
  final String station;
  final String? theater;

  /// `active`, `parked`, `ended` or `lost`.
  final String status;

  /// Why the attempt was lost, when it was.
  final String? lost;

  /// The branch its work was last checkpointed to, when it was.
  final String? checkpointBranch;

  factory MissionAttemptSummary.fromJson(Map<String, dynamic> json) =>
      MissionAttemptSummary(
        attempt: json['attempt'] as int,
        station: json['station'] as String,
        status: json['status'] as String,
        theater: json['theater'] as String?,
        lost: json['lost'] as String?,
        checkpointBranch: json['checkpoint_branch'] as String?,
      );

  String describe() => [
    'Attempt $attempt',
    status,
    'Station $station',
    if (theater case final theater?) 'Theater $theater',
  ].join(' · ');
}

class MoneypennyApp extends StatelessWidget {
  const MoneypennyApp({
    required this.board,
    required this.desktopWindows,
    super.key,
  });

  final MissionBoard board;
  final bool desktopWindows;

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: board,
    builder: (context, _) => MaterialApp(
      title: 'Moneypenny | AI6',
      debugShowCheckedModeBanner: false,
      theme: _moneypennyLightTheme(),
      darkTheme: _moneypennyTheme(),
      themeMode: board.lightTheme ? ThemeMode.light : ThemeMode.dark,
      home: _AccessGate(board: board, desktopWindows: desktopWindows),
    ),
  );
}

class _AccessGate extends StatelessWidget {
  const _AccessGate({required this.board, required this.desktopWindows});

  final MissionBoard board;
  final bool desktopWindows;

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: board,
    builder: (context, _) {
      if (!board.demo && board.accessToken == null) {
        return _LoginPage(board: board);
      }
      return MissionBoardPage(board: board, desktopWindows: desktopWindows);
    },
  );
}

class _LoginPage extends StatefulWidget {
  const _LoginPage({required this.board});

  final MissionBoard board;

  @override
  State<_LoginPage> createState() => _LoginPageState();
}

class _LoginPageState extends State<_LoginPage> {
  final _name = TextEditingController();
  final _secret = TextEditingController();
  bool _submitting = false;

  @override
  void dispose() {
    _name.dispose();
    _secret.dispose();
    super.dispose();
  }

  Future<void> _signIn() async {
    if (_submitting ||
        (!kIsWeb && (_name.text.trim().isEmpty || _secret.text.isEmpty))) {
      return;
    }
    setState(() => _submitting = true);
    if (kIsWeb) {
      await widget.board.loginViaProxy();
    } else {
      await widget.board.login(_name.text.trim(), _secret.text);
    }
    _secret.clear();
    if (mounted) setState(() => _submitting = false);
  }

  bool get _canSubmit =>
      !_submitting &&
      (kIsWeb || (_name.text.trim().isNotEmpty && _secret.text.isNotEmpty));

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(
      title: const Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.radar, color: Color(0xff61d6bd)),
          SizedBox(width: 12),
          Text('Moneypenny'),
        ],
      ),
    ),
    body: Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 420),
        child: Card(
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  'Sign in',
                  style: Theme.of(context).textTheme.headlineSmall,
                ),
                const SizedBox(height: 20),
                if (kIsWeb) ...[
                  const Text(
                    'Continue with the account authenticated by your organization.',
                  ),
                ] else ...[
                  TextField(
                    controller: _name,
                    textInputAction: TextInputAction.next,
                    onChanged: (_) => setState(() {}),
                    decoration: const InputDecoration(labelText: 'Account'),
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: _secret,
                    obscureText: true,
                    onChanged: (_) => setState(() {}),
                    onSubmitted: (_) => _signIn(),
                    decoration: const InputDecoration(labelText: 'Password'),
                  ),
                ],
                if (widget.board.loginError != null) ...[
                  const SizedBox(height: 12),
                  Text(
                    widget.board.loginError!,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                ],
                const SizedBox(height: 20),
                FilledButton(
                  onPressed: _canSubmit ? _signIn : null,
                  child: _submitting
                      ? const SizedBox.square(
                          dimension: 18,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : Text(
                          kIsWeb
                              ? 'Continue with organization sign-in'
                              : 'Continue',
                        ),
                ),
              ],
            ),
          ),
        ),
      ),
    ),
  );
}

ThemeData _moneypennyTheme() => ThemeData(
  useMaterial3: true,
  colorScheme: ColorScheme.fromSeed(
    seedColor: const Color(0xff61d6bd),
    brightness: Brightness.dark,
  ),
  scaffoldBackgroundColor: const Color(0xff0a1114),
  cardTheme: const CardThemeData(
    color: Color(0xff111c20),
    margin: EdgeInsets.zero,
  ),
);

ThemeData _moneypennyLightTheme() => ThemeData.light(useMaterial3: true)
    .copyWith(
      colorScheme: ColorScheme.fromSeed(
        seedColor: const Color(0xff168c79),
        brightness: Brightness.light,
      ),
      scaffoldBackgroundColor: const Color(0xfff2f7f6),
      cardTheme: const CardThemeData(
        color: Colors.white,
        margin: EdgeInsets.zero,
      ),
    );

class MissionBoardPage extends StatefulWidget {
  const MissionBoardPage({
    required this.board,
    required this.desktopWindows,
    super.key,
  });

  final MissionBoard board;
  final bool desktopWindows;

  @override
  State<MissionBoardPage> createState() => _MissionBoardPageState();
}

class _MissionBoardPageState extends State<MissionBoardPage> {
  String? _selectedState;
  SessionSummary? _selectedWebSession;
  _ControlSection _section = _ControlSection.operations;

  List<_ControlSection> _sections(MissionBoard board) => [
    _ControlSection.stations,
    _ControlSection.operations,
    if (board.demo ||
        board.userSession?.operations.any(
              (operation) => operation.actions.contains('agent.read'),
            ) ==
            true ||
        board.userSession?.stations.any(
              (station) => station.actions.contains('station.read'),
            ) ==
            true ||
        board.greenInkOperations.isNotEmpty ||
        board.classAuthorizationOperations.isNotEmpty)
      _ControlSection.administration,
    if (board.timelineOperations.isNotEmpty) _ControlSection.timeline,
    if (board.demo || board.canReadAudit) _ControlSection.audit,
  ];

  String _sectionTitle(_ControlSection section) => switch (section) {
    _ControlSection.stations => 'Stations',
    _ControlSection.operations => 'Operations',
    _ControlSection.administration => 'Administration',
    _ControlSection.timeline => 'Timeline',
    _ControlSection.audit => 'Audit log',
  };

  Widget _sectionBody(
    BuildContext context,
    _ControlSection section,
  ) => AnimatedBuilder(
    animation: widget.board,
    builder: (context, _) {
      if (section == _ControlSection.administration) {
        return AdministrationPage(board: widget.board);
      }
      if (section == _ControlSection.audit) {
        return AuditLogPage(board: widget.board);
      }
      if (section == _ControlSection.timeline) {
        return OperationTimelinePage(board: widget.board);
      }
      if (section == _ControlSection.stations) {
        return ListView(
          children: [
            _StationOverview(
              board: widget.board.stationBoard,
              onCreateMission: (operationId) =>
                  _showCreateMission(context, operationId, widget.board),
              canCreateMission: (operationId) =>
                  !widget.board.demo &&
                  widget.board.canCreateMission(operationId),
            ),
            if (widget.board.previewOperations.isNotEmpty)
              _PlacementPanel(board: widget.board),
            if (widget.board.demo) const _DemoBanner(),
            if (widget.board.error != null)
              _ErrorBanner(
                apiOrigin: widget.board.apiOrigin,
                onRetry: widget.board.refresh,
              ),
          ],
        );
      }

      final sessions = widget.board.sessions
          .where(
            (session) =>
                _selectedState == null ||
                (session.state ?? 'working') == _selectedState,
          )
          .toList();
      final missions = _groupMissions(sessions);
      final showSessionPane =
          !widget.desktopWindows && MediaQuery.sizeOf(context).width >= 1200;
      final boardView = CustomScrollView(
        slivers: [
          SliverToBoxAdapter(
            child: _BoardHeader(
              sessions: widget.board.sessions,
              selectedState: _selectedState,
              onSelectState: (state) => setState(() => _selectedState = state),
            ),
          ),
          if (widget.board.demo) const SliverToBoxAdapter(child: _DemoBanner()),
          if (widget.board.error != null)
            SliverToBoxAdapter(
              child: _ErrorBanner(
                apiOrigin: widget.board.apiOrigin,
                onRetry: widget.board.refresh,
              ),
            ),
          if (widget.board.loading && widget.board.sessions.isEmpty)
            const SliverFillRemaining(
              hasScrollBody: false,
              child: Center(child: CircularProgressIndicator()),
            )
          else if (missions.isEmpty && _selectedState != null)
            SliverFillRemaining(
              hasScrollBody: false,
              child: _EmptyFilter(state: _selectedState!),
            )
          else if (missions.isEmpty && widget.board.error == null)
            const SliverFillRemaining(
              hasScrollBody: false,
              child: _EmptyBoard(),
            )
          else if (missions.isEmpty && widget.board.error != null)
            const SliverFillRemaining(
              hasScrollBody: false,
              child: _GatewayUnavailable(),
            )
          else
            SliverPadding(
              padding: const EdgeInsets.fromLTRB(24, 8, 24, 36),
              sliver: SliverList.separated(
                itemCount: missions.length,
                separatorBuilder: (_, _) => const SizedBox(height: 12),
                itemBuilder: (context, index) => _MissionCard(
                  mission: missions[index],
                  board: widget.board,
                  desktopWindows: widget.desktopWindows,
                  onSelectSession: showSessionPane
                      ? (session) {
                          setState(() => _selectedWebSession = session);
                        }
                      : null,
                ),
              ),
            ),
        ],
      );
      final selectedSession = _selectedWebSession;
      if (!showSessionPane || selectedSession == null) {
        return boardView;
      }
      return Row(
        children: [
          Expanded(child: boardView),
          const VerticalDivider(width: 1),
          SizedBox(
            width: 440,
            child: _SessionDetailPane(
              key: ValueKey(selectedSession.sessionId),
              session: selectedSession,
              board: widget.board,
              onClose: () => setState(() => _selectedWebSession = null),
            ),
          ),
        ],
      );
    },
  );

  @override
  Widget build(BuildContext context) {
    final sections = _sections(widget.board);
    final activeSection = sections.contains(_section)
        ? _section
        : _ControlSection.operations;
    final selectedIndex = sections.indexOf(activeSection);
    final wide = MediaQuery.sizeOf(context).width >= 900;
    final operatorInitial = _operatorInitial(widget.board.userSession);
    return Scaffold(
      appBar: AppBar(
        title: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.radar, color: Color(0xff61d6bd)),
            const SizedBox(width: 12),
            Text(_sectionTitle(activeSection)),
          ],
        ),
        actions: [
          IconButton(
            tooltip: 'Operator account and preferences',
            onPressed: () => _showOperatorMenu(context, widget.board),
            icon: CircleAvatar(
              radius: 16,
              child: operatorInitial == null
                  ? const Icon(Icons.person_outline, size: 19)
                  : Text(operatorInitial),
            ),
          ),
          IconButton(
            tooltip: 'Refresh AI6',
            onPressed: widget.board.refresh,
            icon: const Icon(Icons.refresh),
          ),
          const SizedBox(width: 12),
        ],
      ),
      body: Row(
        children: [
          if (wide)
            NavigationRail(
              selectedIndex: selectedIndex,
              onDestinationSelected: (index) =>
                  setState(() => _section = sections[index]),
              labelType: NavigationRailLabelType.all,
              destinations: [
                for (final section in sections)
                  NavigationRailDestination(
                    icon: Icon(_sectionIcon(section)),
                    label: Text(_sectionTitle(section)),
                  ),
              ],
            ),
          Expanded(child: _sectionBody(context, activeSection)),
        ],
      ),
      bottomNavigationBar: wide
          ? null
          : NavigationBar(
              selectedIndex: selectedIndex,
              onDestinationSelected: (index) =>
                  setState(() => _section = sections[index]),
              destinations: [
                for (final section in sections)
                  NavigationDestination(
                    icon: Icon(_sectionIcon(section)),
                    label: _sectionTitle(section),
                  ),
              ],
            ),
    );
  }
}

enum _ControlSection { stations, operations, administration, timeline, audit }

IconData _sectionIcon(_ControlSection section) => switch (section) {
  _ControlSection.stations => Icons.hub_outlined,
  _ControlSection.operations => Icons.workspaces_outline,
  _ControlSection.administration => Icons.settings_suggest_outlined,
  _ControlSection.timeline => Icons.timeline,
  _ControlSection.audit => Icons.fact_check_outlined,
};

class AuditEntrySummary {
  const AuditEntrySummary({
    required this.id,
    required this.actor,
    required this.action,
    required this.operation,
    this.resource,
    required this.target,
    required this.at,
  });

  final String id;
  final String actor;
  final String action;
  final String operation;
  final String? resource;
  final String target;
  final double at;

  factory AuditEntrySummary.fromJson(Map<String, dynamic> json) =>
      AuditEntrySummary(
        id: json['id'] as String,
        actor: json['actor'] as String,
        action: json['action'] as String,
        operation: json['operation'] as String,
        resource: json['resource'] as String?,
        target: json['target'] as String,
        at: (json['at'] as num).toDouble(),
      );
}

class AdministrationPage extends StatelessWidget {
  const AdministrationPage({required this.board, super.key});

  final MissionBoard board;

  @override
  Widget build(BuildContext context) {
    final canReadRoles =
        board.demo ||
        (board.userSession?.operations.any(
              (operation) => operation.actions.contains('agent.read'),
            ) ??
            false);
    final canReadStationSettings =
        board.demo || board.readableStations.isNotEmpty;
    final canReviewProposals = board.proposalOperations.isNotEmpty;
    final canSign = board.greenInkOperations.isNotEmpty;
    final canManageClasses = board.classAuthorizationOperations.isNotEmpty;
    final pages = <Widget>[
      if (canReadRoles) AgentRolesPage(board: board),
      if (canReadStationSettings) StationSettingsPage(board: board),
      if (canReviewProposals) MissionProposalsPage(board: board),
      if (canSign) GreenInkPage(board: board),
      if (canManageClasses) ClassAuthorizationsPage(board: board),
    ];
    final labels = <String>[
      if (canReadRoles) 'Agent roles',
      if (canReadStationSettings) 'Station settings',
      if (canReviewProposals) 'Mission proposals',
      if (canSign) 'Green ink',
      if (canManageClasses) 'Class authorisations',
    ];
    if (pages.isEmpty) {
      return const Center(
        child: Text('No AI6 settings are available to this account.'),
      );
    }
    if (pages.length == 1) return pages.single;
    return DefaultTabController(
      length: pages.length,
      child: Column(
        children: [
          TabBar(tabs: [for (final label in labels) Tab(text: label)]),
          Expanded(child: TabBarView(children: pages)),
        ],
      ),
    );
  }
}

class MissionProposalsPage extends StatefulWidget {
  const MissionProposalsPage({required this.board, super.key});

  final MissionBoard board;

  @override
  State<MissionProposalsPage> createState() => _MissionProposalsPageState();
}

class _MissionProposalsPageState extends State<MissionProposalsPage> {
  String? _operationId;
  Future<List<MissionProposalSummary>>? _proposals;
  bool _deciding = false;

  List<String> get _operations => widget.board.proposalOperations;

  @override
  void initState() {
    super.initState();
    final operations = _operations;
    if (operations.isNotEmpty) {
      _operationId = operations.first;
      _proposals = widget.board.loadMissionProposals(_operationId!);
    }
  }

  void _selectOperation(String? operationId) {
    if (operationId == null) return;
    setState(() {
      _operationId = operationId;
      _proposals = widget.board.loadMissionProposals(operationId);
    });
  }

  Future<void> _decide(
    MissionProposalSummary proposal, {
    required bool approve,
  }) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(approve ? 'Approve Mission proposal?' : 'Reject proposal?'),
        content: Text(
          approve
              ? 'Queue “${proposal.title}” as a Mission in ${proposal.operationId}? M will assign it under the Operation policy.'
              : 'Reject “${proposal.title}” and keep it out of runnable intake?',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(approve ? 'Approve and queue' : 'Reject'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    setState(() => _deciding = true);
    try {
      await widget.board.decideMissionProposal(
        operationId: proposal.operationId,
        proposalId: proposal.id,
        approve: approve,
      );
      if (!mounted) return;
      setState(() {
        _proposals = widget.board.loadMissionProposals(proposal.operationId);
      });
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            approve ? 'Proposal queued as a Mission.' : 'Proposal rejected.',
          ),
        ),
      );
    } on Object catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.toString())));
      }
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = _operations;
    final operationId = _operationId;
    return Padding(
      padding: const EdgeInsets.all(24),
      child: operations.isEmpty
          ? const Center(
              child: Text('No Operations grant Mission creation access.'),
            )
          : Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 420),
                  child: DropdownButtonFormField<String>(
                    initialValue: operationId,
                    decoration: const InputDecoration(labelText: 'Operation'),
                    items: operations
                        .map(
                          (id) => DropdownMenuItem(value: id, child: Text(id)),
                        )
                        .toList(growable: false),
                    onChanged: _selectOperation,
                  ),
                ),
                const SizedBox(height: 16),
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Pending proposals require an explicit M authorization.',
                        style: Theme.of(context).textTheme.titleMedium,
                      ),
                    ),
                    IconButton(
                      tooltip: 'Refresh proposals',
                      onPressed: operationId == null || _deciding
                          ? null
                          : () => setState(() {
                              _proposals = widget.board.loadMissionProposals(
                                operationId,
                              );
                            }),
                      icon: const Icon(Icons.refresh),
                    ),
                  ],
                ),
                if (widget.board.demo)
                  const Align(
                    alignment: Alignment.centerLeft,
                    child: Chip(
                      label: Text('Preview data — decisions disabled'),
                    ),
                  ),
                const SizedBox(height: 8),
                Expanded(
                  child: FutureBuilder<List<MissionProposalSummary>>(
                    future: _proposals,
                    builder: (context, snapshot) {
                      if (snapshot.hasError) {
                        return Center(child: Text(snapshot.error.toString()));
                      }
                      if (!snapshot.hasData) {
                        return const Center(child: CircularProgressIndicator());
                      }
                      final proposals = snapshot.data!;
                      if (proposals.isEmpty) {
                        return const Center(
                          child: Text('No pending Mission proposals.'),
                        );
                      }
                      return ListView.separated(
                        itemCount: proposals.length,
                        separatorBuilder: (context, index) =>
                            const SizedBox(height: 12),
                        itemBuilder: (context, index) {
                          final proposal = proposals[index];
                          return Card(
                            child: Padding(
                              padding: const EdgeInsets.all(16),
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Text(
                                    proposal.title,
                                    style: Theme.of(context)
                                        .textTheme
                                        .titleMedium,
                                  ),
                                  const SizedBox(height: 8),
                                  SelectableText(proposal.body),
                                  const SizedBox(height: 12),
                                  Text(
                                    'Proposed by ${proposal.proposerAgentId} · Parent Mission ${proposal.parentMissionId} · Session ${proposal.proposerSessionId}',
                                  ),
                                  if (proposal.role != null ||
                                      proposal.tags.isNotEmpty)
                                    Padding(
                                      padding: const EdgeInsets.only(top: 6),
                                      child: Text(
                                        [
                                          if (proposal.role != null)
                                            'Role: ${proposal.role}',
                                          if (proposal.tags.isNotEmpty)
                                            'Tags: ${proposal.tags.join(', ')}',
                                        ].join(' · '),
                                      ),
                                    ),
                                  const SizedBox(height: 12),
                                  Wrap(
                                    spacing: 8,
                                    children: [
                                      OutlinedButton.icon(
                                        onPressed:
                                            _deciding || widget.board.demo
                                            ? null
                                            : () => _decide(
                                                proposal,
                                                approve: false,
                                              ),
                                        icon: const Icon(Icons.close),
                                        label: const Text('Reject'),
                                      ),
                                      FilledButton.icon(
                                        onPressed:
                                            _deciding || widget.board.demo
                                            ? null
                                            : () => _decide(
                                                proposal,
                                                approve: true,
                                              ),
                                        icon: const Icon(
                                          Icons.playlist_add_check,
                                        ),
                                        label: const Text('Approve and queue'),
                                      ),
                                    ],
                                  ),
                                ],
                              ),
                            ),
                          );
                        },
                      );
                    },
                  ),
                ),
              ],
            ),
    );
  }
}

class StationSettingsPage extends StatefulWidget {
  const StationSettingsPage({required this.board, super.key});

  final MissionBoard board;

  @override
  State<StationSettingsPage> createState() => _StationSettingsPageState();
}

class _StationSettingsPageState extends State<StationSettingsPage> {
  final _capacity = TextEditingController();
  String? _stationId;
  Future<StationSettingsSummary>? _settings;
  bool _saving = false;

  List<String> get _stations => widget.board.demo
      ? const ['station-control-01']
      : widget.board.readableStations;

  @override
  void initState() {
    super.initState();
    final stations = _stations;
    if (stations.isNotEmpty) _selectStation(stations.first);
  }

  @override
  void dispose() {
    _capacity.dispose();
    super.dispose();
  }

  void _selectStation(String? stationId) {
    if (stationId == null) return;
    final settings = widget.board.loadStationSettings(stationId);
    setState(() {
      _stationId = stationId;
      _settings = settings;
    });
    unawaited(
      settings.then((value) {
        if (mounted && _stationId == stationId) {
          _capacity.text = '${value.maxConcurrentMissions}';
        }
      }),
    );
  }

  Future<void> _save() async {
    final stationId = _stationId;
    final capacity = int.tryParse(_capacity.text.trim());
    if (stationId == null ||
        capacity == null ||
        capacity < 1 ||
        capacity > 1024) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Mission capacity must be from 1 to 1,024.'),
        ),
      );
      return;
    }
    setState(() => _saving = true);
    try {
      await widget.board.saveStationSettings(stationId, capacity);
      if (!mounted) return;
      setState(() => _settings = widget.board.loadStationSettings(stationId));
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Station settings saved.')));
    } on Object catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.toString())));
      }
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final stations = _stations;
    if (stations.isEmpty) {
      return const Center(child: Text('No Stations grant settings access.'));
    }
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: DropdownButtonFormField<String>(
              initialValue: _stationId,
              decoration: const InputDecoration(labelText: 'Station'),
              items: [
                for (final station in stations)
                  DropdownMenuItem(value: station, child: Text(station)),
              ],
              onChanged: _selectStation,
            ),
          ),
          const SizedBox(height: 20),
          Expanded(
            child: FutureBuilder<StationSettingsSummary>(
              future: _settings,
              builder: (context, snapshot) {
                if (snapshot.hasError) {
                  return const Center(
                    child: Text('Station settings are unavailable.'),
                  );
                }
                if (!snapshot.hasData) {
                  return const Center(child: CircularProgressIndicator());
                }
                final settings = snapshot.requireData;
                final canConfigure =
                    !widget.board.demo &&
                    settings.executionEnabled &&
                    widget.board.canConfigureStation(settings.stationId);
                return ListView(
                  children: [
                    Text(
                      'Execution',
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    const SizedBox(height: 6),
                    Text(
                      !settings.executionEnabled
                          ? 'This Station does not have the execution role, so it cannot run Missions.'
                          : settings.configured
                          ? 'Configured through Moneypenny. Changes apply to new Missions.'
                          : 'Current Station value. Save to establish an authorized live setting.',
                    ),
                    const SizedBox(height: 16),
                    ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 320),
                      child: TextField(
                        controller: _capacity,
                        enabled: canConfigure && !_saving,
                        keyboardType: TextInputType.number,
                        decoration: const InputDecoration(
                          labelText: 'Maximum concurrent Missions',
                          helperText: 'Allowed range: 1–1,024',
                        ),
                      ),
                    ),
                    const SizedBox(height: 12),
                    if (canConfigure)
                      Align(
                        alignment: Alignment.centerLeft,
                        child: FilledButton.icon(
                          onPressed: _saving ? null : _save,
                          icon: const Icon(Icons.save_outlined),
                          label: Text(_saving ? 'Saving…' : 'Save settings'),
                        ),
                      ),
                    if (widget.board.demo) const _DemoBanner(),
                  ],
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

class AgentRolesPage extends StatefulWidget {
  const AgentRolesPage({required this.board, super.key});

  final MissionBoard board;

  @override
  State<AgentRolesPage> createState() => _AgentRolesPageState();
}

class _AgentRolesPageState extends State<AgentRolesPage> {
  String? _operationId;
  Future<List<AgentRoleSummary>>? _roles;

  List<String> get _operations =>
      widget.board.userSession?.operations
          .where(
            (operation) =>
                widget.board.canReadAgentRoles(operation.operationId),
          )
          .map((operation) => operation.operationId)
          .toList(growable: false) ??
      const [];

  @override
  void initState() {
    super.initState();
    final operations = _operations;
    if (operations.isNotEmpty) {
      _operationId = operations.first;
      _roles = widget.board.loadAgentRoles(_operationId!);
    }
  }

  void _selectOperation(String? operationId) {
    if (operationId == null) return;
    setState(() {
      _operationId = operationId;
      _roles = widget.board.loadAgentRoles(operationId);
    });
  }

  Future<void> _editRole([AgentRoleSummary? role]) async {
    final operationId = _operationId;
    if (operationId == null ||
        !widget.board.canConfigureAgentRoles(operationId)) {
      return;
    }
    final edited = await showDialog<AgentRoleSummary>(
      context: context,
      builder: (_) => AgentRoleEditorDialog(initial: role),
    );
    if (edited == null || !mounted) return;
    try {
      await widget.board.saveAgentRole(operationId, edited);
      setState(() => _roles = widget.board.loadAgentRoles(operationId));
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(const SnackBar(content: Text('Agent role saved.')));
      }
    } on Object catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.toString())));
      }
    }
  }

  Future<void> _removeRole(String roleId) async {
    final operationId = _operationId;
    if (operationId == null ||
        !widget.board.canConfigureAgentRoles(operationId)) {
      return;
    }
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove $roleId override?'),
        content: const Text(
          'Missions in this Operation will use the Station role definition again.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Remove override'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    try {
      await widget.board.deleteAgentRole(operationId, roleId);
      setState(() => _roles = widget.board.loadAgentRoles(operationId));
    } on Object catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error.toString())));
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final operations = _operations;
    final operationId = _operationId;
    final canConfigure =
        operationId != null &&
        widget.board.canConfigureAgentRoles(operationId) &&
        !widget.board.demo;
    return Padding(
      padding: const EdgeInsets.all(24),
      child: operations.isEmpty
          ? const Center(child: Text('No Operations grant Agent role access.'))
          : Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 420),
                  child: DropdownButtonFormField<String>(
                    initialValue: operationId,
                    decoration: const InputDecoration(labelText: 'Operation'),
                    items: operations
                        .map(
                          (id) => DropdownMenuItem(value: id, child: Text(id)),
                        )
                        .toList(growable: false),
                    onChanged: _selectOperation,
                  ),
                ),
                const SizedBox(height: 20),
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Role purpose and policy',
                        style: Theme.of(context).textTheme.titleMedium,
                      ),
                    ),
                    if (canConfigure)
                      FilledButton.icon(
                        onPressed: () => _editRole(),
                        icon: const Icon(Icons.add),
                        label: const Text('Add role'),
                      ),
                  ],
                ),
                const SizedBox(height: 12),
                Expanded(
                  child: FutureBuilder<List<AgentRoleSummary>>(
                    future: _roles,
                    builder: (context, snapshot) {
                      if (snapshot.hasError) {
                        return Center(child: Text(snapshot.error.toString()));
                      }
                      if (!snapshot.hasData) {
                        return const Center(child: CircularProgressIndicator());
                      }
                      if (snapshot.data!.isEmpty) {
                        return const Center(
                          child: Text('No Agent roles are configured.'),
                        );
                      }
                      return ListView.separated(
                        itemCount: snapshot.data!.length,
                        separatorBuilder: (_, _) => const SizedBox(height: 8),
                        itemBuilder: (context, index) {
                          final role = snapshot.data![index];
                          return Card(
                            child: ListTile(
                              title: Text(role.id),
                              subtitle: Text('${role.purpose}\n${role.policy}'),
                              isThreeLine: true,
                              trailing: canConfigure
                                  ? Wrap(
                                      children: [
                                        IconButton(
                                          tooltip: 'Edit ${role.id}',
                                          onPressed: () => _editRole(role),
                                          icon: const Icon(Icons.edit_outlined),
                                        ),
                                        if (role.operationOverride)
                                          IconButton(
                                            tooltip:
                                                'Remove ${role.id} override',
                                            onPressed: () =>
                                                _removeRole(role.id),
                                            icon: const Icon(
                                              Icons.delete_outline,
                                            ),
                                          ),
                                      ],
                                    )
                                  : null,
                            ),
                          );
                        },
                      );
                    },
                  ),
                ),
              ],
            ),
    );
  }
}

class AgentRoleEditorDialog extends StatefulWidget {
  const AgentRoleEditorDialog({required this.initial, super.key});

  final AgentRoleSummary? initial;

  @override
  State<AgentRoleEditorDialog> createState() => _AgentRoleEditorDialogState();
}

class _AgentRoleEditorDialogState extends State<AgentRoleEditorDialog> {
  final _formKey = GlobalKey<FormState>();
  late final TextEditingController _id;
  late final TextEditingController _purpose;
  late final TextEditingController _policy;

  @override
  void initState() {
    super.initState();
    final initial = widget.initial;
    _id = TextEditingController(text: initial?.id ?? '');
    _purpose = TextEditingController(text: initial?.purpose ?? '');
    _policy = TextEditingController(text: initial?.policy ?? '');
  }

  @override
  void dispose() {
    _id.dispose();
    _purpose.dispose();
    _policy.dispose();
    super.dispose();
  }

  String? _required(String? value) =>
      value == null || value.trim().isEmpty ? 'Required' : null;

  void _save() {
    if (!_formKey.currentState!.validate()) return;
    Navigator.of(context).pop(
      AgentRoleSummary(
        id: _id.text.trim(),
        purpose: _purpose.text.trim(),
        policy: _policy.text.trim(),
      ),
    );
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(
      widget.initial == null ? 'Add Agent role' : 'Edit ${widget.initial!.id}',
    ),
    content: SizedBox(
      width: 560,
      child: Form(
        key: _formKey,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextFormField(
                controller: _id,
                enabled: widget.initial == null,
                validator: _required,
                decoration: const InputDecoration(labelText: 'Role ID'),
              ),
              TextFormField(
                controller: _purpose,
                validator: _required,
                decoration: const InputDecoration(labelText: 'Purpose'),
              ),
              TextFormField(
                controller: _policy,
                validator: _required,
                minLines: 2,
                maxLines: 5,
                decoration: const InputDecoration(
                  labelText: 'Policy instructions',
                ),
              ),
            ],
          ),
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(onPressed: _save, child: const Text('Save')),
    ],
  );
}

class AuditLogPage extends StatefulWidget {
  const AuditLogPage({required this.board, super.key});

  final MissionBoard board;

  @override
  State<AuditLogPage> createState() => _AuditLogPageState();
}

class _AuditLogPageState extends State<AuditLogPage> {
  late Future<List<AuditEntrySummary>> _records;

  @override
  void initState() {
    super.initState();
    _records = widget.board.loadAudit();
  }

  void _refresh() => setState(() => _records = widget.board.loadAudit());

  @override
  Widget build(BuildContext context) => Column(
    children: [
      Align(
        alignment: Alignment.centerRight,
        child: IconButton(
          tooltip: 'Refresh audit log',
          onPressed: _refresh,
          icon: const Icon(Icons.refresh),
        ),
      ),
      Expanded(
        child: FutureBuilder<List<AuditEntrySummary>>(
          future: _records,
          builder: (context, snapshot) {
            if (snapshot.hasError) {
              return const Center(
                child: Text('Audit records are unavailable.'),
              );
            }
            if (!snapshot.hasData) {
              return const Center(child: CircularProgressIndicator());
            }
            final records = snapshot.requireData;
            if (records.isEmpty) {
              return const Center(
                child: Text('No audit records are available.'),
              );
            }
            return ListView.separated(
              padding: const EdgeInsets.all(20),
              itemCount: records.length,
              separatorBuilder: (_, _) => const Divider(height: 1),
              itemBuilder: (context, index) {
                final record = records[index];
                final at = DateTime.fromMillisecondsSinceEpoch(
                  (record.at * 1000).round(),
                  isUtc: true,
                ).toLocal();
                return ListTile(
                  leading: const Icon(Icons.receipt_long_outlined),
                  title: Text('${record.actor} · ${record.action}'),
                  subtitle: Text(
                    '${record.resource ?? 'Operation ${record.operation}'} · ${record.target}\n$at',
                  ),
                  isThreeLine: true,
                );
              },
            );
          },
        ),
      ),
    ],
  );
}

String? _operatorInitial(UserSessionSummary? session) {
  final displayName = session?.displayName?.trim();
  final name = displayName?.isNotEmpty == true
      ? displayName
      : session?.name.trim();
  if (name == null || name.isEmpty) return null;
  return String.fromCharCode(name.runes.first).toUpperCase();
}

Future<void> _showOperatorMenu(BuildContext context, MissionBoard board) async {
  final identity = board.demo ? null : board.loadUserSession();
  await showDialog<void>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: const Text('Operator account'),
      content: SizedBox(
        width: 440,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxHeight: 440),
          child: SingleChildScrollView(
            child: AnimatedBuilder(
              animation: board,
              builder: (context, _) => Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (board.demo)
                    const Text('Preview mode · no signed-in operator')
                  else
                    FutureBuilder<UserSessionSummary>(
                      future: identity,
                      builder: (context, snapshot) {
                        if (snapshot.hasError) {
                          return const Text('Account details are unavailable.');
                        }
                        if (!snapshot.hasData) {
                          return const LinearProgressIndicator();
                        }
                        final account = snapshot.requireData;
                        final expiresAt = DateTime.fromMillisecondsSinceEpoch(
                          (account.expiresAt * 1000).round(),
                          isUtc: true,
                        );
                        return Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              account.displayName?.isNotEmpty == true
                                  ? account.displayName!
                                  : account.name,
                            ),
                            Text('Person ID: ${account.personId}'),
                            const SizedBox(height: 6),
                            Text('Access expires: ${expiresAt.toLocal()}'),
                            if (account.groups.isNotEmpty) ...[
                              const SizedBox(height: 10),
                              const Text('Directory groups'),
                              Wrap(
                                spacing: 6,
                                children: [
                                  for (final group in account.groups)
                                    Chip(label: Text(group)),
                                ],
                              ),
                            ],
                            const SizedBox(height: 10),
                            const Text('Effective actions by Operation'),
                            if (account.operations.isEmpty)
                              const Text('No AI6 actions are granted.')
                            else
                              for (final operation in account.operations)
                                Padding(
                                  padding: const EdgeInsets.only(top: 8),
                                  child: Text(
                                    '${operation.operationId}: ${operation.actions.join(', ')}',
                                  ),
                                ),
                          ],
                        );
                      },
                    ),
                  const SizedBox(height: 18),
                  SwitchListTile.adaptive(
                    contentPadding: EdgeInsets.zero,
                    title: const Text('Light appearance'),
                    value: board.lightTheme,
                    onChanged: (_) => board.toggleTheme(),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
      actions: [
        if (!board.demo)
          TextButton.icon(
            onPressed: () {
              Navigator.pop(dialogContext);
              unawaited(board.logout());
            },
            icon: const Icon(Icons.logout),
            label: const Text('Sign out'),
          ),
        TextButton(
          onPressed: () => Navigator.pop(dialogContext),
          child: const Text('Close'),
        ),
      ],
    ),
  );
}

class _StationOverview extends StatelessWidget {
  const _StationOverview({
    required this.board,
    required this.onCreateMission,
    required this.canCreateMission,
  });

  final StationBoardSummary board;
  final ValueChanged<String> onCreateMission;
  final bool Function(String operationId) canCreateMission;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          'STATIONS',
          style: Theme.of(context).textTheme.labelMedium
              ?.copyWith(letterSpacing: 2.4, color: const Color(0xff86a39d)),
        ),
        const SizedBox(height: 10),
        if (board.stations.isEmpty)
          const Card(
            child: Padding(
              padding: EdgeInsets.all(16),
              child: Text('No Stations have checked in yet.'),
            ),
          )
        else
          Wrap(
            spacing: 12,
            runSpacing: 12,
            children: [
              for (final station in board.stations)
                SizedBox(
                  width: 300,
                  child: _StationCard(
                    station: station,
                    onCreateMission: onCreateMission,
                    canCreateMission: canCreateMission,
                  ),
                ),
            ],
          ),
      ],
    ),
  );
}

/// Asks the gateway where a Mission for an Operation would run and shows the
/// reason for every Station, Theater or Agent it would not.
class _PlacementPanel extends StatefulWidget {
  const _PlacementPanel({required this.board});

  final MissionBoard board;

  @override
  State<_PlacementPanel> createState() => _PlacementPanelState();
}

class _PlacementPanelState extends State<_PlacementPanel> {
  String? _operationId;
  Future<PlacementPreview>? _preview;

  @override
  Widget build(BuildContext context) {
    final operations = widget.board.previewOperations;
    final selected = operations.contains(_operationId)
        ? _operationId
        : operations.first;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 8),
      child: Card(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'PLACEMENT PREVIEW',
                style: Theme.of(context).textTheme.labelMedium?.copyWith(
                  letterSpacing: 2.4,
                  color: const Color(0xff86a39d),
                ),
              ),
              const SizedBox(height: 10),
              Wrap(
                spacing: 12,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  DropdownButton<String>(
                    value: selected,
                    items: [
                      for (final operation in operations)
                        DropdownMenuItem(
                          value: operation,
                          child: Text(operation),
                        ),
                    ],
                    onChanged: (value) => setState(() {
                      _operationId = value;
                      _preview = null;
                    }),
                  ),
                  FilledButton.tonalIcon(
                    onPressed: selected == null
                        ? null
                        : () => setState(() {
                            _operationId = selected;
                            _preview = widget.board.loadPlacementPreview(
                              selected,
                            );
                          }),
                    icon: const Icon(Icons.alt_route),
                    label: const Text('Preview placement'),
                  ),
                ],
              ),
              if (_preview != null)
                FutureBuilder<PlacementPreview>(
                  future: _preview,
                  builder: (context, snapshot) {
                    if (snapshot.hasError) {
                      return const Padding(
                        padding: EdgeInsets.only(top: 10),
                        child: Text('The gateway could not preview placement.'),
                      );
                    }
                    final preview = snapshot.data;
                    if (preview == null) {
                      return const Padding(
                        padding: EdgeInsets.only(top: 10),
                        child: LinearProgressIndicator(),
                      );
                    }
                    return _PlacementResult(preview: preview);
                  },
                ),
            ],
          ),
        ),
      ),
    );
  }
}

class _PlacementResult extends StatelessWidget {
  const _PlacementResult({required this.preview});

  final PlacementPreview preview;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(top: 10),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          preview.choices.isEmpty
              ? 'No place can run this Operation now.'
              : 'Would run on, best first',
          style: Theme.of(context).textTheme.titleSmall,
        ),
        for (final choice in preview.choices) Text(choice.description),
        if (preview.rejections.isNotEmpty) ...[
          const SizedBox(height: 8),
          Text('Ruled out', style: Theme.of(context).textTheme.titleSmall),
          for (final rejection in preview.rejections)
            Text('${rejection.subject}: ${rejection.reason}'),
        ],
        const SizedBox(height: 6),
        Text(
          'Policy from ${preview.policyStation}',
          style: Theme.of(context).textTheme.bodySmall,
        ),
      ],
    ),
  );
}

Future<void> _showCreateMission(
  BuildContext context,
  String operationId,
  MissionBoard board,
) async {
  final title = TextEditingController();
  final prompt = TextEditingController();
  var submitting = false;
  try {
    final result = await showDialog<String>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: Text('New Mission · $operationId'),
          content: SizedBox(
            width: 520,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  controller: title,
                  enabled: !submitting,
                  maxLength: 512,
                  onChanged: (_) => setState(() {}),
                  decoration: const InputDecoration(labelText: 'Mission title'),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: prompt,
                  enabled: !submitting,
                  minLines: 5,
                  maxLines: 10,
                  maxLength: 65536,
                  onChanged: (_) => setState(() {}),
                  decoration: const InputDecoration(
                    labelText: 'Instructions for the agent',
                    alignLabelWithHint: true,
                  ),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: submitting ? null : () => Navigator.pop(dialogContext),
              child: const Text('Cancel'),
            ),
            FilledButton.icon(
              onPressed:
                  submitting ||
                      title.text.trim().isEmpty ||
                      prompt.text.trim().isEmpty
                  ? null
                  : () async {
                      setState(() => submitting = true);
                      try {
                        final missionId = await board.createMission(
                          operationId: operationId,
                          title: title.text.trim(),
                          prompt: prompt.text.trim(),
                        );
                        if (dialogContext.mounted) {
                          Navigator.pop(dialogContext, missionId);
                        }
                      } on Object {
                        if (dialogContext.mounted) {
                          setState(() => submitting = false);
                          ScaffoldMessenger.of(dialogContext).showSnackBar(
                            const SnackBar(
                              content: Text(
                                'Could not queue the Mission. Check access and try again.',
                              ),
                            ),
                          );
                        }
                      }
                    },
              icon: submitting
                  ? const SizedBox.square(
                      dimension: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.send),
              label: const Text('Queue Mission'),
            ),
          ],
        ),
      ),
    );
    if (result != null && context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('Mission $result queued for Operation $operationId.'),
        ),
      );
    }
  } on Object {
    if (context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text(
            'Could not queue the Mission. Check access and try again.',
          ),
        ),
      );
    }
  } finally {
    title.dispose();
    prompt.dispose();
  }
}

class _StationCard extends StatelessWidget {
  const _StationCard({
    required this.station,
    required this.onCreateMission,
    required this.canCreateMission,
  });

  final StationSummary station;
  final ValueChanged<String> onCreateMission;
  final bool Function(String operationId) canCreateMission;

  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                station.healthy ? Icons.circle : Icons.circle_outlined,
                size: 11,
                color: station.healthy
                    ? const Color(0xff61d6bd)
                    : const Color(0xffe3a466),
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Text(
                  station.stationId,
                  style: Theme.of(context).textTheme.titleMedium,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              if (station.headquarters)
                const Chip(
                  avatar: Icon(Icons.account_balance, size: 15),
                  label: Text('HQS'),
                  visualDensity: VisualDensity.compact,
                ),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            '${station.healthy ? 'Healthy' : 'Not reporting'} · '
            '${station.running}/${station.capacity} sessions',
            style: Theme.of(context).textTheme.bodySmall,
          ),
          if (station.theaters.isNotEmpty) ...[
            const SizedBox(height: 10),
            Wrap(
              spacing: 6,
              runSpacing: 4,
              children: [
                for (final theater in station.theaters)
                  Tooltip(
                    message: theater.description,
                    child: Chip(
                      avatar: Icon(
                        theater.unavailable != null
                            ? Icons.link_off
                            : theater.access == 'local'
                            ? Icons.computer
                            : Icons.cloud_outlined,
                        size: 15,
                      ),
                      label: Text('Theater ${theater.id}'),
                      visualDensity: VisualDensity.compact,
                    ),
                  ),
              ],
            ),
          ],
          if (station.operations.isNotEmpty) ...[
            const SizedBox(height: 10),
            Wrap(
              spacing: 6,
              runSpacing: 4,
              children: [
                for (final operation in station.operations)
                  ActionChip(
                    label: Text(operation),
                    avatar: const Icon(Icons.add, size: 16),
                    tooltip: canCreateMission(operation)
                        ? 'Start a Mission in $operation'
                        : 'You do not have mission.create access to $operation',
                    onPressed: canCreateMission(operation)
                        ? () => onCreateMission(operation)
                        : null,
                    visualDensity: VisualDensity.compact,
                  ),
              ],
            ),
          ],
        ],
      ),
    ),
  );
}

class _BoardHeader extends StatelessWidget {
  const _BoardHeader({
    required this.sessions,
    required this.selectedState,
    required this.onSelectState,
  });

  final List<SessionSummary> sessions;
  final String? selectedState;
  final ValueChanged<String?> onSelectState;

  @override
  Widget build(BuildContext context) {
    final counts = <String, int>{};
    for (final session in sessions) {
      final state = session.state ?? 'working';
      counts.update(state, (value) => value + 1, ifAbsent: () => 1);
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'MISSIONS',
            style: Theme.of(context).textTheme.labelMedium
                ?.copyWith(letterSpacing: 2.4, color: const Color(0xff86a39d)),
          ),
          const SizedBox(height: 4),
          Text(
            'Mission board',
            style: Theme.of(context).textTheme.headlineMedium
                ?.copyWith(fontWeight: FontWeight.w600),
          ),
          const SizedBox(height: 16),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _CountChip(
                label: 'Sessions',
                count: sessions.length,
                color: const Color(0xffb5c4c5),
                selected: selectedState == null,
                onPressed: () => onSelectState(null),
              ),
              for (final state in const [
                'queued',
                'working',
                'blocked',
                'stalled',
                'parked',
                'quarantined',
                'done',
                'failed',
                'abandoned',
              ])
                _CountChip(
                  label: state,
                  count: counts[state] ?? 0,
                  color: _stateColor(state),
                  selected: selectedState == state,
                  onPressed: () =>
                      onSelectState(selectedState == state ? null : state),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

class _CountChip extends StatelessWidget {
  const _CountChip({
    required this.label,
    required this.count,
    required this.color,
    required this.selected,
    required this.onPressed,
  });

  final String label;
  final int count;
  final Color color;
  final bool selected;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) => ChoiceChip(
    avatar: CircleAvatar(backgroundColor: color, radius: 5),
    label: Text('$label  $count'),
    selected: selected,
    onSelected: (_) => onPressed(),
    visualDensity: VisualDensity.compact,
    tooltip: label == 'Sessions' ? 'Show all Sessions' : 'Filter by $label',
  );
}

class _EmptyFilter extends StatelessWidget {
  const _EmptyFilter({required this.state});

  final String state;

  @override
  Widget build(BuildContext context) =>
      Center(child: Text('No ${state.toUpperCase()} Sessions'));
}

class _MissionGroup {
  const _MissionGroup({
    required this.operationId,
    required this.missionId,
    required this.sessions,
  });

  final String operationId;
  final String missionId;
  final List<SessionSummary> sessions;
}

List<_MissionGroup> _groupMissions(List<SessionSummary> sessions) {
  final groups = <(String, String), List<SessionSummary>>{};
  for (final session in sessions) {
    groups
        .putIfAbsent((session.operationId, session.missionId), () => [])
        .add(session);
  }
  final result = groups.entries
      .map(
        (entry) => _MissionGroup(
          operationId: entry.key.$1,
          missionId: entry.key.$2,
          sessions: List.unmodifiable(entry.value),
        ),
      )
      .toList();
  result.sort((left, right) {
    final station = left.operationId.compareTo(right.operationId);
    return station != 0 ? station : left.missionId.compareTo(right.missionId);
  });
  return result;
}

class _MissionCard extends StatelessWidget {
  const _MissionCard({
    required this.mission,
    required this.board,
    required this.desktopWindows,
    required this.onSelectSession,
  });

  final _MissionGroup mission;
  final MissionBoard board;
  final bool desktopWindows;
  final ValueChanged<SessionSummary>? onSelectSession;

  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.folder_open, color: Color(0xff61d6bd), size: 19),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  mission.operationId,
                  style: Theme.of(context).textTheme.titleMedium,
                ),
              ),
              Text(
                '${mission.sessions.length} session${mission.sessions.length == 1 ? '' : 's'}',
                style: Theme.of(context).textTheme.labelMedium,
              ),
              if (board.missionBrief(mission.operationId, mission.missionId)
                  case final brief?)
                IconButton(
                  tooltip: 'View Mission brief and launch context',
                  onPressed: () => Navigator.of(context).push(
                    MaterialPageRoute<void>(
                      builder: (_) => _MissionBriefPage(brief: brief),
                    ),
                  ),
                  icon: const Icon(Icons.description_outlined),
                ),
              MissionActionsMenu(
                board: board,
                operationId: mission.operationId,
                missionId: mission.missionId,
                sessions: mission.sessions,
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.only(left: 27, top: 4, bottom: 12),
            child: Text(
              'Mission ${mission.missionId}',
              style: Theme.of(context).textTheme.bodySmall
                  ?.copyWith(color: const Color(0xff9aafb0)),
            ),
          ),
          if (board.attempts(mission.operationId, mission.missionId)
              case final attempts
              when attempts.length > 1 ||
                  attempts.any((attempt) => attempt.status == 'lost'))
            _AttemptHistory(attempts: attempts),
          if (board.controls[(mission.operationId, mission.missionId)]
              case final control?)
            Padding(
              padding: const EdgeInsets.only(left: 27, bottom: 12),
              child: Text(
                control.describe(),
                style: Theme.of(context).textTheme.labelMedium,
              ),
            ),
          for (final session in mission.sessions)
            _SessionRow(
              session: session,
              board: board,
              desktopWindows: desktopWindows,
              onSelectSession: onSelectSession,
            ),
        ],
      ),
    ),
  );
}

/// Every attempt at a Mission, first first, with why a lost one was lost.
class _AttemptHistory extends StatelessWidget {
  const _AttemptHistory({required this.attempts});

  final List<MissionAttemptSummary> attempts;

  @override
  Widget build(BuildContext context) {
    final muted = Theme.of(context).textTheme.bodySmall
        ?.copyWith(color: const Color(0xff9aafb0));
    return Padding(
      padding: const EdgeInsets.only(left: 27, bottom: 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (final attempt in attempts) ...[
            Text(
              attempt.describe(),
              style: Theme.of(context).textTheme.labelMedium,
            ),
            if (attempt.lost case final lost?) Text(lost, style: muted),
            if (attempt.checkpointBranch case final branch?)
              Text('Checkpointed to branch $branch', style: muted),
          ],
        ],
      ),
    );
  }
}

class _MissionBriefPage extends StatelessWidget {
  const _MissionBriefPage({required this.brief});

  final MissionBriefSummary brief;

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: Text('Mission ${brief.missionId}')),
    body: ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Text(brief.operationId, style: Theme.of(context).textTheme.titleMedium),
        const SizedBox(height: 20),
        Text('Brief', style: Theme.of(context).textTheme.titleLarge),
        const SizedBox(height: 8),
        SelectableText(brief.prompt),
        const SizedBox(height: 24),
        Text('Launch context', style: Theme.of(context).textTheme.titleLarge),
        const SizedBox(height: 8),
        _manifestRow('Agent', brief.agentId ?? 'Unassigned'),
        _manifestRow('Role', brief.roleId ?? 'Unspecified'),
        _manifestRow('Station', brief.stationId ?? 'Not recorded'),
        _manifestRow('Theater', brief.theaterId ?? 'Not recorded'),
        if (brief.mode != null) _manifestRow('Mode', brief.mode!),
        _manifestRow('Handler', brief.handlerProvider),
        _manifestRow('Terminal', brief.terminalProvider),
        _manifestRow('Working directory', brief.workingDirectory),
        _manifestRow('Model', brief.model ?? 'Configured by handler'),
        _manifestRow('Effort', brief.effort ?? 'Configured by handler'),
        _manifestRow(
          'Environment names',
          brief.environmentKeys.isEmpty
              ? 'None'
              : brief.environmentKeys.join(', '),
        ),
        const SizedBox(height: 8),
        Text(
          'Environment values are withheld.',
          style: Theme.of(context).textTheme.bodySmall,
        ),
      ],
    ),
  );

  Widget _manifestRow(String label, String value) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 5),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(width: 160, child: Text(label)),
        Expanded(child: SelectableText(value)),
      ],
    ),
  );
}

class _SessionRow extends StatelessWidget {
  const _SessionRow({
    required this.session,
    required this.board,
    required this.desktopWindows,
    required this.onSelectSession,
  });

  final SessionSummary session;
  final MissionBoard board;
  final bool desktopWindows;
  final ValueChanged<SessionSummary>? onSelectSession;

  void _open(BuildContext context) {
    if (onSelectSession case final select?) {
      select(session);
    } else if (desktopWindows) {
      _openSessionWindow(context, session, board);
    } else {
      _openSessionPage(context, session, board);
    }
  }

  @override
  Widget build(BuildContext context) {
    final state = session.state ?? 'working';
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: const Color(0xff0c1518),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: const Color(0xff26363a)),
      ),
      child: InkWell(
        onTap: () => _open(context),
        borderRadius: BorderRadius.circular(8),
        child: Row(
          children: [
            Icon(Icons.circle, size: 9, color: _stateColor(state)),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    session.sessionId,
                    style: const TextStyle(fontFamily: 'monospace'),
                  ),
                  Text(
                    'Updated at event ${session.sequence}',
                    style: Theme.of(context).textTheme.labelSmall
                        ?.copyWith(color: const Color(0xff84999b)),
                  ),
                ],
              ),
            ),
            if (session.exposure.isNotEmpty) ...[
              Tooltip(
                message: 'Read untrusted input: ${session.exposure.join(', ')}',
                child: const Chip(
                  label: Text('EXPOSED'),
                  visualDensity: VisualDensity.compact,
                ),
              ),
              const SizedBox(width: 6),
            ],
            Chip(
              label: Text(
                [
                  state,
                  ?session.reason?.replaceAll('-', ' '),
                ].join(' · ').toUpperCase(),
              ),
              visualDensity: VisualDensity.compact,
            ),
            if (desktopWindows) ...[
              const SizedBox(width: 8),
              IconButton(
                tooltip: 'Open Session window',
                onPressed: () => _open(context),
                icon: const Icon(Icons.open_in_new),
              ),
            ] else ...[
              const SizedBox(width: 8),
              IconButton(
                tooltip: 'View Session details',
                onPressed: () => _open(context),
                icon: const Icon(Icons.chevron_right),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

void _openSessionPage(
  BuildContext context,
  SessionSummary session,
  MissionBoard board,
) {
  Navigator.of(context).push(
    MaterialPageRoute<void>(
      builder: (_) => SessionWindow(session: session, board: board),
    ),
  );
}

void _openSessionWindow(
  BuildContext context,
  SessionSummary session,
  MissionBoard board,
) {
  final registry = WindowRegistry.of(context);
  late final WindowEntry entry;
  final controller = WindowController(
    size: const Size(900, 680),
    title: 'Session ${session.sessionId} | Moneypenny',
    delegate: _WindowDelegate(onDestroyed: () => registry.unregister(entry)),
  );
  entry = WindowEntry(
    controller: controller,
    builder: (_) => AnimatedBuilder(
      animation: board,
      builder: (context, _) => MaterialApp(
        title: 'Session ${session.sessionId}',
        theme: _moneypennyLightTheme(),
        darkTheme: _moneypennyTheme(),
        themeMode: board.lightTheme ? ThemeMode.light : ThemeMode.dark,
        home: SessionWindow(session: session, board: board),
      ),
    ),
  );
  registry.register(entry);
}

class _WindowDelegate with WindowControllerDelegate {
  _WindowDelegate({required this.onDestroyed});

  final VoidCallback onDestroyed;

  @override
  void onWindowDestroyed() {
    onDestroyed();
    super.onWindowDestroyed();
  }
}

class SessionWindow extends StatelessWidget {
  const SessionWindow({required this.session, required this.board, super.key});

  final SessionSummary session;
  final MissionBoard board;

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: Text('Session ${session.sessionId}')),
    body: _SessionDetailContent(session: session, board: board),
  );
}

class _SessionDetailPane extends StatelessWidget {
  const _SessionDetailPane({
    required this.session,
    required this.board,
    required this.onClose,
    super.key,
  });

  final SessionSummary session;
  final MissionBoard board;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) => Column(
    children: [
      ListTile(
        title: Text(
          'Session ${session.sessionId}',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        trailing: IconButton(
          tooltip: 'Close Session details',
          onPressed: onClose,
          icon: const Icon(Icons.close),
        ),
      ),
      const Divider(height: 1),
      Expanded(
        child: _SessionDetailContent(session: session, board: board),
      ),
    ],
  );
}

class _SessionDetailContent extends StatelessWidget {
  const _SessionDetailContent({required this.session, required this.board});

  final SessionSummary session;
  final MissionBoard board;

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: board,
    builder: (context, _) {
      final current =
          board.sessions
              .where((item) => item.sessionId == session.sessionId)
              .firstOrNull ??
          session;
      return Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              current.operationId,
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 4),
            Text(
              'Mission ${current.missionId} · ${current.state ?? 'awaiting lifecycle'}',
            ),
            const SizedBox(height: 24),
            Expanded(
              child: board.demo
                  ? const _SessionPreviewPanel(
                      message: 'Demo Sessions are sample data. No terminal connection is available.',
                    )
                  : board.accessToken == null
                  ? const _SessionPreviewPanel(
                      message: 'Sign in to Universal Exports to attach to this Session.',
                    )
                  : _LiveSessionTerminal(session: current, board: board),
            ),
          ],
        ),
      );
    },
  );
}

class _SessionPreviewPanel extends StatelessWidget {
  const _SessionPreviewPanel({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) => Card(
    child: Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.terminal, size: 36, color: Color(0xff61d6bd)),
            const SizedBox(height: 12),
            const Text('Session preview'),
            const SizedBox(height: 8),
            Text(message, textAlign: TextAlign.center),
          ],
        ),
      ),
    ),
  );
}

class _LiveSessionTerminal extends StatefulWidget {
  const _LiveSessionTerminal({required this.session, required this.board});

  final SessionSummary session;
  final MissionBoard board;

  @override
  State<_LiveSessionTerminal> createState() => _LiveSessionTerminalState();
}

class _LiveSessionTerminalState extends State<_LiveSessionTerminal> {
  late final SessionConnection _connection;

  @override
  void initState() {
    super.initState();
    _connection = SessionConnection(
      board: widget.board,
      session: widget.session,
    )..addListener(_changed);
    _connection.start();
  }

  @override
  void didUpdateWidget(_LiveSessionTerminal oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.session.sequence != widget.session.sequence) {
      _connection.advanceCursor(widget.session.sequence);
    }
  }

  @override
  void dispose() {
    _connection.removeListener(_changed);
    _connection.dispose();
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  @override
  Widget build(BuildContext context) => Card(
    clipBehavior: Clip.antiAlias,
    child: Column(
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
          child: Row(
            children: [
              Expanded(child: Text(_connection.status)),
              TextButton.icon(
                onPressed: _connection.isConnected
                    ? () => _connection.setWriteLease(
                        !_connection.writeLeaseRequested,
                      )
                    : null,
                icon: Icon(
                  _connection.writeLeaseRequested
                      ? Icons.lock_open
                      : Icons.lock_outline,
                ),
                label: Text(
                  _connection.writeLeaseRequested
                      ? 'Release input lease'
                      : 'Request input lease',
                ),
              ),
            ],
          ),
        ),
        const Divider(height: 1),
        Expanded(
          child: _connection.runtimeError == null
              ? SessionTerminal(
                  output: _connection.output,
                  onInput: _connection.sendInput,
                  onResize: _connection.sendResize,
                  writeLeaseHeld: _connection.writeLeaseHeld,
                )
              : Center(
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: Text(_connection.runtimeError!),
                  ),
                ),
        ),
      ],
    ),
  );
}

class _EmptyBoard extends StatelessWidget {
  const _EmptyBoard();

  @override
  Widget build(BuildContext context) => const Center(
    child: Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(Icons.radar, size: 42, color: Color(0xff64847d)),
        SizedBox(height: 12),
        Text('No active Missions'),
        SizedBox(height: 4),
        Text('Registered agent Sessions will appear here.'),
      ],
    ),
  );
}

class _DemoBanner extends StatelessWidget {
  const _DemoBanner();

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 0, 24, 12),
    child: DecoratedBox(
      decoration: BoxDecoration(
        color: const Color(0xff102521),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: const Color(0xff28574d)),
      ),
      child: const Padding(
        padding: EdgeInsets.all(12),
        child: Row(
          children: [
            Icon(Icons.visibility, color: Color(0xff61d6bd), size: 18),
            SizedBox(width: 10),
            Expanded(
              child: Text(
                'Demo preview · sign-in is unavailable. Open the authenticated Moneypenny site.',
              ),
            ),
          ],
        ),
      ),
    ),
  );
}

class _GatewayUnavailable extends StatelessWidget {
  const _GatewayUnavailable();

  @override
  Widget build(BuildContext context) => const Center(
    child: Padding(
      padding: EdgeInsets.all(32),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.cloud_off, size: 42, color: Color(0xff84999b)),
          SizedBox(height: 12),
          Text('Universal Exports is offline'),
          SizedBox(height: 4),
          Text(
            'Start the gateway, or launch with MONEYPENNY_DEMO=true to preview.',
          ),
        ],
      ),
    ),
  );
}

class _ErrorBanner extends StatelessWidget {
  const _ErrorBanner({required this.apiOrigin, required this.onRetry});

  final Uri apiOrigin;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
    child: MaterialBanner(
      content: Text(
        'Could not reach Moneypenny at ${apiOrigin.host}:${apiOrigin.port}.',
      ),
      leading: const Icon(Icons.cloud_off),
      actions: [TextButton(onPressed: onRetry, child: const Text('Retry'))],
    ),
  );
}

Color _stateColor(String state) => switch (state) {
  'working' => const Color(0xff61d6bd),
  'blocked' => const Color(0xffffc16b),
  'stalled' => const Color(0xffff7777),
  'parked' => const Color(0xff8fb7ff),
  'quarantined' => const Color(0xffd98cff),
  'queued' => const Color(0xffb5c4c5),
  'done' => const Color(0xff9da8b5),
  'failed' => const Color(0xffff4e66),
  'abandoned' => const Color(0xff6f7f86),
  _ => const Color(0xff9da8b5),
};
