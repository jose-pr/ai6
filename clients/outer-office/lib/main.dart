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

import 'agents/agents_page.dart';
import 'assistant/assistant_controller.dart';
import 'assistant/chat_layout.dart';
import 'assistant/chat_region.dart';
import 'assistant/chat_window.dart';
import 'assistant/demo_conversation.dart';
import 'assistant/memory_page.dart';
import 'assistant/navigation_card.dart';
import 'controls/mission_actions.dart';
import 'green_ink/green_ink_page.dart';
import 'held/held_briefs_page.dart';
import 'pools/pools_page.dart';
import 'roster/roster_page.dart';
import 'routes/oo_filters.dart';
import 'routes/oo_route.dart';
import 'routes/oo_router.dart';
import 'settings/connection_settings.dart';
import 'settings/oo_config.dart';
import 'settings/oo_config_file.dart';
import 'shell/app_window.dart';
import 'shell/oo_shell.dart';
import 'operations/operation_tree.dart';
import 'shell/operations_page.dart';
import 'shell/missions_board.dart';
import 'shell/missions_page.dart';
import 'shell/oo_freshness.dart';
import 'shell/overview_page.dart';
import 'shell/sessions_page.dart';
import 'shell/station_watch.dart';
import 'shell/top_bar.dart';
import 'theme/oo_appearance.dart';
import 'theme/oo_backdrop.dart';
import 'theme/oo_icon.dart';
import 'theme/oo_theme.dart';
import 'timeline/timeline_page.dart';

const _apiOverride = String.fromEnvironment('OO_API');
const _demoMode = bool.fromEnvironment('OO_DEMO');

/// Universal Exports' origin: the `OO_API` define, else on the web the page's own origin, else
/// `OO.cfg`'s `[unex] url`, else the loopback default.
Uri _apiOrigin(OoConfig config) => _apiOverride.isNotEmpty
    ? Uri.parse(_apiOverride)
    : kIsWeb
    ? Uri.base
    : config.unexUrl ?? Uri.parse('http://127.0.0.1:7082');

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  LicenseRegistry.addLicense(() => fontLicenses(web: kIsWeb));
  if (kIsWeb) {
    unawaited(_startWeb());
    return;
  }
  _startDesktop();
}

/// The licences of the fonts the app ships, for its licence page: Roboto and Cinzel
/// everywhere, and on the [web] the Noto fallback fonts its build serves.
Stream<LicenseEntry> fontLicenses({required bool web}) async* {
  yield LicenseEntryWithLineBreaks(const [
    'Roboto',
  ], await rootBundle.loadString('assets/fonts/LICENSE-Roboto.txt'));
  yield LicenseEntryWithLineBreaks(const [
    'Cinzel',
  ], await rootBundle.loadString('assets/fonts/LICENSE-Cinzel.txt'));
  if (web) {
    yield LicenseEntryWithLineBreaks(const [
      'Noto fonts',
    ], await rootBundle.loadString('assets/fonts/LICENSE-Noto.txt'));
  }
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
  final config = await loadWebConfig(Uri.base);
  OoRouteLimits.current = config.routeLimits;
  final appearance = AppearancePreferences(SharedPreferences.getInstance());
  final board = MissionBoard(
    apiOrigin: _apiOrigin(config),
    demo: _demoMode,
    themeId: config.defaultTheme(warn: debugPrint),
    terminalRuntimeError: terminalRuntimeError,
    onThemeChanged: appearance.saveTheme,
    onBackdropChanged: appearance.saveBackdrop,
    pollInterval: config.limits.refresh.sessions,
    limits: config.limits,
  );
  try {
    await appearance.restore(board);
  } on Object {
    // Browser storage unavailable: the defaults stand.
  }

  runApp(
    OuterOfficeApp(
      board: board,
      desktopWindows: false,
      chatSettings: config.chat,
    ),
  );
}

void _startDesktop() {
  final config = loadDesktopConfig();
  OoRouteLimits.current = config.routeLimits;
  final appearance = AppearancePreferences(SharedPreferences.getInstance());
  final board = MissionBoard(
    apiOrigin: _apiOrigin(config),
    demo: _demoMode,
    themeId: config.defaultTheme(warn: debugPrint),
    onThemeChanged: appearance.saveTheme,
    onBackdropChanged: appearance.saveBackdrop,
    pollInterval: config.limits.refresh.sessions,
    limits: config.limits,
  );
  runWidget(_DesktopWindowHost(board: board, chatSettings: config.chat));
  unawaited(appearance.restore(board).catchError((Object _) {}));
}

/// The person's appearance choices on this device: `oo.theme` holds a theme id or
/// `system`, `oo.backdrop` whether the wallpaper is painted.
class AppearancePreferences {
  AppearancePreferences(this._preferences);

  final Future<SharedPreferences> _preferences;

  static const themeKey = 'oo.theme';
  static const backdropKey = 'oo.backdrop';

  Future<void> saveTheme(String themeId) async {
    await (await _preferences).setString(themeKey, themeId);
  }

  Future<void> saveBackdrop(bool backdrop) async {
    await (await _preferences).setBool(backdropKey, backdrop);
  }

  /// Applies the stored choices to [board]: a stored theme wins over the one it started
  /// with (`OO.cfg`'s default), and an unknown one is Night Ops.
  Future<void> restore(MissionBoard board) async {
    final preferences = await _preferences;
    final theme = preferences.getString(themeKey);
    if (theme != null) board.restoreThemeId(theme);
    board.restoreBackdrop(preferences.getBool(backdropKey));
  }
}

class MissionBoard extends ChangeNotifier {
  MissionBoard({
    required this.apiOrigin,
    this.demo = false,
    this.terminalRuntimeError,
    String themeId = 'night-ops',
    this.backdrop = true,
    this.onThemeChanged,
    this.onBackdropChanged,
    this.assistantChannel,
    http.Client? client,
    Duration? pollInterval = RefreshSettings.defaultSessions,
    this.limits = const OoLimits(),
  }) : themeId = ooThemePreference(themeId),
       _client = client ?? http.Client() {
    if (demo) {
      sessions = const [
        SessionSummary(
          operationId: 'G',
          missionId: 'mission-007',
          sessionId: 'preview-session-007',
          state: 'working',
          sequence: 18,
          stationId: 'station-control-01',
          agent: 'technical-officer',
        ),
        SessionSummary(
          operationId: 'G',
          missionId: 'mission-007',
          sessionId: 'preview-session-008',
          state: 'blocked',
          sequence: 7,
          exposure: ['tool:WebFetch'],
          stationId: 'station-control-01',
          agent: 'technical-officer',
        ),
        SessionSummary(
          operationId: 'L',
          missionId: 'mission-021',
          sessionId: 'preview-session-021',
          state: 'done',
          sequence: 32,
          stationId: 'station-remote-02',
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
          origin: BriefOrigin(
            origin: 'operator',
            principal: 'demo-operator',
            receivedBy: 'universal-exports',
          ),
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
          target: 'oo--mission-007',
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
            resources: HostResources(
              sampledAt: 1_798_000_000,
              cpus: 8,
              cpuPercent: 20,
              load1m: 1.2,
              memoryTotalMb: 16384,
              memoryAvailableMb: 10240,
            ),
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
      heldBriefs = _demoHeldBriefs('G');
      missionProposals = _demoMissionProposals;
      loading = false;
      return;
    }
    unawaited(refresh());
    final every = pollInterval;
    if (every != null) {
      _poll = Timer.periodic(every, (_) => unawaited(refresh()));
    }
  }

  final Uri apiOrigin;
  final bool demo;

  /// `OO.cfg`'s refresh, control, page, terminal and input settings.
  final OoLimits limits;
  final String? terminalRuntimeError;
  final Future<void> Function(String themeId)? onThemeChanged;
  final Future<void> Function(bool backdrop)? onBackdropChanged;

  /// Opens the channel to Moneypenny; without one, her socket on [apiOrigin]. Tests give a
  /// fake one.
  final AssistantChannel Function()? assistantChannel;
  final http.Client _client;
  Timer? _poll;
  List<SessionSummary> sessions = const [];
  List<MissionBriefSummary> missionBriefs = const [];

  /// Every attempt at each Mission, by (Operation, Mission), first first.
  Map<(String, String), List<MissionAttemptSummary>> missionAttempts = const {};
  List<AuditEntrySummary> auditEntries = const [];

  /// The briefs vetting holds, in the Operations whose held briefs this account may review.
  List<HeldBriefSummary> heldBriefs = const [];

  /// The Mission proposals waiting for a decision, in the Operations where this account may
  /// decide them; read at most every [slowEvery], not with each refresh.
  List<MissionProposalSummary> missionProposals = const [];
  DateTime? _proposalsAt;

  /// How often the sources that change slowly are read again.
  static const slowEvery = Duration(seconds: 15);
  UserSessionSummary? userSession;
  StationBoardSummary stationBoard = const StationBoardSummary(
    headquartersStation: null,
    stations: [],
  );
  String? error;
  String? loginError;
  String? accessToken;
  bool loading = true;

  /// The person's theme: a theme id or `system` ([ooThemePreference]).
  String themeId;

  /// Whether the theme's wallpaper is painted where one may be.
  bool backdrop;

  /// Switches to a theme id or `system` and stores the choice.
  void setThemeId(String themeId) {
    restoreThemeId(themeId);
    final persist = onThemeChanged;
    if (persist != null) {
      unawaited(persist(this.themeId).catchError((Object _) {}));
    }
  }

  /// Applies a stored theme preference without storing it again; absent or unknown is
  /// Night Ops.
  void restoreThemeId(String? themeId) {
    final theme = ooThemePreference(themeId);
    if (theme == this.themeId) return;
    this.themeId = theme;
    notifyListeners();
  }

  /// Turns the wallpaper on or off and stores the choice.
  void setBackdrop(bool backdrop) {
    restoreBackdrop(backdrop);
    final persist = onBackdropChanged;
    if (persist != null) {
      unawaited(persist(backdrop).catchError((Object _) {}));
    }
  }

  /// Applies a stored wallpaper preference without storing it again; absent is on.
  void restoreBackdrop(bool? backdrop) {
    if ((backdrop ?? true) == this.backdrop) return;
    this.backdrop = backdrop ?? true;
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
            mode: 'field',
            score: 1.03,
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
    if (response.statusCode == 403) throw const PlacementForbidden();
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
    Duration? wait,
    Duration? pollEvery,
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
            'oo-${DateTime.now().microsecondsSinceEpoch}-${_controlSerial++}',
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
    final deadline = DateTime.now().add(wait ?? limits.controls.wait);
    while (status.isOpen && DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(pollEvery ?? limits.controls.poll);
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

  static const _demoMissionProposals = [
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
      vetting: [],
    ),
  ];

  Future<List<MissionProposalSummary>> loadMissionProposals(
    String operationId,
  ) async {
    if (demo) return _demoMissionProposals;
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

  /// The briefs vetting holds in an Operation, oldest first.
  Future<List<HeldBriefSummary>> loadHeldBriefs(String operationId) async {
    if (demo) return _demoHeldBriefs(operationId);
    final response = await _userRequest(
      'GET',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}/held-briefs',
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused the held briefs.');
    }
    return [
      for (final held in jsonDecode(response.body) as List<dynamic>)
        HeldBriefSummary.fromJson(held as Map<String, dynamic>),
    ];
  }

  /// The briefs vetting holds in every Operation whose held briefs this account may review
  /// (`mission.create`); an Operation the gateway refuses is left out.
  Future<List<HeldBriefSummary>> _loadAllHeldBriefs() async {
    final held = await Future.wait([
      for (final operationId in proposalOperations)
        loadHeldBriefs(operationId)
            .catchError((Object _) => const <HeldBriefSummary>[]),
    ]);
    return [for (final briefs in held) ...briefs];
  }

  /// The Mission proposals in every Operation where this account may decide them; an
  /// Operation the gateway refuses is left out.
  Future<List<MissionProposalSummary>> _loadAllProposals() async {
    final proposals = await Future.wait([
      for (final operationId in proposalOperations)
        loadMissionProposals(operationId)
            .catchError((Object _) => const <MissionProposalSummary>[]),
    ]);
    return [for (final list in proposals) ...list];
  }

  static List<HeldBriefSummary> _demoHeldBriefs(String operationId) => [
    HeldBriefSummary(
      key: 'source_github--org_repo_7',
      operationId: operationId,
      title: 'Resolve github item org/repo#7',
      body: 'Title: Crash on save\n\nThe app crashes when saving.',
      source: 'source:github',
      url: 'https://github.com/org/repo/issues/7',
      findings: const [
        VettingFinding(
          kind: 'duplicate',
          detail: 'the same URL as other work',
          related: 'mission-007',
        ),
      ],
      heldAt: 1_798_000_000,
    ),
  ];

  /// Releases a held brief to the queue, or refuses it with an optional reason.
  Future<void> decideHeldBrief({
    required String operationId,
    required String key,
    required bool release,
    String? reason,
  }) async {
    if (demo) {
      throw StateError('Held brief decisions are unavailable in preview.');
    }
    final response = await _userRequest(
      'POST',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}'
          '/held-briefs/${Uri.encodeComponent(key)}/'
          '${release ? 'release' : 'refuse'}',
      body: release ? null : {'reason': ?reason},
    );
    switch (response.statusCode) {
      case 200:
        await refresh();
      case 403:
        throw StateError(
          'Your account lacks mission.create in this Operation.',
        );
      case 404:
        throw StateError('The brief is no longer held.');
      default:
        throw StateError('The gateway refused the held brief decision.');
    }
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
    int? limit,
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
              'limit': '${limit ?? limits.pages.timeline}',
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

  /// Operations whose account pools this account may read (`pool.read`).
  List<String> get poolOperations => _operationsWith('pool.read');

  /// Whether the person may read the roster every Station reads.
  bool get canReadRoster =>
      demo || (userSession?.roster.contains('roster.read') ?? false);

  /// Whether the person may edit it.
  bool get canManageRoster =>
      !demo && (userSession?.roster.contains('roster.manage') ?? false);

  /// The roster records of one kind (`operations`, `agents`, `roles`, `covers` or
  /// `assistant`).
  Future<RosterRecords> loadRoster(String kind) async {
    if (demo) return RosterRecords.demo(kind);
    final response = await _userRequest(
      'GET',
      '/v1/user/roster/${Uri.encodeComponent(kind)}',
    );
    if (response.statusCode == 503) {
      throw StateError('Universal Exports serves no roster.');
    }
    if (response.statusCode != 200) {
      throw StateError('The gateway refused the roster.');
    }
    return RosterRecords.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  /// One Operation as its Stations resolve it: its definition with its ancestors' keys and
  /// the Operation that wrote each. Needs `roster.read`.
  Future<EffectiveOperationSummary> effectiveOperation(
    String operationId,
  ) async {
    if (demo) {
      throw StateError('The effective policy is unavailable in preview.');
    }
    final response = await _userRequest(
      'GET',
      '/v1/user/roster/operation/${Uri.encodeComponent(operationId)}/effective',
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused the effective policy.');
    }
    return EffectiveOperationSummary.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  /// Stores one record's definition; returns the roster revision it moved to and what the
  /// stored record weakens. Throws [RosterRefused] with the reason when the roster it would
  /// leave is not acceptable.
  Future<RosterChange> saveRosterRecord(
    String kind,
    String key,
    Map<String, dynamic> definition,
  ) async {
    if (demo) throw StateError('Roster changes are unavailable in preview.');
    return _rosterChange(
      await _userRequest(
        'PUT',
        '/v1/user/roster/${Uri.encodeComponent(kind)}/${Uri.encodeComponent(key)}',
        body: definition,
      ),
    );
  }

  /// Removes one record; returns the roster revision it moved to.
  Future<RosterChange> deleteRosterRecord(String kind, String key) async {
    if (demo) throw StateError('Roster changes are unavailable in preview.');
    return _rosterChange(
      await _userRequest(
        'DELETE',
        '/v1/user/roster/${Uri.encodeComponent(kind)}/${Uri.encodeComponent(key)}',
      ),
    );
  }

  RosterChange _rosterChange(http.Response response) {
    switch (response.statusCode) {
      case 200:
        return RosterChange.fromJson(
          jsonDecode(response.body) as Map<String, dynamic>,
        );
      case 422:
        throw RosterRefused(
          (jsonDecode(response.body) as Map<String, dynamic>)['refused']
              as String,
        );
      case 409:
        throw StateError('This roster is read-only; edit its files instead.');
      case 404:
        throw StateError('There is no such roster record.');
      default:
        throw StateError('The gateway refused the roster change.');
    }
  }

  /// The files of Moneypenny's memory of the signed-in person. Throws [MemoryRefused] with
  /// Universal Exports' words when it cannot be read.
  Future<MemoryListing> loadMemory() async {
    if (demo) return MemoryListing.demo();
    final response = await _userRequest('GET', '/v1/user/assistant/memory');
    if (response.statusCode != 200) throw _memoryRefused(response);
    return MemoryListing.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  /// One file of that memory, by its path under `memory/`.
  Future<MemoryText> readMemory(String path) async {
    if (demo) {
      return MemoryText(
        path: path,
        text: '# Memory\n\n- preferences.md: how the operator likes answers\n',
      );
    }
    final response = await _userRequest('GET', _memoryUrl(path));
    if (response.statusCode != 200) throw _memoryRefused(response);
    return MemoryText.fromJson(
      jsonDecode(response.body) as Map<String, dynamic>,
    );
  }

  /// Writes one file of that memory as a commit by the person. With [base], the digest of
  /// the text it was made from, a file changed since is refused rather than replaced.
  Future<void> saveMemory(String path, String text, {String? base}) async {
    if (demo) throw StateError('Memory changes are unavailable in preview.');
    final response = await _userRequest(
      'PUT',
      _memoryUrl(path),
      body: {'content': text, 'base': ?base},
    );
    if (response.statusCode != 200) throw _memoryRefused(response);
  }

  /// Forgets one file of that memory: a commit by the person that removes it.
  Future<void> forgetMemory(String path) async {
    if (demo) throw StateError('Memory changes are unavailable in preview.');
    final response = await _userRequest('DELETE', _memoryUrl(path));
    if (response.statusCode != 200) throw _memoryRefused(response);
  }

  String _memoryUrl(String path) =>
      '/v1/user/assistant/memory/${path.split('/').map(Uri.encodeComponent).join('/')}';

  /// Universal Exports' own words for a refused memory request, when it gave any.
  MemoryRefused _memoryRefused(http.Response response) {
    try {
      final error =
          (jsonDecode(response.body) as Map<String, dynamic>)['error'];
      if (error is String && error.isNotEmpty) return MemoryRefused(error);
    } on Object {
      // Not Universal Exports' JSON: fall back to the status.
    }
    return MemoryRefused(
      'Universal Exports refused it (HTTP ${response.statusCode}).',
    );
  }

  /// The pool accounts an Operation may draw from.
  Future<List<PoolAccountSummary>> loadPools(String operationId) async {
    if (demo) {
      return const [
        PoolAccountSummary(
          pool: 'claude-team',
          account: 'alpha',
          handler: 'claude',
          kind: 'claude-oauth-token',
          active: ['mission-007'],
          maxConcurrent: 2,
          windows: [
            QuotaWindowSummary(name: 'five_hour', usedPermille: 640),
            QuotaWindowSummary(name: 'seven_day', usedPermille: 210),
          ],
          tokensToday: 182_400,
        ),
        PoolAccountSummary(
          pool: 'claude-team',
          account: 'beta',
          handler: 'claude',
          kind: 'claude-oauth-token',
          active: [],
          windows: [
            QuotaWindowSummary(
              name: 'five_hour',
              usedPermille: 1000,
              status: 'rejected',
            ),
          ],
          limitedUntil: 1_798_003_600,
          tokensToday: 951_020,
        ),
      ];
    }
    final token = accessToken;
    if (token == null) {
      throw StateError('Sign in before viewing account pools.');
    }
    final response = await _client.get(
      apiOrigin.resolve(
        '/v1/user/operations/${Uri.encodeComponent(operationId)}/pools',
      ),
      headers: {'authorization': 'Bearer $token'},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused the account pools.');
    }
    final board = jsonDecode(response.body) as Map<String, dynamic>;
    return [
      for (final account in board['accounts'] as List<dynamic>)
        PoolAccountSummary.fromJson(account as Map<String, dynamic>),
    ];
  }

  /// Operations whose Agents and records this account may read (`agent.read`).
  List<String> get agentOperations => _operationsWith('agent.read');

  /// The Agents of an Operation and their records there.
  Future<List<AgentSummary>> loadAgents(String operationId) async {
    if (demo) {
      return const [
        AgentSummary(
          agent: '007',
          posted: true,
          operation: AgentTallySummary(done: 12, failed: 1, stalled: 1),
          model: 'sonnet',
          clearance: 'field',
          configuredClearance: 'desk',
          grant: ClearanceGrantSummary(
            clearance: 'field',
            signer: 'operator',
            approval: 'green-ink-preview',
          ),
          retirement: RetirementSummary(
            retireOn: '2027-01-31',
            onRetire: 'desk',
            status: 'retiring',
          ),
        ),
        AgentSummary(
          agent: '008',
          posted: true,
          operation: AgentTallySummary(done: 4),
          model: 'gpt-5.6',
          clearance: 'field',
          configuredClearance: 'field',
        ),
      ];
    }
    final response = await _userRequest(
      'GET',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}/agents',
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused the Operation\'s Agents.');
    }
    return [
      for (final agent in jsonDecode(response.body) as List<dynamic>)
        AgentSummary.fromJson(agent as Map<String, dynamic>),
    ];
  }

  /// Holds an Agent at `clearance` or below in an Operation (`agent.configure`).
  Future<void> lowerClearance(
    String operationId,
    String agent,
    String clearance,
    String reason,
  ) async {
    if (demo) throw StateError('Clearance changes are unavailable in preview.');
    final response = await _userRequest(
      'PUT',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}/agents/${Uri.encodeComponent(agent)}/clearance',
      body: {'clearance': clearance, 'reason': reason},
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused to lower the clearance.');
    }
  }

  /// Removes an Agent's clearance grant in an Operation (`agent.configure`).
  Future<void> revokeClearanceGrant(String operationId, String agent) async {
    if (demo) throw StateError('Clearance changes are unavailable in preview.');
    final response = await _userRequest(
      'DELETE',
      '/v1/user/operations/${Uri.encodeComponent(operationId)}/agents/${Uri.encodeComponent(agent)}/clearance',
    );
    if (response.statusCode != 200) {
      throw StateError('The gateway refused to remove the clearance grant.');
    }
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

  /// Sets a Station's Mission capacity and, when given, the free memory (MiB) and CPU
  /// percent past which it takes no new Missions; a limit left out keeps its value.
  Future<void> saveStationSettings(
    String stationId,
    int maxConcurrentMissions, {
    int? minFreeMemoryMb,
    int? maxCpuPercent,
  }) async {
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
      body: jsonEncode({
        'max_concurrent_missions': maxConcurrentMissions,
        'min_free_memory_mb': ?minFreeMemoryMb,
        'max_cpu_percent': ?maxCpuPercent,
      }),
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

  Future<List<AuditEntrySummary>> loadAudit({int? limit}) async {
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
          .replace(
            queryParameters: {'limit': '${limit ?? limits.pages.audit}'},
          ),
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
      heldBriefs = await _loadAllHeldBriefs();
      final now = ooClock();
      if (_proposalsAt == null || now.difference(_proposalsAt!) >= slowEvery) {
        _proposalsAt = now;
        missionProposals = await _loadAllProposals();
      }
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

  /// The latest attempt at a Mission, whose Session sets its state; `null` before one is
  /// recorded.
  MissionAttemptSummary? currentAttempt(String operationId, String missionId) =>
      attempts(operationId, missionId).fold<MissionAttemptSummary?>(
        null,
        (latest, attempt) => latest == null || attempt.attempt > latest.attempt
            ? attempt
            : latest,
      );

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
            ? 'Organization sign-in was rejected. Open Outer Office through your authenticated organization proxy and check its gateway configuration.'
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
    this.resources,
    this.config,
    this.headOfStation,
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

  /// The host's CPUs, load and memory; `null` from a Station that does not report them.
  final HostResources? resources;

  /// The roster revision the Station runs and what it refused; `null` when not reported.
  final StationConfigStatus? config;

  /// The latest report its Head of Station filed; `null` until one is filed.
  final StationLogReport? headOfStation;

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
    resources: json['resources'] == null
        ? null
        : HostResources.fromJson(json['resources'] as Map<String, dynamic>),
    config: json['config'] == null
        ? null
        : StationConfigStatus.fromJson(json['config'] as Map<String, dynamic>),
    headOfStation: json['head_of_station'] == null
        ? null
        : StationLogReport.fromJson(
            json['head_of_station'] as Map<String, dynamic>,
          ),
  );
}

/// One thing a Head of Station round found on its Station.
class StationLogFinding {
  const StationLogFinding({
    required this.kind,
    required this.severity,
    required this.text,
  });

  /// `disk`, `theater`, `safe-house`, `provider` or `other`.
  final String kind;

  /// `info`, `warning` or `critical`.
  final String severity;
  final String text;

  factory StationLogFinding.fromJson(Map<String, dynamic> json) =>
      StationLogFinding(
        kind: json['kind'] as String,
        severity: json['severity'] as String,
        text: json['text'] as String,
      );
}

/// A report a Station's Head of Station filed in its logbook.
class StationLogReport {
  const StationLogReport({
    required this.at,
    required this.summary,
    this.findings = const [],
    this.agent,
  });

  /// When it was filed, as Unix seconds.
  final int at;
  final String summary;
  final List<StationLogFinding> findings;
  final String? agent;

  factory StationLogReport.fromJson(Map<String, dynamic> json) =>
      StationLogReport(
        at: (json['at'] as num).toInt(),
        summary: json['summary'] as String,
        findings: ((json['findings'] as List<dynamic>?) ?? const [])
            .map(
              (finding) =>
                  StationLogFinding.fromJson(finding as Map<String, dynamic>),
            )
            .toList(growable: false),
        agent: json['agent'] as String?,
      );

  /// `Head of Station 007 · 14:05`, in local time.
  String get heading {
    final filed = DateTime.fromMillisecondsSinceEpoch(
      at * 1000,
      isUtc: true,
    ).toLocal();
    String two(int value) => value.toString().padLeft(2, '0');
    return [
      if (agent case final agent? when agent.isNotEmpty)
        'Head of Station $agent'
      else
        'Head of Station',
      '${two(filed.hour)}:${two(filed.minute)}',
    ].join(' · ');
  }
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

/// A Station host's or Theater's CPUs, load and memory as last sampled. A figure that was
/// not reported is `null` and shows as unknown.
class HostResources {
  const HostResources({
    required this.sampledAt,
    this.cpus,
    this.cpuPercent,
    this.load1m,
    this.memoryTotalMb,
    this.memoryAvailableMb,
  });

  final double? cpus;
  final double? cpuPercent;
  final double? load1m;
  final int? memoryTotalMb;
  final int? memoryAvailableMb;

  /// When it was sampled, as Unix seconds.
  final int sampledAt;

  factory HostResources.fromJson(Map<String, dynamic> json) => HostResources(
    cpus: (json['cpus'] as num?)?.toDouble(),
    cpuPercent: (json['cpu_percent'] as num?)?.toDouble(),
    load1m: (json['load_1m'] as num?)?.toDouble(),
    memoryTotalMb: json['memory_total_mb'] as int?,
    memoryAvailableMb: json['memory_available_mb'] as int?,
    sampledAt: json['sampled_at'] as int? ?? 0,
  );

  static String _number(double value) => value == value.roundToDouble()
      ? value.toStringAsFixed(0)
      : value.toStringAsFixed(1);

  static String _size(int mb) =>
      mb < 1024 ? '$mb MiB' : '${(mb / 1024).toStringAsFixed(1)} GiB';

  /// When it was sampled, as local `HH:MM:SS`.
  String get sampledTime {
    final at = DateTime.fromMillisecondsSinceEpoch(
      sampledAt * 1000,
      isUtc: true,
    ).toLocal();
    String two(int value) => value.toString().padLeft(2, '0');
    return '${two(at.hour)}:${two(at.minute)}:${two(at.second)}';
  }

  /// `8 CPUs · 23% busy · load 1.20 · 9.1 GiB free of 15.5 GiB · sampled 14:05:03`.
  String get description {
    final memory = switch ((memoryAvailableMb, memoryTotalMb)) {
      (final int free, final int total) =>
        '${_size(free)} free of ${_size(total)}',
      (null, final int total) => 'free memory unknown of ${_size(total)}',
      _ => 'memory unknown',
    };
    return [
      cpus == null ? 'CPUs unknown' : '${_number(cpus!)} CPUs',
      cpuPercent == null ? 'CPU busy unknown' : '${_number(cpuPercent!)}% busy',
      load1m == null ? 'load unknown' : 'load ${load1m!.toStringAsFixed(2)}',
      memory,
      'sampled $sampledTime',
    ].join(' · ');
  }
}

/// The gateway answered 403 to a placement preview: the person may not see where an
/// Operation's Missions would run.
class PlacementForbidden implements Exception {
  const PlacementForbidden();

  @override
  String toString() => 'This account may not preview placement here.';
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
    this.requires,
    this.report,
    this.madeFor,
    this.resources,
    this.sessions = 0,
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

  /// Settings looser than their defaults, and credentials found on the Theater outside
  /// AI6's leases.
  final List<String> weakened;

  /// What the Theater must have before Sessions run there.
  final TheaterRequirements? requires;

  /// What the Station's last probe found there.
  final TheaterReport? report;

  /// The Mission an ephemeral Theater was made for.
  final String? madeFor;

  /// Its CPUs, load and memory as last sampled; `null` until a sample succeeds.
  final HostResources? resources;

  /// Sessions the Station is running there.
  final int sessions;

  /// The Theater declares requirements that are unchecked or unmet.
  bool get unfit => requires != null && (report == null || !report!.fit);

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
    requires: json['requires'] == null
        ? null
        : TheaterRequirements.fromJson(
            json['requires'] as Map<String, dynamic>,
          ),
    report: json['report'] == null
        ? null
        : TheaterReport.fromJson(json['report'] as Map<String, dynamic>),
    madeFor: json['made_for'] as String?,
    resources: json['resources'] == null
        ? null
        : HostResources.fromJson(json['resources'] as Map<String, dynamic>),
    sessions: json['sessions'] as int? ?? 0,
  );

  String get description {
    final via = provider ?? host;
    final where = via == null ? access : '$access $via';
    final tags = labels.entries.map((e) => '${e.key}=${e.value}').join(', ');
    return [
      tags.isEmpty ? where : '$where · $tags',
      if (access == 'ephemeral') 'a fresh Theater is made for each Mission',
      if (madeFor != null) 'made for Mission $madeFor',
      if (unavailable != null) 'unreachable: $unavailable',
      resources?.description ?? 'resources unknown',
      if (sessions > 0) '$sessions running',
      if (capabilities != null) capabilities!.description,
      ...?capabilities?.notes,
      'agents $agents · egress $egress',
      if (harnesses.isNotEmpty) 'harnesses ${harnesses.join(', ')}',
      if (requires != null) 'requires ${requires!.description}',
      if (requires != null && report == null) 'requirements not checked yet',
      ...?report?.lines,
      ...weakened.map((setting) => 'weakened: $setting'),
    ].join('\n');
  }
}

/// What a Theater must have before Sessions run there.
class TheaterRequirements {
  const TheaterRequirements({
    this.os,
    this.harnesses = const [],
    this.tools = const [],
    this.diskMb,
  });

  final String? os;
  final List<String> harnesses;
  final List<String> tools;
  final int? diskMb;

  factory TheaterRequirements.fromJson(Map<String, dynamic> json) =>
      TheaterRequirements(
        os: json['os'] as String?,
        harnesses: ((json['harnesses'] as List<dynamic>?) ?? const [])
            .cast<String>(),
        tools: ((json['tools'] as List<dynamic>?) ?? const []).cast<String>(),
        diskMb: json['disk_mb'] as int?,
      );

  String get description => [
    ?os,
    ...harnesses,
    ...tools,
    if (diskMb != null) '$diskMb MiB free',
  ].join(', ');
}

/// What a Station's probe found on a Theater.
class TheaterReport {
  const TheaterReport({
    this.found = const {},
    this.freeMb,
    this.unmet = const [],
    this.error,
  });

  /// Programs found, with the first line each printed for `--version`.
  final Map<String, String> found;
  final int? freeMb;

  /// Each unmet requirement as `requirement: reason`.
  final List<String> unmet;

  /// Why the probe could not finish.
  final String? error;

  bool get fit => error == null && unmet.isEmpty;

  factory TheaterReport.fromJson(Map<String, dynamic> json) => TheaterReport(
    found: ((json['found'] as Map<String, dynamic>?) ?? const {}).map(
      (key, value) => MapEntry(key, value as String),
    ),
    freeMb: json['free_mb'] as int?,
    unmet: ((json['unmet'] as List<dynamic>?) ?? const [])
        .cast<Map<String, dynamic>>()
        .map((unmet) => '${unmet['requirement']}: ${unmet['reason']}')
        .toList(),
    error: json['error'] as String?,
  );

  List<String> get lines => [
    if (error != null) 'not checked: $error',
    ...unmet.map((unmet) => 'unmet: $unmet'),
    if (found.isNotEmpty)
      'found ${found.entries.map((e) => '${e.key} ${e.value}'.trim()).join(', ')}',
    if (freeMb != null) '$freeMb MiB free',
  ];
}

class PlacementChoice {
  const PlacementChoice({
    required this.station,
    required this.theater,
    required this.agent,
    required this.mode,
    required this.score,
  });

  final String station;
  final String theater;
  final String agent;

  /// `field` runs the harness on the Theater; `remote-hands` runs it on the Station and
  /// acts on the Theater through the transport.
  final String mode;

  /// How loaded the Station and Theater are, 0 (idle) to 4; `spread` prefers the lowest,
  /// `pack` the highest.
  final double score;

  factory PlacementChoice.fromJson(Map<String, dynamic> json) =>
      PlacementChoice(
        station: json['station'] as String,
        theater: json['theater'] as String,
        agent: json['agent'] as String,
        mode: json['mode'] as String,
        score: (json['score'] as num).toDouble(),
      );

  String get description =>
      '$station \u00b7 $theater \u00b7 $agent'
      '${mode == 'field' ? '' : ' \u00b7 $mode'}'
      ' \u00b7 score ${score.toStringAsFixed(2)}';
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
    this.roster = const [],
  });

  final String personId;
  final String name;
  final String? displayName;
  final List<String> groups;
  final double expiresAt;
  final List<UserOperationAccess> operations;
  final List<UserStationAccess> stations;

  /// The person's actions on the roster every Station reads (`roster.read`, `roster.manage`).
  final List<String> roster;

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
        roster: (json['roster'] as List<dynamic>? ?? const []).cast<String>(),
      );
}

class UserOperationAccess {
  const UserOperationAccess({
    required this.operationId,
    required this.actions,
    this.parent,
  });

  final String operationId;
  final List<String> actions;

  /// The nearest Operation above it the person may read; `null` for a root, or when every
  /// ancestor is hidden from the person.
  final String? parent;

  factory UserOperationAccess.fromJson(Map<String, dynamic> json) =>
      UserOperationAccess(
        operationId: json['operation_id'] as String,
        actions: (json['actions'] as List<dynamic>).cast<String>(),
        parent: json['parent'] as String?,
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
    this.minFreeMemoryMb = defaultMinFreeMemoryMb,
    this.maxCpuPercent = defaultMaxCpuPercent,
  });

  /// The Station's defaults when nothing was set: below 512 MiB free or above 95% CPU it
  /// takes no new Missions.
  static const defaultMinFreeMemoryMb = 512;
  static const defaultMaxCpuPercent = 95;

  final String stationId;
  final int maxConcurrentMissions;
  final bool configured;
  final bool executionEnabled;

  /// Free memory in MiB below which the Station takes no new Missions; 0 turns it off.
  final int minFreeMemoryMb;

  /// CPU percent above which the Station takes no new Missions; 100 turns it off.
  final int maxCpuPercent;

  factory StationSettingsSummary.fromJson(Map<String, dynamic> json) =>
      StationSettingsSummary(
        stationId: json['station_id'] as String,
        maxConcurrentMissions: json['max_concurrent_missions'] as int,
        configured: json['configured'] as bool,
        executionEnabled: json['execution_enabled'] as bool,
        minFreeMemoryMb:
            json['min_free_memory_mb'] as int? ?? defaultMinFreeMemoryMb,
        maxCpuPercent: json['max_cpu_percent'] as int? ?? defaultMaxCpuPercent,
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
    this.vetting,
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

  /// What M's vetting found: `null` until it has looked, empty when the proposal is clean.
  final List<VettingFinding>? vetting;

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
      vetting: json['vetting'] == null
          ? null
          : vettingFindings(json['vetting'] as Map<String, dynamic>),
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
  const _DesktopWindowHost({required this.board, required this.chatSettings});

  final MissionBoard board;
  final ChatSettings chatSettings;

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
      title: 'Outer Office | AI6',
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
        builder: (_) => OuterOfficeApp(
          board: widget.board,
          desktopWindows: true,
          chatSettings: widget.chatSettings,
        ),
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
    this.stationId,
    this.agent,
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

  /// The Station that holds, or last held, the Session; `null` when the gateway has no
  /// record of it.
  final String? stationId;

  /// The Agent its Mission's brief names; `null` before the brief arrives.
  final String? agent;

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
        stationId: stationId,
        agent: agent,
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
    stationId: json['station_id'] as String?,
    agent: json['agent'] as String?,
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
    this.origin,
  });

  final String operationId;
  final String missionId;
  final String prompt;

  /// Where the brief came from, as the Station recorded it; `null` for a Mission started
  /// directly on a Station.
  final BriefOrigin? origin;
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
      origin: switch (brief['provenance']) {
        final Map<String, dynamic> provenance => BriefOrigin.fromJson(
          provenance,
        ),
        _ => null,
      },
    );
  }
}

/// Who asked for a Mission: the provenance AI6 recorded where its brief entered.
class BriefOrigin {
  const BriefOrigin({
    required this.origin,
    required this.receivedBy,
    this.principal,
    this.reference,
    this.itemUrl,
  });

  /// `operator`, `source`, `sleeper`, `proposal`, `review` or `registry_drop`.
  final String origin;

  /// The Station, or `universal-exports`, that accepted the brief.
  final String receivedBy;

  /// The person, Source provider, Sleeper or proposing Agent; `null` for a registry drop.
  final String? principal;

  /// The Source item, Sleeper fingerprint or proposing Session.
  final String? reference;
  final String? itemUrl;

  /// A brief nobody AI6 authenticated put into the queue.
  bool get isRegistryDrop => origin == 'registry_drop';

  factory BriefOrigin.fromJson(Map<String, dynamic> json) => BriefOrigin(
    origin: json['origin'] as String,
    receivedBy: json['received_by'] as String,
    principal: json['principal'] as String?,
    reference: json['reference'] as String?,
    itemUrl: json['item_url'] as String?,
  );

  /// One line for the Mission detail.
  String describe() => switch (origin) {
    'operator' => 'Operator ${principal ?? 'unknown'} via $receivedBy',
    'source' =>
      'Source ${principal ?? 'unknown'} item ${reference ?? 'unknown'}',
    'sleeper' => 'Sleeper ${principal ?? 'unknown'}, fired by $receivedBy',
    'proposal' =>
      'Proposal by Agent ${principal ?? 'unknown'}, approved via $receivedBy',
    'review' => 'Chief of Staff review queued by $receivedBy',
    'registry_drop' =>
      'Dropped into the Registry queue (unauthenticated), found by $receivedBy',
    _ => '$origin via $receivedBy',
  };
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

/// The app: its pages are routes ([OoRoute]), read from and written to the web URL after the
/// `#` (`#/missions?state=failed`).
class OuterOfficeApp extends StatefulWidget {
  const OuterOfficeApp({
    required this.board,
    required this.desktopWindows,
    this.chatSettings = const ChatSettings(),
    super.key,
  });

  final MissionBoard board;
  final bool desktopWindows;

  /// Moneypenny's chat sizes and timing, `OO.cfg`'s `[chat]`.
  final ChatSettings chatSettings;

  @override
  State<OuterOfficeApp> createState() => _OuterOfficeAppState();
}

class _OuterOfficeAppState extends State<OuterOfficeApp> {
  late final OoRouterDelegate _router = OoRouterDelegate(
    shell: (context, router) => _AccessGate(
      board: widget.board,
      desktopWindows: widget.desktopWindows,
      chatSettings: widget.chatSettings,
      router: router,
    ),
    memory: (context) => widget.board.demo || widget.board.accessToken != null
        ? MemoryPage(board: widget.board)
        : null,
  );
  final _location = PlatformRouteInformationProvider(
    initialRouteInformation: RouteInformation(
      uri: Uri.parse(
        WidgetsBinding.instance.platformDispatcher.defaultRouteName,
      ),
    ),
  );
  late final RouterConfig<OoRoute> _config = RouterConfig(
    routeInformationProvider: _location,
    routeInformationParser: OoRouteParser(onUnknown: _router.unknown),
    routerDelegate: _router,
    backButtonDispatcher: RootBackButtonDispatcher(),
  );

  @override
  void dispose() {
    _router.dispose();
    _location.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.board,
    builder: (context, _) => OoMaterialApp(
      themeId: widget.board.themeId,
      backdrop: widget.board.backdrop,
      title: 'Outer Office | AI6',
      routerConfig: _config,
    ),
  );
}

class _AccessGate extends StatefulWidget {
  const _AccessGate({
    required this.board,
    required this.desktopWindows,
    required this.chatSettings,
    required this.router,
  });

  final MissionBoard board;
  final bool desktopWindows;
  final ChatSettings chatSettings;
  final OoRouterDelegate router;

  @override
  State<_AccessGate> createState() => _AccessGateState();
}

class _AccessGateState extends State<_AccessGate> {
  MoneypennyChat? _chat;

  @override
  void dispose() {
    _drop();
    super.dispose();
  }

  /// The conversation with Moneypenny for the signed-in token: made at sign-in, replaced
  /// when the token changes, gone at sign-out. The preview gets the scripted sample
  /// conversation, which reaches no network, and nothing else does.
  MoneypennyChat? _chatFor(MissionBoard board) {
    final token = board.demo ? 'demo' : board.accessToken;
    if (token == null) {
      _drop();
      return null;
    }
    if (_chat case final chat? when chat.controller.token == token) return chat;
    _drop();
    return _chat = MoneypennyChat(
      controller: AssistantController(
        token: token,
        connect: board.demo
            ? DemoAssistantChannel.new
            : board.assistantChannel ?? () => connectAssistant(board.apiOrigin),
      ),
      label: board.demo ? demoConversationLabel : null,
      preferences: ChatPreferences(SharedPreferences.getInstance()),
      settings: widget.chatSettings,
      openWindow: widget.desktopWindows
          ? (context, chat, onClosed) =>
                _openChatWindow(context, chat, onClosed, board)
          : null,
    );
  }

  /// Disposes the old conversation after this frame, once nothing shows it.
  void _drop() {
    final chat = _chat;
    if (chat == null) return;
    _chat = null;
    WidgetsBinding.instance.addPostFrameCallback((_) => chat.dispose());
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.board,
    builder: (context, _) {
      final board = widget.board;
      final chat = _chatFor(board);
      if (!board.demo && board.accessToken == null) {
        return _LoginPage(board: board);
      }
      return MissionBoardPage(
        board: board,
        desktopWindows: widget.desktopWindows,
        chat: chat,
        route: widget.router.page,
        onRoute: (route) => widget.router.go(context, route),
        onFollow: (route) => widget.router.follow(context, route),
        openedByMoneypenny: widget.router.openedByMoneypenny,
        notice: widget.router.notice,
        onDismissNotice: widget.router.dismissNotice,
      );
    },
  );
}

/// Moneypenny's own window, themed like the app.
AppWindow _openChatWindow(
  BuildContext context,
  MoneypennyChat chat,
  VoidCallback onClosed,
  MissionBoard board,
) => openChatWindow(
  context,
  chat: chat,
  onClosed: onClosed,
  app: (home) => boardThemedApp(board, title: chatWindowTitle, home: home),
);

/// The app of a native window: [home] in the board's theme and wallpaper, following
/// every change the person makes in the main window.
Widget boardThemedApp(
  MissionBoard board, {
  required String title,
  required Widget home,
}) => AnimatedBuilder(
  animation: board,
  builder: (context, _) => OoMaterialApp(
    themeId: board.themeId,
    backdrop: board.backdrop,
    title: title,
    home: home,
  ),
);

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
    appBar: AppBar(title: const Text('Outer Office')),
    body: SizedBox.expand(
      child: OoBackdrop(
        alignment: Alignment.center,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 420),
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

class MissionBoardPage extends StatefulWidget {
  const MissionBoardPage({
    required this.board,
    required this.desktopWindows,
    this.chat,
    required this.route,
    required this.onRoute,
    this.onFollow,
    this.openedByMoneypenny = false,
    this.notice,
    this.onDismissNotice,
    super.key,
  });

  final MissionBoard board;
  final bool desktopWindows;

  /// The conversation with Moneypenny; none in the preview or before sign-in.
  final MoneypennyChat? chat;

  /// The page shown and its filters.
  final OoRoute route;

  /// Opens another route: a new history entry for another page, a replaced one for a
  /// filter of this page.
  final ValueChanged<OoRoute> onRoute;

  /// Opens a route from one of Moneypenny's links or cards: always a new history entry, and
  /// marked as hers; [onRoute] where unset.
  final ValueChanged<OoRoute>? onFollow;

  /// Whether one of her links or cards opened [route]; the page then says so.
  final bool openedByMoneypenny;

  /// Why a link opened Overview instead of its page, until dismissed.
  final String? notice;
  final VoidCallback? onDismissNotice;

  @override
  State<MissionBoardPage> createState() => _MissionBoardPageState();
}

class _MissionBoardPageState extends State<MissionBoardPage> {
  /// Opens the app page one of her views, links or navigation cards points to
  /// (`app:/missions?operation_id=G`); a link that is not a route is ignored. An expanded chat
  /// docks and one over the page closes, so the page is in view. Opening only reads.
  void _openLink(String link) {
    final OoRoute route;
    try {
      route = OoRoute.parseUri(link);
    } on FormatException {
      return;
    }
    if (!mounted) return;
    if (widget.chat case final chat?) {
      switch (chatPlacement(chat.layout, MediaQuery.sizeOf(context).width)) {
        case ChatPlacement.centre:
          chat.dock();
        case ChatPlacement.sheet || ChatPlacement.fullScreen:
          chat.collapse();
        case _:
      }
    }
    (widget.onFollow ?? widget.onRoute)(route);
  }

  bool get _canReadRoles =>
      widget.board.demo ||
      (widget.board.userSession?.operations.any(
            (operation) => operation.actions.contains('agent.read'),
          ) ??
          false);

  /// The rail: every page this person's permissions open, in the kit's order.
  List<OoPage> _pages(MissionBoard board) => [
    OoPage.overview,
    OoPage.operations,
    OoPage.missions,
    OoPage.sessions,
    if (_canReadRoles) OoPage.agents,
    OoPage.stations,
    if (board.greenInkOperations.isNotEmpty ||
        board.classAuthorizationOperations.isNotEmpty)
      OoPage.greenInk,
    if (board.proposalOperations.isNotEmpty) OoPage.briefings,
    if (board.canReadRoster) OoPage.roster,
    if (board.poolOperations.isNotEmpty) OoPage.pools,
    if (board.timelineOperations.isNotEmpty) OoPage.timeline,
    if (board.demo || board.canReadAudit) OoPage.audit,
  ];

  void _openSession(BuildContext context, SessionSummary session) {
    if (widget.desktopWindows) {
      openSessionWindow(context, session, widget.board);
    } else {
      openSessionPage(context, session, widget.board);
    }
  }

  void _open(OoPage page) {
    // An expanded chat hides the page; going to a page shows it.
    if (widget.chat case final chat? when chat.layout == ChatLayout.expanded) {
      chat.dock();
    }
    widget.onRoute(OoRoute(page));
  }

  /// The tab a person picks, kept in the route.
  void _tab(OoRoute route, String tab) =>
      widget.onRoute(route.withField('tab', tab));

  Widget _pageBody(BuildContext context, OoRoute route, List<OoPage> pages) =>
      AnimatedBuilder(
        animation: widget.board,
        builder: (context, _) {
          final board = widget.board;
          final onRoute = widget.onRoute;
          return switch (route.page) {
            OoPage.overview || OoPage.memory => OverviewPage(
              board: board,
              pages: pages,
              onOpen: _open,
              activityEvery: board.limits.refresh.overview,
              assistant: widget.chat?.controller,
              onAsk: (text) {
                widget.chat?.open();
                widget.chat?.controller.ask(
                  text,
                  route: const OoRoute(OoPage.overview),
                );
              },
            ),
            OoPage.operations => OperationsPage(
              board: board,
              route: route,
              onRoute: onRoute,
              pages: pages,
            ),
            OoPage.missions => MissionsPage(
              board: board,
              route: route,
              onRoute: onRoute,
              desktopWindows: widget.desktopWindows,
              pages: pages,
            ),
            OoPage.sessions => SessionsPage(
              board: board,
              route: route,
              onRoute: onRoute,
              onOpen: (session) => _openSession(context, session),
            ),
            OoPage.agents => OoTabs(
              empty: 'No Agents are available to this account.',
              selected: route['tab'],
              onSelect: (tab) => _tab(route, tab),
              tabs: [
                if (_canReadRoles)
                  (
                    'agents',
                    'Agents',
                    AgentsPage(board: board, route: route, onRoute: onRoute),
                  ),
                if (_canReadRoles)
                  (
                    'roles',
                    'Roles',
                    AgentRolesPage(
                      board: board,
                      route: route,
                      onRoute: onRoute,
                    ),
                  ),
              ],
            ),
            OoPage.stations => OoTabs(
              empty: 'No Stations are available to this account.',
              selected: route['tab'],
              onSelect: (tab) => _tab(route, tab),
              tabs: [
                ('overview', 'Stations', _stationsTab(context, route)),
                (
                  'theaters',
                  'Theaters',
                  _TheatersTab(
                    board: board.stationBoard,
                    stationId: route['station_id'],
                    theaterId: route['theater_id'],
                  ),
                ),
                if (board.demo || board.readableStations.isNotEmpty)
                  (
                    'settings',
                    'Settings',
                    StationSettingsPage(
                      board: board,
                      route: route,
                      onRoute: onRoute,
                    ),
                  ),
                if (board.previewOperations.isNotEmpty)
                  (
                    'placement',
                    'Placement',
                    ListView(children: [_PlacementPanel(board: board)]),
                  ),
              ],
            ),
            OoPage.greenInk => OoTabs(
              empty: 'No green ink is available to this account.',
              selected: route['tab'],
              onSelect: (tab) => _tab(route, tab),
              tabs: [
                if (board.greenInkOperations.isNotEmpty)
                  (
                    'inbox',
                    'Inbox',
                    GreenInkPage(board: board, route: route, onRoute: onRoute),
                  ),
                if (board.classAuthorizationOperations.isNotEmpty)
                  (
                    'classes',
                    'Class authorisations',
                    ClassAuthorizationsPage(
                      board: board,
                      route: route,
                      onRoute: onRoute,
                    ),
                  ),
              ],
            ),
            OoPage.briefings => OoTabs(
              empty: 'No briefings are available to this account.',
              selected: route['tab'],
              onSelect: (tab) => _tab(route, tab),
              tabs: [
                (
                  'proposals',
                  'Mission proposals',
                  MissionProposalsPage(
                    board: board,
                    route: route,
                    onRoute: onRoute,
                  ),
                ),
                (
                  'held',
                  'Held briefs',
                  HeldBriefsPage(board: board, route: route, onRoute: onRoute),
                ),
              ],
            ),
            OoPage.roster => RosterPage(
              board: board,
              route: route,
              onRoute: onRoute,
            ),
            OoPage.pools => AccountPoolsPage(
              board: board,
              route: route,
              onRoute: onRoute,
              refreshEvery: board.limits.refresh.pools,
            ),
            OoPage.timeline => OperationTimelinePage(
              board: board,
              route: route,
              onRoute: onRoute,
              refreshEvery: board.limits.refresh.timeline,
            ),
            OoPage.audit => AuditLogPage(
              board: board,
              route: route,
              onRoute: onRoute,
            ),
          };
        },
      );

  Widget _stationsTab(BuildContext context, OoRoute route) {
    final stationBoard = widget.board.stationBoard;
    final stationId = route['station_id'];
    return ListView(
      children: [
        OoFilterBar(
          route: route,
          onRoute: widget.onRoute,
          padding: const EdgeInsets.fromLTRB(24, 16, 24, 0),
        ),
        StationOverview(
          board: widget.board,
          onRoute: widget.onRoute,
          detailed: stationId != null,
          stations: stationId == null
              ? stationBoard
              : StationBoardSummary(
                  headquartersStation: stationBoard.headquartersStation,
                  stations: [
                    for (final station in stationBoard.stations)
                      if (station.stationId == stationId) station,
                  ],
                ),
          empty: stationId == null
              ? 'No Stations have checked in yet.'
              : 'Station $stationId is not on this board.',
          onCreateMission: (operationId) =>
              _showCreateMission(context, operationId, widget.board),
          canCreateMission: (operationId) =>
              !widget.board.demo && widget.board.canCreateMission(operationId),
        ),
        if (widget.board.demo) const DemoBanner(),
        if (widget.board.error != null)
          ErrorBanner(
            apiOrigin: widget.board.apiOrigin,
            onRetry: widget.board.refresh,
          ),
      ],
    );
  }

  /// Copies the web link of the current page, `<OO_API origin>/#<route>`: the web app is
  /// served from the gateway's origin.
  Future<void> _copyLink(BuildContext context, OoRoute route) async {
    final link = '${widget.board.apiOrigin.origin}/#${route.location}';
    await Clipboard.setData(ClipboardData(text: link));
    if (context.mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Copied $link')));
    }
  }

  @override
  Widget build(BuildContext context) {
    final pages = _pages(widget.board);
    final route = pages.contains(widget.route.page)
        ? widget.route
        : const OoRoute(OoPage.overview);
    final chat = widget.chat;
    final width = MediaQuery.sizeOf(context).width;
    final phone = width < OoBreakpoints.phone;
    final notice = widget.notice;
    return ChatShell(
      chat: chat,
      personId: widget.board.userSession?.personId,
      route: route,
      onLink: _openLink,
      onMemory: (context) => Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) => MemoryPage(board: widget.board),
        ),
      ),
      child: OoShell(
        pages: pages,
        page: route.page,
        onSelect: _open,
        search: (expanded) => AnimatedBuilder(
          animation: widget.board,
          builder: (context, _) => OoSearch(
            board: widget.board,
            pages: pages,
            onOpen: _open,
            expanded: expanded,
          ),
        ),
        actions: [
          if (!phone)
            Builder(
              builder: (context) => IconButton(
                tooltip: 'Copy link to this page',
                onPressed: () => unawaited(_copyLink(context, route)),
                icon: const Icon(Icons.link),
              ),
            ),
          if (chat != null)
            MoneypennyButton(chat: chat)
          else
            const IconButton(
              tooltip: 'Moneypenny answers once you are signed in',
              onPressed: null,
              icon: OoIcon(OoIconName.moneypenny),
            ),
          AnimatedBuilder(
            animation: widget.board,
            builder: (context, _) =>
                AttentionBell(board: widget.board, pages: pages, onOpen: _open),
          ),
          if (!phone)
            IconButton(
              tooltip: 'Appearance settings',
              onPressed: () => _showAppearance(context, widget.board),
              icon: const OoIcon(OoIconName.settings),
            ),
          const SizedBox(width: 4),
          AnimatedBuilder(
            animation: widget.board,
            builder: (context, _) => UserChip(
              session: widget.board.userSession,
              compact: width < OoBreakpoints.wide,
              onPressed: () => _showOperatorMenu(context, widget.board),
            ),
          ),
        ],
        body: ChatRegion(
          chat: chat,
          pageLabel: route.page.label,
          child: Column(
            children: [
              if (notice != null)
                MaterialBanner(
                  content: Text(notice),
                  leading: const Icon(Icons.link_off),
                  actions: [
                    TextButton(
                      onPressed: widget.onDismissNotice,
                      child: const Text('Dismiss'),
                    ),
                  ],
                ),
              if (widget.openedByMoneypenny) const OpenedByMoneypenny(),
              Expanded(child: _pageBody(context, route, pages)),
            ],
          ),
        ),
      ),
    );
  }
}

/// Every Theater each Station reports, with what it can do and why it may not be used.
class _TheatersTab extends StatelessWidget {
  const _TheatersTab({required this.board, this.stationId, this.theaterId});

  final StationBoardSummary board;

  /// Only this Station's Theaters, and of them only [theaterId], when given.
  final String? stationId;
  final String? theaterId;

  @override
  Widget build(BuildContext context) {
    final theaters = [
      for (final station in board.stations)
        if (stationId == null || station.stationId == stationId)
          for (final theater in station.theaters)
            if (theaterId == null || theater.id == theaterId)
              (station, theater),
    ];
    if (theaters.isEmpty) {
      return Center(
        child: Text(switch ((stationId, theaterId)) {
          (final station?, final theater?) =>
            'Station $station reports no Theater $theater.',
          (final station?, null) => 'Station $station reports no Theater.',
          _ => 'No Station reports a Theater.',
        }),
      );
    }
    final muted = OoTokens.of(context).muted;
    return ListView.separated(
      padding: const EdgeInsets.all(24),
      itemCount: theaters.length,
      separatorBuilder: (_, _) => const SizedBox(height: 8),
      itemBuilder: (context, index) {
        final (station, theater) = theaters[index];
        return Card(
          child: ListTile(
            leading: const OoIcon(OoIconName.theaters),
            title: Text('Theater ${theater.id}'),
            subtitle: Text(
              'Station ${station.stationId}\n${theater.description}',
              style: TextStyle(color: muted),
            ),
            isThreeLine: true,
          ),
        );
      },
    );
  }
}

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

class MissionProposalsPage extends StatefulWidget {
  const MissionProposalsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;

  /// A Briefings route: its `operation_id` and `item_id` (a proposal ID).
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<MissionProposalsPage> createState() => _MissionProposalsPageState();
}

class _MissionProposalsPageState extends State<MissionProposalsPage> {
  Future<List<MissionProposalSummary>>? _proposals;
  bool _deciding = false;

  List<String> get _operations => widget.board.proposalOperations;

  /// The route's Operation, else the first this person may create Missions in.
  String? get _operationId =>
      widget.route['operation_id'] ??
      (_operations.isEmpty ? null : _operations.first);

  void _load() {
    final operationId = _operationId;
    _proposals = operationId != null && _operations.contains(operationId)
        ? widget.board.loadMissionProposals(operationId)
        : null;
  }

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(MissionProposalsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      _load();
    }
  }

  void _selectOperation(String? operationId) {
    if (operationId == null) return;
    widget.onRoute(
      widget.route.without('item_id').withField('operation_id', operationId),
    );
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
    final itemId = widget.route['item_id'];
    if (operations.isNotEmpty &&
        operationId != null &&
        !operations.contains(operationId)) {
      return Center(
        child: Text(
          'Operation $operationId does not grant this account Mission creation.',
        ),
      );
    }
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
                    // Keyed so that a route change replaces the field's first value.
                    key: ValueKey(operationId),
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
                const SizedBox(height: 8),
                OoFilterBar(
                  route: widget.route,
                  onRoute: widget.onRoute,
                  padding: EdgeInsets.zero,
                ),
                const SizedBox(height: 8),
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
                      final proposals = [
                        for (final proposal in snapshot.data!)
                          if (itemId == null || proposal.id == itemId) proposal,
                      ];
                      if (proposals.isEmpty) {
                        return Center(
                          child: Text(
                            itemId == null
                                ? 'No pending Mission proposals.'
                                : 'No pending Mission proposal $itemId in Operation '
                                      '$operationId.',
                          ),
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
                                  const SizedBox(height: 6),
                                  ...switch (proposal.vetting) {
                                    null => [
                                      const Text('Vetting: not finished'),
                                    ],
                                    [] => [
                                      const Text('Vetting: nothing found'),
                                    ],
                                    final findings => [
                                      const Text('Vetting found:'),
                                      for (final finding in findings)
                                        Text('• ${finding.line}'),
                                    ],
                                  },
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
  const StationSettingsPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;

  /// A Stations route: its `station_id` picks the Station.
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<StationSettingsPage> createState() => _StationSettingsPageState();
}

class _StationSettingsPageState extends State<StationSettingsPage> {
  final _capacity = TextEditingController();
  final _minFreeMemory = TextEditingController();
  final _maxCpu = TextEditingController();
  String? _stationId;
  Future<StationSettingsSummary>? _settings;
  bool _saving = false;

  List<String> get _stations => widget.board.demo
      ? const ['station-control-01']
      : widget.board.readableStations;

  /// The route's Station, else the first whose settings this person may read.
  String? get _routeStation =>
      widget.route['station_id'] ??
      (_stations.isEmpty ? null : _stations.first);

  @override
  void initState() {
    super.initState();
    final station = _routeStation;
    if (station != null && _stations.contains(station)) _load(station);
  }

  @override
  void didUpdateWidget(StationSettingsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    final station = _routeStation;
    if (station != _stationId &&
        station != null &&
        _stations.contains(station)) {
      _load(station);
    }
  }

  @override
  void dispose() {
    _capacity.dispose();
    _minFreeMemory.dispose();
    _maxCpu.dispose();
    super.dispose();
  }

  void _selectStation(String? stationId) {
    if (stationId == null) return;
    widget.onRoute(
      widget.route.without('theater_id').withField('station_id', stationId),
    );
  }

  void _load(String stationId) {
    final settings = widget.board.loadStationSettings(stationId);
    setState(() {
      _stationId = stationId;
      _settings = settings;
    });
    unawaited(
      settings.then((value) {
        if (mounted && _stationId == stationId) {
          _capacity.text = '${value.maxConcurrentMissions}';
          _minFreeMemory.text = '${value.minFreeMemoryMb}';
          _maxCpu.text = '${value.maxCpuPercent}';
        }
      }),
    );
  }

  Future<void> _save() async {
    final stationId = _stationId;
    final capacity = int.tryParse(_capacity.text.trim());
    final minFreeMemory = int.tryParse(_minFreeMemory.text.trim());
    final maxCpu = int.tryParse(_maxCpu.text.trim());
    String? problem;
    if (capacity == null || capacity < 1 || capacity > 1024) {
      problem = 'Mission capacity must be from 1 to 1,024.';
    } else if (minFreeMemory == null || minFreeMemory < 0) {
      problem = 'Minimum free memory must be a whole number of MiB, 0 or more.';
    } else if (maxCpu == null || maxCpu < 0 || maxCpu > 100) {
      problem = 'Maximum CPU must be a whole percent from 0 to 100.';
    }
    if (stationId == null || problem != null) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(problem ?? 'Choose a Station.')));
      return;
    }
    setState(() => _saving = true);
    try {
      await widget.board.saveStationSettings(
        stationId,
        capacity!,
        minFreeMemoryMb: minFreeMemory,
        maxCpuPercent: maxCpu,
      );
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
    final station = _routeStation;
    if (station != null && !stations.contains(station)) {
      return Center(
        child: Text(
          'Station $station does not grant this account settings access.',
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: DropdownButtonFormField<String>(
              // Keyed so that a route change replaces the field's first value.
              key: ValueKey(_stationId),
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
                          ? 'Configured through Outer Office. Changes apply to new Missions.'
                          : 'Current Station value. Save to establish an authorized live setting.',
                    ),
                    const SizedBox(height: 16),
                    ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 320),
                      child: TextField(
                        key: const Key('station-capacity'),
                        controller: _capacity,
                        enabled: canConfigure && !_saving,
                        keyboardType: TextInputType.number,
                        decoration: const InputDecoration(
                          labelText: 'Maximum concurrent Missions',
                          helperText: 'Allowed range: 1–1,024',
                        ),
                      ),
                    ),
                    const SizedBox(height: 20),
                    Text(
                      'Resource limits',
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    const SizedBox(height: 6),
                    const Text(
                      'The Station and its Theaters take no new Missions with less free '
                      'memory or a busier CPU than this.',
                    ),
                    const SizedBox(height: 16),
                    ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 320),
                      child: TextField(
                        key: const Key('station-min-free-memory'),
                        controller: _minFreeMemory,
                        enabled: canConfigure && !_saving,
                        keyboardType: TextInputType.number,
                        decoration: InputDecoration(
                          labelText: 'Minimum free memory (MiB)',
                          helperText:
                              'Default ${StationSettingsSummary.defaultMinFreeMemoryMb}; '
                              '0 turns the floor off',
                        ),
                      ),
                    ),
                    const SizedBox(height: 16),
                    ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 320),
                      child: TextField(
                        key: const Key('station-max-cpu'),
                        controller: _maxCpu,
                        enabled: canConfigure && !_saving,
                        keyboardType: TextInputType.number,
                        decoration: InputDecoration(
                          labelText: 'Maximum CPU (%)',
                          helperText:
                              'Default ${StationSettingsSummary.defaultMaxCpuPercent}; '
                              '0–100, 100 turns the ceiling off',
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
                    if (widget.board.demo) const DemoBanner(),
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
  const AgentRolesPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;

  /// An Agents route: its `operation_id` and `role`.
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<AgentRolesPage> createState() => _AgentRolesPageState();
}

class _AgentRolesPageState extends State<AgentRolesPage> {
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

  /// The route's Operation, else the first whose roles this person may read.
  String? get _operationId =>
      widget.route['operation_id'] ??
      (_operations.isEmpty ? null : _operations.first);

  void _load() {
    final operationId = _operationId;
    _roles = operationId != null && _operations.contains(operationId)
        ? widget.board.loadAgentRoles(operationId)
        : null;
  }

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(AgentRolesPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.route['operation_id'] != widget.route['operation_id']) {
      _load();
    }
  }

  void _selectOperation(String? operationId) {
    if (operationId == null) return;
    widget.onRoute(
      widget.route.without('role').withField('operation_id', operationId),
    );
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
    final roleId = widget.route['role'];
    if (operations.isNotEmpty &&
        operationId != null &&
        !operations.contains(operationId)) {
      return Center(
        child: Text(
          'Operation $operationId does not grant this account Agent role access.',
        ),
      );
    }
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
                    // Keyed so that a route change replaces the field's first value.
                    key: ValueKey(operationId),
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
                const SizedBox(height: 8),
                OoFilterBar(
                  route: widget.route,
                  onRoute: widget.onRoute,
                  padding: EdgeInsets.zero,
                ),
                const SizedBox(height: 12),
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
                      final roles = [
                        for (final role in snapshot.data!)
                          if (roleId == null || role.id == roleId) role,
                      ];
                      if (roles.isEmpty) {
                        return Center(
                          child: Text(
                            roleId == null
                                ? 'No Agent roles are configured.'
                                : 'Role $roleId is not configured in Operation '
                                      '$operationId.',
                          ),
                        );
                      }
                      return ListView.separated(
                        itemCount: roles.length,
                        separatorBuilder: (_, _) => const SizedBox(height: 8),
                        itemBuilder: (context, index) {
                          final role = roles[index];
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
  const AuditLogPage({
    required this.board,
    required this.route,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;

  /// An Audit route: its `operation_id` or `station_id` keeps only that scope's records.
  final OoRoute route;
  final ValueChanged<OoRoute> onRoute;

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

  bool _selected(AuditEntrySummary record) {
    if (widget.route['operation_id'] case final operation?) {
      return record.operation == operation;
    }
    if (widget.route['station_id'] case final station?) {
      return record.resource == 'station:$station';
    }
    return true;
  }

  @override
  Widget build(BuildContext context) => Column(
    children: [
      Row(
        children: [
          Expanded(
            child: OoFilterBar(
              route: widget.route,
              onRoute: widget.onRoute,
              padding: const EdgeInsets.fromLTRB(20, 8, 8, 0),
            ),
          ),
          IconButton(
            tooltip: 'Refresh audit log',
            onPressed: _refresh,
            icon: const Icon(Icons.refresh),
          ),
        ],
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
            final records = snapshot.requireData.where(_selected).toList();
            if (records.isEmpty) {
              return Center(
                child: Text(
                  widget.route.filtered
                      ? 'No audit records match these filters.'
                      : 'No audit records are available.',
                ),
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
                  _ThemeChooser(board: board),
                  if (!kIsWeb && !board.demo) ...[
                    const SizedBox(height: 18),
                    ConnectionSettings(),
                  ],
                ],
              ),
            ),
          ),
        ),
      ),
      actions: [
        if (!board.demo)
          FilledButton.icon(
            style: ooDangerButton(OoTokens.of(dialogContext)),
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

/// The top bar's settings: the operator menu's appearance settings on their own.
Future<void> _showAppearance(BuildContext context, MissionBoard board) =>
    showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Appearance'),
        content: SizedBox(
          width: 440,
          child: SingleChildScrollView(
            child: AnimatedBuilder(
              animation: board,
              builder: (context, _) => _ThemeChooser(board: board),
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('Close'),
          ),
        ],
      ),
    );

/// The operator menu's and the top bar's appearance settings, applied and stored at once.
class _ThemeChooser extends StatelessWidget {
  const _ThemeChooser({required this.board});

  final MissionBoard board;

  @override
  Widget build(BuildContext context) => OoAppearanceChooser(
    themeId: board.themeId,
    backdrop: board.backdrop,
    onTheme: board.setThemeId,
    onBackdrop: board.setBackdrop,
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
                  color: OoTokens.of(context).accent,
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
                  maxLength: InputSettings.field(
                    board.limits.inputs.missionTitleChars,
                  ),
                  onChanged: (_) => setState(() {}),
                  decoration: const InputDecoration(labelText: 'Mission title'),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: prompt,
                  enabled: !submitting,
                  minLines: 5,
                  maxLines: 10,
                  maxLength: InputSettings.field(
                    board.limits.inputs.missionPromptChars,
                  ),
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
