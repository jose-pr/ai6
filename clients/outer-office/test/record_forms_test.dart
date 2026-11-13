import 'dart:convert';
import 'dart:io';

// Flutter's own TextField is what guatiao_flutter draws; the app's is material_ui's.
// ignore: deprecated_member_use
import 'package:flutter/material.dart' as flutter show TextField;
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/forms/form_host.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/roster/record_summary.dart';

import 'shell_navigation.dart';

/// The routes' answers, as the gateway's types declare them.
final _forms = jsonDecode(
  File('assets/record_forms.json').readAsStringSync(),
) as Map<String, dynamic>;

Map<String, Object?> _schema(String kind) =>
    (_forms[kind] as Map<String, dynamic>)['schema'] as Map<String, Object?>;
Map<String, Object?> _form(String kind) =>
    (_forms[kind] as Map<String, dynamic>)['form'] as Map<String, Object?>;

FormModel _model(String kind, [Map<String, Object?> value = const {}]) =>
    ooFormModel(schema: _schema(kind), form: _form(kind), value: value);

http.Response _json(Object? body, [int status = 200]) => http.Response(
  jsonEncode(body),
  status,
  headers: {'content-type': 'application/json'},
);

/// The roster routes of a gateway, over records held in memory.
class FakeRoster {
  FakeRoster({this.writable = true, Map<String, Map<String, dynamic>>? records})
    : records = records ?? {};

  final bool writable;

  /// `kind/key` to its definition; a Station's own record is `kind@station/key`.
  final Map<String, Map<String, dynamic>> records;
  final requests = <http.Request>[];
  int revision = 12;

  /// Refuses a save with this reason, once.
  String? refuse;

  /// Holds a save back with these weakenings until it is sent with `confirm=true`.
  List<String>? hold;

  String _token(String id) => jsonEncode(records[id]).hashCode.toString();

  http.Response answer(http.Request request) {
    requests.add(request);
    final parts = request.url.pathSegments;
    final kind = parts[3];
    final station = request.url.queryParameters['station'];
    final scope = station == null ? kind : '$kind@$station';
    if (parts.length == 5 && parts[4] == 'schema') {
      return _json(_forms[kind]);
    }
    if (parts.length == 4) {
      return _json({
        'kind': kind,
        'revision': '$revision',
        'writable': writable,
        'records': [
          for (final id in records.keys)
            if (id.startsWith('$scope/'))
              {
                'key': id.substring(scope.length + 1),
                'definition': records[id],
                'revision': _token(id),
              },
        ],
      });
    }
    final id = '$scope/${parts[4]}';
    final expected = request.headers['if-match']?.replaceAll('"', '');
    final now = records.containsKey(id) ? _token(id) : '0';
    if (expected != null && expected != now) {
      return _json({'changed': now}, 409);
    }
    if (hold case final weakened? when request.method == 'PUT') {
      if (request.url.queryParameters['confirm'] != 'true') {
        return _json({'applied': false, 'weakened': weakened});
      }
    }
    if (refuse case final reason?) {
      refuse = null;
      return _json({'refused': reason}, 422);
    }
    if (request.method == 'DELETE') {
      if (records.remove(id) == null) return http.Response('', 404);
    } else {
      records[id] = (jsonDecode(request.body) as Map).cast<String, dynamic>();
    }
    revision += 1;
    return _json({'revision': '$revision'});
  }

  http.Request get lastWrite =>
      requests.lastWhere((request) => request.method != 'GET');
}

/// What the Stations report they have loaded, as `GET /v1/user/providers` merges it.
const _reported = <String, Object?>{
  'hosts': ['station-a', 'station-b'],
  'differs': <String>[],
  'providers': [
    {
      'id': 'claude',
      'name': 'Claude Code',
      'kinds': ['ai6_agent_handler'],
      'hosts': ['station-a', 'station-b'],
      'schema': {
        'type': 'object',
        'properties': {
          'allowed_tools': {
            'type': 'array',
            'items': {'type': 'string'},
            'title': 'Allowed tools',
            'description': 'Tools the Agent may call.',
            'x-section': 'agent',
          },
          'program': {
            'type': 'string',
            'title': 'Program',
            'description': 'The command that starts it.',
            'x-section': 'station',
          },
        },
      },
    },
    {
      'id': 'codex',
      'name': 'Codex',
      'kinds': ['ai6_agent_handler'],
      'hosts': ['station-a'],
    },
    {
      'id': 'ssh',
      'name': 'SSH',
      'kinds': ['ai6_transport'],
      'hosts': ['station-a'],
      'schema': {
        'type': 'object',
        'properties': {
          'keepalive_secs': {
            'type': 'integer',
            'title': 'Keepalive',
            'description': 'Seconds between probes.',
          },
        },
      },
    },
    {
      'id': 'qemu',
      'name': 'QEMU',
      'kinds': ['ai6_transport'],
      'hosts': ['station-a'],
      'schema': {
        'type': 'object',
        'properties': {
          'image': {
            'type': 'string',
            'title': 'Disk image',
            'description': 'The image the machine boots.',
          },
        },
      },
    },
    {
      'id': 'file',
      'name': 'File',
      'kinds': ['ai6_secret'],
      'hosts': ['station-a'],
      'schema': {
        'type': 'object',
        'properties': {
          'root': {
            'type': 'string',
            'title': 'Root directory',
            'description': 'Where the secret files are.',
          },
        },
      },
    },
    {
      'id': 'github',
      'name': 'GitHub',
      'kinds': ['ai6_source'],
      'hosts': ['station-a'],
      'schema': {
        'type': 'object',
        'properties': {
          'repo': {
            'type': 'string',
            'title': 'Repository',
            'description': 'owner/name.',
          },
        },
      },
    },
    {
      'id': 'git',
      'name': 'git',
      'kinds': ['ai6_safehouse'],
      'hosts': ['station-a'],
    },
    {
      'id': 'copy',
      'name': 'copy',
      'kinds': ['ai6_safehouse'],
      'hosts': ['station-a'],
    },
  ],
};

const _allFamilies = <String, List<String>>{
  'charges': ['config.read', 'config.manage'],
  'theaters': ['config.read', 'config.manage'],
  'loadouts': ['config.read', 'config.manage'],
  'sandboxes': ['config.read', 'config.manage'],
  'mcps': ['config.read', 'config.manage'],
  'overlays': ['config.read', 'config.manage'],
  'secrets': ['config.read', 'config.manage'],
  'secret_providers': ['config.read', 'config.manage'],
  'pools': ['config.read', 'config.manage'],
  'plugins': ['config.read', 'config.manage'],
  'tls': ['config.read', 'config.manage'],
  'cables': ['config.read', 'config.manage'],
  'station': ['config.read', 'config.manage'],
  'unex': ['config.read', 'config.manage'],
};

MissionBoard _signedIn(
  FakeRoster roster, {
  List<String> actions = const ['roster.read', 'roster.manage'],
  Map<String, List<String>> config = _allFamilies,
  Map<String, Object?> providers = _reported,
}) {
  final board = MissionBoard(
    apiOrigin: Uri.parse('https://oo.example'),
    pollInterval: null,
    client: MockClient((request) async {
      if (request.url.path.startsWith('/v1/user/roster/')) {
        return roster.answer(request);
      }
      if (request.url.path == '/v1/user/providers') return _json(providers);
      if (request.url.path.endsWith('/agents')) return _json(const []);
      return http.Response('[]', 200);
    }),
  );
  board.accessToken = 'operator-token';
  board.loading = false;
  board.userSession = UserSessionSummary(
    personId: 'operator-007',
    name: 'operator-007',
    displayName: null,
    groups: const [],
    expiresAt: 1e12,
    operations: const [
      UserOperationAccess(
        operationId: 'alpha',
        actions: ['mission.read', 'agent.read'],
      ),
    ],
    roster: actions,
    config: config,
  );
  board.stationBoard = StationBoardSummary.fromJson({
    'headquarters_station': 'station-a',
    'stations': [
      {
        'station_id': 'station-a',
        'healthy': true,
        'headquarters': true,
        'seen': 1000,
        'roles': ['control', 'execution'],
        'running': 0,
        'capacity': 4,
        'operations': <String>[],
        'handlers': ['claude', 'codex'],
        'config': {'revision': '13'},
      },
      {
        'station_id': 'station-b',
        'healthy': true,
        'headquarters': false,
        'seen': 1000,
        'roles': ['execution'],
        'running': 0,
        'capacity': 4,
        'operations': <String>[],
        'config': {
          'revision': '12',
          'needs_restart': ['theaters.toml'],
        },
      },
    ],
  });
  return board;
}

Future<void> _open(
  WidgetTester tester,
  MissionBoard board,
  String page, {
  Size size = const Size(1000, 1600),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(board.dispose);
  await tester.pumpWidget(OuterOfficeApp(board: board, desktopWindows: false));
  await openPage(tester, page);
  await tester.pumpAndSettle();
  // The roster opens on All records; these cases edit its Agents.
  if (page == 'Roster') await _rosterKind(tester, 'Agents');
}

Future<void> _rosterKind(WidgetTester tester, String label) async {
  await tester.tap(find.byType(DropdownButton<String>).first);
  await tester.pumpAndSettle();
  await tester.tap(find.text(label).last);
  await tester.pumpAndSettle();
}

Finder _field(String path) => find.byKey(Key('form-field-$path'));

/// The text box at [path], whatever wraps it.
Finder _box(String path) => find.descendant(
  of: _field(path),
  matching: find.byType(EditableText),
  matchRoot: true,
);

/// The menu of a closed choice at [path].
Finder _menu(String path) => _field(path);

Future<void> _type(WidgetTester tester, String path, String text) async {
  await tester.ensureVisible(_field(path));
  await tester.pumpAndSettle();
  await tester.enterText(_box(path), text);
  await tester.pumpAndSettle();
}

const _kinds = {
  'operations': ('Operations', 'path', '/srv/alpha'),
  'agents': ('Agents', 'description', 'A case officer'),
  'roles': ('Agent roles', 'purpose', 'Runs cases'),
  'covers': ('Cover policies', 'description', 'Named'),
  'assistant': ("Moneypenny's settings", 'personal_repo', '/srv/{person}.git'),
  'charges': ('Charges', 'agent', 'M-00'),
  'theaters': ('Theaters', 'workdir', '/work'),
  'sandboxes': ('Sandbox profiles', 'backend', 'bubblewrap'),
  'overlays': ('Overlays', 'path', '/overlays/house'),
  'secrets': ('Secrets', 'env', 'TOKEN'),
};

void main() {
  group('the model', () {
    test('every kind of record fits its form and lays out', () {
      for (final kind in [..._kinds.keys, 'roster']) {
        final model = _model(kind);
        expect(model.layout(), isNotEmpty, reason: kind);
        model.dispose();
      }
    });

    test('a record reads back as it was, each value its own kind', () {
      const operation = <String, Object?>{
        'path': '/srv/alpha',
        'inherit': false,
        'roster': ['007', '006'],
        'clearance': '00',
        'station_labels': {'os': 'linux', 'a.b': 'dotted'},
        'pools': <String>[],
        'limit_fallback_secs': 60,
        'workspace': {
          'settings': {'branch': '{id}', 'isolate': false},
        },
        'budget': {'run_tokens': 10, 'mission_cost_usd': 1.5},
        'cover_identity': {'name': 'Field', 'email': 'f@example.invalid'},
        'green_ink': {
          'unattended': {
            '00': ['push'],
          },
        },
        'vetting': {'max_per_hour': 5, 'screening': 'all'},
        'sources': {
          'tracker': {'kind': 'github', 'every': 60},
        },
        'sleepers': {
          'round': {
            'schedule': '0 6 * * 1',
            'title': 'Weekly',
            'priority': 2,
            'each_station': true,
          },
        },
      };
      final model = _model('operations', operation);
      addTearDown(model.dispose);
      expect(model.value(), operation);

      final words = _model('operations', {'vetting': 'off'});
      addTearDown(words.dispose);
      expect(words.value(), {'vetting': 'off'});

      const agent = <String, Object?>{
        'handler_settings': {
          'command': 'agent',
          'args': ['--acp'],
          'request_timeout_ms': 5,
          'quiet': true,
        },
        'limits': {'stall_timeout': 45.5},
        'env': {'LANG': 'C'},
        'priority': 5,
      };
      final agents = _model('agents', agent);
      addTearDown(agents.dispose);
      expect(agents.value(), agent);

      const assistant = <String, Object?>{
        'limits': {'message_bytes': 'none', 'events': 3},
        'desk': {'idle': 'none', 'job_wait': 0.5},
        'views': {'blocks': 1},
        'shared': {
          'ops': {'repo': '/srv/ops.git', 'write': true},
        },
      };
      final settings = _model('assistant', assistant);
      addTearDown(settings.dispose);
      expect(settings.value(), assistant);
    });

    test('an entry of a map of groups lays out by its own form', () {
      final model = _model('operations', {
        'sleepers': {
          'round': {'schedule': '0 6 * * 1', 'title': 'Weekly'},
        },
      });
      addTearDown(model.dispose);
      expect(model.mapKeys('sleepers'), ['round']);
      const entry = 'sleepers["round"]';
      expect(model.layout(entry).single.fields, contains('schedule'));
      expect(model.hints(entry, 'schedule').placeholder, '0 6 * * 1');
      expect(model.hints(entry, 'prompt').widget, 'textarea');
      expect(model.isVisible(entry, 'prompt'), isTrue);
      expect(model.text('$entry.title'), 'Weekly');
    });

    test(
      'a list emptied on purpose stays empty, and one left alone stays absent',
      () {
        final model = _model('operations', {
          'exposure': {
            'restrict': ['cables'],
          },
        });
        addTearDown(model.dispose);
        model.removeFromList('exposure.restrict', 0);
        expect(model.value(), {
          'exposure': {'restrict': <Object?>[]},
        });
      },
    );

    test('a bound stored as null keeps meaning no bound', () {
      final model = _model('assistant', {
        'desk': {'live': null},
      });
      addTearDown(model.dispose);
      expect(model.value(), {
        'desk': {'live': 'none'},
      });
    });

    test('what will not read as its kind is named, by its path', () {
      final model = _model('agents');
      addTearDown(model.dispose);
      model.setText('priority', 'soon');
      model.setText('budget.run_tokens', '1.5');
      final issues = <String, String>{};
      model.value(issues: issues);
      expect(issues.keys, unorderedEquals(['priority', 'budget.run_tokens']));
    });

    test('a field shows by what the form says, and hiding reads values', () {
      final model = _model('operations');
      addTearDown(model.dispose);
      expect(model.isVisible('harness', 'pin'), isFalse);
      model.setText('harness.project_settings', 'pinned');
      expect(model.isVisible('harness', 'pin'), isTrue);
    });

    test('a refusal is matched to the longest field it names', () {
      final model = _model('operations');
      addTearDown(model.dispose);
      expect(
        model.fieldNamedIn('Operation "x": workspace.source must be'),
        'workspace.source',
      );
      expect(
        model.fieldNamedIn('roster revision 3: clearance is not one of'),
        'clearance',
      );
      expect(model.fieldNamedIn('the database is on fire'), isNull);
    });

    test('summaries use titles and labels, never the file format', () {
      expect(
        summaryOf(_schema('agents'), {
          'handler': 'claude',
          'clearance': '00',
          'roles': ['case-officer', 'desk'],
          'limits': {'history': 3},
        }),
        [
          'Handler: claude',
          'Roles: case-officer, desk',
          'Clearance: 00: acts unattended where the Operation allows',
        ],
      );
    });

    test('the renderer knows no kind of record', () {
      for (final name in ['form_host.dart']) {
        final source = File('lib/forms/$name').readAsStringSync();
        expect(
          RegExp(
            'Agent|Operation|Moneypenny|Mission|Station|roster',
            caseSensitive: false,
          ).hasMatch(source),
          isFalse,
          reason: name,
        );
      }
    });
  });

  group('the form', () {
    testWidgets('a secret is masked and never put back in its box', (
      tester,
    ) async {
      final model = FormModel(
        schema: const {
          'type': 'object',
          'properties': {
            'token': {
              'type': 'string',
              'title': 'Token',
              'writeOnly': true,
              'x-sensitive': true,
            },
          },
        },
        form: const {},
        value: const {'token': 'hunter2'},
      );
      addTearDown(model.dispose);
      await tester.pumpWidget(
        MaterialApp(
          localizationsDelegates: const [flutterMaterialLocalizations],
          home: Scaffold(
            body: SingleChildScrollView(child: OoFormView(model: model)),
          ),
        ),
      );
      final box = tester.widget<flutter.TextField>(_field('token'));
      expect(box.obscureText, isTrue);
      expect(box.controller!.text, isEmpty);
      expect(find.text('hunter2'), findsNothing);
      expect(find.text('Set; type to replace it'), findsOneWidget);
      expect(model.value(), {'token': 'hunter2'}, reason: 'kept, not lost');
      await tester.enterText(_field('token'), 'new');
      expect(model.value(), {'token': 'new'});
      await tester.enterText(_field('token'), '');
      expect(model.value(), {'token': 'hunter2'});
    });

    testWidgets('a number shows its unit and what it comes to, a bound can be '
        'lifted, and a choice offers its labels', (tester) async {
      final model = _model('assistant');
      addTearDown(model.dispose);
      await tester.binding.setSurfaceSize(const Size(800, 1600));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await tester.pumpWidget(
        MaterialApp(
          localizationsDelegates: const [flutterMaterialLocalizations],
          home: Scaffold(
            body: SingleChildScrollView(child: OoFormView(model: model)),
          ),
        ),
      );
      expect(
        find.textContaining('Limited: only the hosts listed'),
        findsOneWidget,
      );
      await tester.tap(find.text('Limits'));
      await tester.pumpAndSettle();
      await tester.ensureVisible(find.text('Her desk on each Station'));
      await tester.tap(find.text('Her desk on each Station'));
      await tester.pumpAndSettle();
      await tester.enterText(_box('desk.idle'), '3600');
      await tester.pumpAndSettle();
      expect(find.text('seconds'), findsWidgets);
      await tester.ensureVisible(find.byKey(const Key('form-word-desk.live')));
      await tester.tap(find.byKey(const Key('form-word-desk.live')));
      await tester.pumpAndSettle();
      expect(model.value(), {
        'desk': {'idle': 3600, 'live': 'none'},
      });
    });

    testWidgets('the roster settings say how often limited accounts are read '
        'again', (tester) async {
      final model = _model('roster');
      addTearDown(model.dispose);
      await tester.binding.setSurfaceSize(const Size(800, 1600));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await tester.pumpWidget(
        MaterialApp(
          localizationsDelegates: const [flutterMaterialLocalizations],
          home: Scaffold(
            body: SingleChildScrollView(child: OoFormView(model: model)),
          ),
        ),
      );
      expect(
        find.text('Re-read a limited account\'s usage every'),
        findsOneWidget,
      );
      expect(
        find.text('Back off re-reads of a used-up account by'),
        findsOneWidget,
      );
      expect(find.text('Longest wait between re-reads'), findsOneWidget);
      await tester.ensureVisible(_field('usage.refresh_every'));
      await tester.enterText(_box('usage.refresh_every'), '120');
      await tester.pumpAndSettle();
      expect(model.value(), {
        'usage': {'refresh_every': 120},
      });
    });

    testWidgets('a field waits on another: choosing it shows and hides', (
      tester,
    ) async {
      final model = _model('assistant');
      addTearDown(model.dispose);
      await tester.binding.setSurfaceSize(const Size(800, 1600));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await tester.pumpWidget(
        MaterialApp(
          localizationsDelegates: const [flutterMaterialLocalizations],
          home: Scaffold(
            body: SingleChildScrollView(child: OoFormView(model: model)),
          ),
        ),
      );
      expect(_field('allowed_hosts'), findsOneWidget);
      expect(_field('denied_networks'), findsNothing);
      await tester.tap(_field('network'));
      await tester.pumpAndSettle();
      await tester.tap(
        find.text('Open: all the Station reaches, and web search').last,
      );
      await tester.pumpAndSettle();
      expect(_field('allowed_hosts'), findsNothing);
      expect(_field('denied_networks'), findsOneWidget);
    });
  });

  group('a field that picks a provider', () {
    Future<void> choose(WidgetTester tester, String path, String label) async {
      await tester.ensureVisible(_field(path));
      await tester.tap(_menu(path));
      await tester.pumpAndSettle();
      await tester.tap(find.text(label).last);
      await tester.pumpAndSettle();
    }

    testWidgets('offers the providers the Stations report and draws the '
        'chosen one\'s own settings', (tester) async {
      final roster = FakeRoster();
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('record-form-key')), '007');

      await tester.ensureVisible(_field('handler'));
      await tester.tap(_menu('handler'));
      await tester.pumpAndSettle();
      expect(find.text('Claude Code (claude)'), findsOneWidget);
      expect(find.text('Codex (codex)'), findsOneWidget);
      expect(find.text('git (git)'), findsNothing, reason: 'another kind');
      await tester.tap(find.text('Claude Code (claude)'));
      await tester.pumpAndSettle();

      // Claude's form: only what an Agent may set, not the Station's program.
      final settings = find.byKey(
        const Key('provider-settings-handler_settings'),
      );
      await tester.ensureVisible(settings);
      expect(
        find.descendant(of: settings, matching: find.text('Allowed tools')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: settings, matching: find.text('Program')),
        findsNothing,
      );

      // Another handler draws its own form: codex takes none.
      await choose(tester, 'handler', 'Codex (codex)');
      expect(
        find.descendant(of: settings, matching: find.text('Allowed tools')),
        findsNothing,
      );
      expect(find.text('codex takes no settings.'), findsOneWidget);

      await choose(tester, 'handler', 'Claude Code (claude)');
      await tester.enterText(
        find.descendant(
          of: find.descendant(
            of: settings,
            matching: find.byKey(const Key('form-field-allowed_tools')),
          ),
          matching: find.byType(EditableText),
        ),
        'Read',
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      final saved = roster.records['agents/007']!;
      expect(saved['handler'], 'claude');
      expect(saved.keys, isNot(contains('program')));
    });

    testWidgets('a Theater reached by a transport draws that transport\'s '
        'settings, and keeps what the form does not draw', (tester) async {
      final roster = FakeRoster(
        records: {
          'theaters/box': {
            'access': {
              'transport': {
                'provider': 'ssh',
                'config': {'keepalive_secs': 45},
              },
            },
            'runner': {'address': 'keep-me'},
          },
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Theaters');
      await tester.tap(find.byTooltip('Edit box'));
      await tester.pumpAndSettle();
      final settings = find.byKey(
        const Key('provider-settings-access.transport.config'),
      );
      await tester.ensureVisible(settings);
      expect(
        find.descendant(of: settings, matching: find.text('Keepalive')),
        findsOneWidget,
      );
      await choose(tester, 'access.transport.provider', 'QEMU (qemu)');
      expect(
        find.descendant(of: settings, matching: find.text('Keepalive')),
        findsNothing,
      );
      expect(
        find.descendant(of: settings, matching: find.text('Disk image')),
        findsOneWidget,
      );
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      final saved = roster.records['theaters/box']!;
      expect((saved['access'] as Map)['transport'], {
        'provider': 'qemu',
      }, reason: 'ssh\'s settings are not qemu\'s');
      expect(saved['runner'], {'address': 'keep-me'});
    });

    testWidgets('a route inside a list draws its own handler\'s settings', (
      tester,
    ) async {
      final roster = FakeRoster(
        records: {
          'agents/007': {
            'route': [
              {
                'handler': 'claude',
                'handler_settings': {
                  'allowed_tools': ['Read'],
                },
              },
              {'handler': 'codex'},
            ],
          },
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      final first = find.byKey(
        const Key('provider-settings-route.0.handler_settings'),
      );
      await tester.ensureVisible(first);
      expect(
        find.descendant(of: first, matching: find.text('Allowed tools')),
        findsOneWidget,
      );
      expect(
        find.byKey(const Key('provider-settings-route.1.handler_settings')),
        findsOneWidget,
      );
      expect(find.text('codex takes no settings.'), findsOneWidget);
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      final saved = roster.records['agents/007']!;
      expect(((saved['route'] as List).first as Map)['handler_settings'], {
        'allowed_tools': ['Read'],
      });
    });

    testWidgets('a trust-root family shows what a change loosens and saves it '
        'when confirmed', (tester) async {
      final roster = FakeRoster()
        ..hold = ['an inline credential in a provider'];
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Universal Exports settings');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      await _type(tester, 'directory', 'people');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('record-form-held')), findsOneWidget);
      expect(find.textContaining('an inline credential'), findsOneWidget);
      expect(roster.records.containsKey('unex/settings'), isFalse);
      await tester.ensureVisible(find.byKey(const Key('record-form-confirm')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('record-form-confirm')));
      await tester.pumpAndSettle();
      expect(roster.records['unex/settings']!['directory'], 'people');
      expect(roster.lastWrite.url.queryParameters['confirm'], 'true');
    });

    testWidgets('lets the ID be typed when no host reports a catalog', (
      tester,
    ) async {
      final roster = FakeRoster();
      final board = _signedIn(
        roster,
        providers: const {'providers': <Object?>[], 'hosts': <String>[]},
      );
      await _open(tester, board, 'Roster');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('record-form-key')), '007');
      await _type(tester, 'handler', 'claude');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['agents/007'], {'handler': 'claude'});
    });

    testWidgets('a secret provider\'s settings sit beside its name', (
      tester,
    ) async {
      final roster = FakeRoster(
        records: {
          'secret_providers/vault': {
            'provider': 'file',
            'root': '/run/secrets',
            'other': 'kept',
          },
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Secret providers');
      await tester.tap(find.byTooltip('Edit vault'));
      await tester.pumpAndSettle();
      final settings = find.byKey(const Key('provider-settings-settings'));
      await tester.ensureVisible(settings);
      expect(
        find.descendant(of: settings, matching: find.text('Root directory')),
        findsOneWidget,
      );
      await tester.enterText(
        find.descendant(
          of: find.descendant(
            of: settings,
            matching: find.byKey(const Key('form-field-root')),
          ),
          matching: find.byType(EditableText),
        ),
        '/run/other',
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['secret_providers/vault'], {
        'provider': 'file',
        'root': '/run/other',
        'other': 'kept',
      });
    });
  });

  group('on the Roster page', () {
    for (final MapEntry(key: kind, value: (label, path, text))
        in _kinds.entries) {
      testWidgets('$label: add, edit and remove', (tester) async {
        final roster = FakeRoster();
        final board = _signedIn(roster);
        await _open(tester, board, 'Roster');
        if (kind != 'agents') await _rosterKind(tester, label);

        await tester.tap(find.text('Add'));
        await tester.pumpAndSettle();
        final fixed = kind == 'assistant';
        expect(
          find.byKey(const Key('record-form-key')),
          fixed ? findsNothing : findsOneWidget,
        );
        if (!fixed) {
          await tester.enterText(
            find.byKey(const Key('record-form-key')),
            'new-one',
          );
        }
        await _type(tester, path, text);
        if (kind == 'roles') await _type(tester, 'policy', 'Follow the brief');
        await tester.tap(find.text('Save'));
        await tester.pumpAndSettle();
        final key = fixed ? 'settings' : 'new-one';
        final put = roster.lastWrite;
        expect(put.url.path, '/v1/user/roster/$kind/$key');
        expect(put.headers['if-match'], '"0"', reason: 'it must be new');
        expect(roster.records['$kind/$key']![path], text);
        expect(find.textContaining('roster revision 13'), findsOneWidget);
        expect(find.text(key), findsOneWidget);
        expect(find.textContaining('{"'), findsNothing, reason: 'never JSON');

        await tester.tap(find.byTooltip('Edit $key'));
        await tester.pumpAndSettle();
        expect(find.byKey(const Key('record-form-key')), findsNothing);
        await _type(tester, path, '$text 2');
        await tester.tap(find.text('Save'));
        await tester.pumpAndSettle();
        expect(roster.records['$kind/$key']![path], '$text 2');
        expect(roster.lastWrite.headers['if-match'], isNot('"0"'));

        await tester.tap(find.byTooltip('Remove $key'));
        await tester.pumpAndSettle();
        expect(find.text('Remove $key?'), findsOneWidget);
        await tester.tap(find.widgetWithText(FilledButton, 'Remove'));
        await tester.pumpAndSettle();
        expect(roster.records.containsKey('$kind/$key'), isFalse);
        expect(find.text('No records of this kind.'), findsOneWidget);
      });
    }

    testWidgets('a person without config.read is not offered Charges, and one '
        'with it alone edits none', (tester) async {
      final roster = FakeRoster(
        records: {
          'charges/operation:G': {'agent': 'M-00'},
        },
      );
      final board = _signedIn(roster, config: const {});
      await _open(tester, board, 'Roster');
      await tester.tap(find.byType(DropdownButton<String>).first);
      await tester.pumpAndSettle();
      expect(find.text('Charges'), findsNothing);
      expect(
        roster.requests.where(
          (request) => request.url.path.endsWith('/charges'),
        ),
        isEmpty,
      );
      await tester.tapAt(const Offset(5, 5));
      await tester.pumpAndSettle();

      final reader = _signedIn(
        roster,
        config: const {
          'charges': ['config.read'],
        },
      );
      await _open(tester, reader, 'Roster');
      await _rosterKind(tester, 'Charges');
      expect(find.text('operation:G'), findsOneWidget);
      expect(find.text('Add'), findsNothing);
      expect(find.byTooltip('Edit operation:G'), findsNothing);
    });

    testWidgets('a refusal shows on the field it names', (tester) async {
      final roster = FakeRoster(
        records: {
          'agents/007': {'handler': 'claude'},
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      roster.refuse = 'Agent "007": clearance is not one of desk, field, 00';
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(
        find.descendant(
          of: _field('clearance'),
          matching: find.text(
            'Agent "007": clearance is not one of desk, field, 00',
          ),
        ),
        findsOneWidget,
      );
      expect(find.text('Save'), findsOneWidget, reason: 'it stays open');
    });

    testWidgets('a refusal naming no field shows above the form', (
      tester,
    ) async {
      final roster = FakeRoster();
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('record-form-key')), 'x');
      roster.refuse = 'the roster it would leave has no room';
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(
        find.text('the roster it would leave has no room'),
        findsOneWidget,
      );
    });

    testWidgets('a save over a newer record offers to reload', (tester) async {
      final roster = FakeRoster(
        records: {
          'agents/007': {'description': 'mine'},
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      await _type(tester, 'description', 'yours');
      roster.records['agents/007'] = {'description': 'theirs'};
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(
        find.textContaining('Someone changed this record'),
        findsOneWidget,
      );
      expect(roster.records['agents/007'], {'description': 'theirs'});

      await tester.ensureVisible(find.text('Reload'));
      await tester.tap(find.text('Reload'));
      await tester.pumpAndSettle();
      expect(find.text('theirs'), findsOneWidget);
      await _type(tester, 'description', 'on top of theirs');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['agents/007'], {'description': 'on top of theirs'});
    });

    testWidgets('a removal names what depends on the record', (tester) async {
      final roster = FakeRoster(
        records: {
          'roles/case-officer': {'purpose': 'p', 'policy': 'q'},
          'agents/007': {
            'roles': ['case-officer'],
          },
          'agents/other': {
            'roles': ['else'],
          },
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Agent roles');
      await tester.tap(find.byTooltip('Remove case-officer'));
      await tester.pumpAndSettle();
      expect(find.text('• Agent 007: Roles'), findsOneWidget);
      expect(find.textContaining('Agent other'), findsNothing);
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(roster.records.containsKey('roles/case-officer'), isTrue);
    });

    testWidgets('fields that name something offer it', (tester) async {
      final roster = FakeRoster(
        records: {
          'roles/case-officer': {'purpose': 'p', 'policy': 'q'},
          'agents/007': {'model': 'opus'},
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('record-form-key')), 'new');
      await tester.ensureVisible(_field('roles'));
      await tester.tap(
        find.descendant(
          of: _field('roles'),
          matching: find.text('case-officer'),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(_menu('handler'));
      await tester.pumpAndSettle();
      expect(find.text('Codex (codex)'), findsOneWidget);
      await tester.tap(find.text('Codex (codex)'));
      await tester.pumpAndSettle();
      await _type(tester, 'model', 'a-model-nobody-listed');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['agents/new'], {
        'handler': 'codex',
        'model': 'a-model-nobody-listed',
        'roles': ['case-officer'],
      });
    });

    testWidgets('the configuration can be read, and rows show titled fields', (
      tester,
    ) async {
      final roster = FakeRoster(
        records: {
          'agents/007': {'handler': 'claude', 'priority': 5},
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      expect(find.text('Handler: claude · Priority: 5'), findsOneWidget);
      await tester.tap(find.byTooltip('As configuration: 007'));
      await tester.pumpAndSettle();
      expect(find.textContaining('[agent.007]'), findsOneWidget);
      expect(find.textContaining('handler = '), findsOneWidget);
    });

    testWidgets('it fits a phone', (tester) async {
      final roster = FakeRoster(
        records: {
          'agents/007': {
            'handler': 'claude',
            'env': {'LANG': 'C'},
          },
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster', size: const Size(390, 800));
      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      expect(find.byType(Dialog), findsOneWidget);
      expect(tester.takeException(), isNull);
      for (final title in ['About', 'Limits and spending', 'Retirement']) {
        await tester.scrollUntilVisible(
          find.text(title).last,
          100,
          scrollable: find.byType(Scrollable).last,
        );
        await tester.tap(find.text(title).last);
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull, reason: title);
      }
    });
  });

  group('the last families and the record editor', () {
    testWidgets('the Station form saves the Station record', (tester) async {
      final roster = FakeRoster();
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Station settings');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('record-form-key')), findsNothing);
      await _type(tester, 'max_concurrent_missions', '8');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['station/settings'], {
        'max_concurrent_missions': 8,
      });
    });

    testWidgets('a provider library is pinned under its file name', (
      tester,
    ) async {
      final roster = FakeRoster();
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Provider libraries');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('record-form-key')),
        'ai6_claude.dll',
      );
      final pin = 'sha256:${'ab' * 32}';
      await _type(tester, 'pin', pin);
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['plugins/ai6_claude.dll'], {'pin': pin});
    });

    testWidgets('a TLS change that loosens is held, then confirmed', (
      tester,
    ) async {
      final roster = FakeRoster()
        ..hold = ['tls.allow_resumption = true (default false)'];
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'TLS policy');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      await tester.ensureVisible(_field('allow_resumption'));
      await tester.pumpAndSettle();
      await tester.tap(_field('allow_resumption'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('record-form-held')), findsOneWidget);
      expect(find.textContaining('allow_resumption = true'), findsOneWidget);
      expect(roster.records.containsKey('tls/settings'), isFalse);
      await tester.ensureVisible(find.byKey(const Key('record-form-confirm')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('record-form-confirm')));
      await tester.pumpAndSettle();
      expect(roster.records['tls/settings']!['allow_resumption'], true);
      expect(roster.lastWrite.url.queryParameters['confirm'], 'true');
    });

    testWidgets('a Source is drawn with its provider\'s own form', (
      tester,
    ) async {
      final roster = FakeRoster(
        records: {
          'operations/alpha': {
            'sources': {
              'github': {'repo': 'a/b'},
            },
          },
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Operations');
      await tester.tap(find.byTooltip('Edit alpha'));
      await tester.pumpAndSettle();
      final section = find.byKey(
        const Key('provider-settings-sources.github.settings'),
      );
      await tester.ensureVisible(section);
      await tester.pumpAndSettle();
      expect(section, findsOneWidget);
      expect(find.text('Repository'), findsOneWidget);
      await _type(tester, 'repo', 'c/d');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['operations/alpha'], {
        'sources': {
          'github': {'repo': 'c/d'},
        },
      });
    });

    testWidgets('a Station\'s own records are listed, edited and removed', (
      tester,
    ) async {
      final roster = FakeRoster(
        records: {
          'theaters/lab': {'workdir': '/global'},
          'theaters@station-a/lab': {'workdir': '/own'},
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Theaters');
      expect(find.textContaining('/global'), findsOneWidget);
      await tester.tap(find.byKey(const Key('roster-scope')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Station station-a (1 override)').last);
      await tester.pumpAndSettle();
      expect(find.textContaining('/own'), findsOneWidget);
      expect(find.textContaining('/global'), findsNothing);

      await tester.tap(find.byTooltip('Edit lab'));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('record-form-scope')), findsOneWidget);
      await _type(tester, 'workdir', '/changed');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['theaters@station-a/lab'], {'workdir': '/changed'});
      expect(roster.records['theaters/lab'], {'workdir': '/global'});
      expect(roster.lastWrite.url.queryParameters['station'], 'station-a');

      await tester.tap(find.byTooltip('Remove lab'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Remove'));
      await tester.pumpAndSettle();
      expect(roster.records.containsKey('theaters@station-a/lab'), isFalse);
      expect(roster.records['theaters/lab'], {'workdir': '/global'});
    });

    testWidgets('a refused revision shows on the card and in the editor', (
      tester,
    ) async {
      final roster = FakeRoster();
      final board = _signedIn(roster);
      board.stationBoard = StationBoardSummary.fromJson({
        'headquarters_station': 'station-a',
        'stations': [
          {
            'station_id': 'station-a',
            'healthy': true,
            'headquarters': true,
            'seen': 1000,
            'roles': ['control'],
            'running': 0,
            'capacity': 4,
            'operations': <String>[],
            'config': {
              'revision': '12',
              'refused': ['theaters: pool main names no account'],
            },
          },
        ],
      });
      await _open(tester, board, 'Stations');
      expect(
        find.byKey(const Key('station-config-refused-station-a')),
        findsOneWidget,
      );
      expect(find.textContaining('pool main names no account'), findsWidgets);

      await openPage(tester, 'Roster');
      await tester.pumpAndSettle();
      await _rosterKind(tester, 'Theaters');
      await tester.tap(find.byKey(const Key('roster-scope')));
      await tester.pumpAndSettle();
      await tester.tap(find.textContaining('Station station-a').last);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      expect(
        find.descendant(
          of: find.byType(Dialog),
          matching: find.textContaining('refused: theaters: pool main'),
        ),
        findsOneWidget,
      );
    });

    testWidgets('a weakened Theater shows on its Station\'s card', (
      tester,
    ) async {
      final board = _signedIn(FakeRoster());
      board.stationBoard = StationBoardSummary.fromJson({
        'headquarters_station': 'station-a',
        'stations': [
          {
            'station_id': 'station-a',
            'healthy': true,
            'headquarters': true,
            'seen': 1000,
            'roles': ['control'],
            'running': 0,
            'capacity': 4,
            'operations': <String>[],
            'theaters': [
              {
                'id': 'lab',
                'access': 'ssh',
                'weakened': ['trust_on_first_use = true'],
              },
            ],
          },
        ],
      });
      await _open(tester, board, 'Stations');
      expect(
        find.byKey(const Key('station-posture-station-a')),
        findsOneWidget,
      );
      expect(
        find.textContaining('lab: trust_on_first_use = true'),
        findsWidgets,
      );
    });

    testWidgets('a search narrows the form to the settings it names', (
      tester,
    ) async {
      final board = _signedIn(
        FakeRoster(
          records: {
            'agents/007': {'handler': 'claude'},
          },
        ),
      );
      await _open(tester, board, 'Roster');
      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      expect(_field('handler'), findsOneWidget);
      expect(_field('description'), findsOneWidget);
      await tester.enterText(find.byKey(const Key('form-filter')), 'xyzzy');
      await tester.pumpAndSettle();
      expect(_field('handler'), findsNothing);
      expect(_field('description'), findsNothing);
      await tester.enterText(find.byKey(const Key('form-filter')), 'handler');
      await tester.pumpAndSettle();
      expect(_field('handler'), findsOneWidget);
      expect(_field('description'), findsNothing);
    });

    testWidgets('modified only counts, lists and resets to the default', (
      tester,
    ) async {
      final board = _signedIn(
        FakeRoster(
          records: {
            'agents/007': {'handler': 'claude'},
          },
        ),
      );
      await _open(tester, board, 'Roster');
      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      expect(find.text('Modified only (1)'), findsOneWidget);
      await tester.tap(find.byKey(const Key('form-modified-only')));
      await tester.pumpAndSettle();
      expect(_field('handler'), findsOneWidget);
      expect(_field('description'), findsNothing);
      expect(find.byKey(const Key('form-modified-handler')), findsOneWidget);
      await tester.tap(find.byKey(const Key('form-reset-handler')));
      await tester.pumpAndSettle();
      expect(find.text('Modified only (0)'), findsOneWidget);
      expect(find.text('Every setting is at its default.'), findsOneWidget);
      expect(_field('handler'), findsNothing);
    });

    testWidgets('a section folds away and counts what it holds modified', (
      tester,
    ) async {
      final board = _signedIn(
        FakeRoster(
          records: {
            'agents/007': {'handler': 'claude'},
          },
        ),
      );
      await _open(tester, board, 'Roster');
      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      final chip = find.byWidgetPredicate(
        (widget) =>
            widget is ActionChip &&
            widget.key.toString().contains('form-section-chip-') &&
            (widget.label as Text).data!.contains('1 modified'),
      );
      expect(chip, findsOneWidget);
      expect(_field('handler'), findsOneWidget);
      await tester.tap(chip);
      await tester.pumpAndSettle();
      expect(_field('handler'), findsNothing);
      await tester.tap(chip);
      await tester.pumpAndSettle();
      expect(_field('handler'), findsOneWidget);
    });

    testWidgets('the changes are read key by key before saving', (
      tester,
    ) async {
      final board = _signedIn(
        FakeRoster(
          records: {
            'agents/007': {'handler': 'claude'},
          },
        ),
      );
      await _open(tester, board, 'Roster');
      await tester.tap(find.byTooltip('Edit 007'));
      await tester.pumpAndSettle();
      await _type(tester, 'description', 'x');
      await tester.ensureVisible(find.byKey(const Key('record-changes')));
      await tester.tap(find.text('Review changes (1)'));
      await tester.pumpAndSettle();
      expect(find.text('+ description = "x"'), findsOneWidget);
      await tester.tap(find.byKey(const Key('record-changes-raw')));
      await tester.pumpAndSettle();
      expect(find.textContaining('description = "x"'), findsOneWidget);
    });

    testWidgets('a reference field is searched by what is typed', (
      tester,
    ) async {
      final board = _signedIn(
        FakeRoster(
          records: {
            'sandboxes/strict': {'backend': 'bubblewrap'},
            'sandboxes/loose': {'backend': 'bubblewrap'},
          },
        ),
      );
      await _open(tester, board, 'Roster');
      await _rosterKind(tester, 'Loadouts');
      await tester.tap(find.text('Add'));
      await tester.pumpAndSettle();
      await tester.ensureVisible(_field('sandbox'));
      await tester.pumpAndSettle();
      await tester.enterText(_box('sandbox'), 'str');
      await tester.pumpAndSettle();
      expect(find.text('strict'), findsOneWidget);
      expect(find.text('loose'), findsNothing);
    });
  });

  testWidgets(
    'the preview summarizes its sample records and offers no change',
    (tester) async {
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        demo: true,
        pollInterval: null,
      );
      // The asset arrives on the real clock, which pumping does not advance.
      await tester.runAsync(() => board.loadRecordForm('agents'));
      await _open(tester, board, 'Roster');
      await tester.pumpAndSettle();
      expect(find.textContaining('Handler: claude'), findsOneWidget);
      expect(find.text('Add'), findsNothing);
      expect(find.byTooltip('Edit 007'), findsNothing);
    },
  );

  group('on their own pages', () {
    testWidgets('an Agent is added, edited and removed from Agents', (
      tester,
    ) async {
      final roster = FakeRoster(
        records: {
          'agents/007': {'handler': 'claude'},
        },
      );
      final board = _signedIn(roster);
      await _open(tester, board, 'Agents');
      await tester.tap(find.text('Add Agent'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('record-form-key')), '009');
      await _type(tester, 'description', 'A new one');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['agents/009'], {'description': 'A new one'});
      expect(
        find.textContaining('Saved as roster revision 13'),
        findsOneWidget,
      );
      expect(find.text('Running it: station-a'), findsOneWidget);
      expect(find.text('Not yet: station-b'), findsOneWidget);
      expect(
        find.text('station-b: theaters.toml changed; restart it to apply'),
        findsOneWidget,
      );
    });

    testWidgets('an Operation is added from Operations, first what a first one '
        'needs and the rest folded', (tester) async {
      final roster = FakeRoster();
      final board = _signedIn(roster);
      await _open(tester, board, 'Operations');
      await tester.tap(find.text('Add Operation'));
      await tester.pumpAndSettle();
      expect(_field('path'), findsOneWidget);
      expect(_field('roster'), findsOneWidget);
      expect(
        _field('limit_fallback_secs'),
        findsNothing,
        reason: 'the rest is folded',
      );
      await tester.enterText(find.byKey(const Key('record-form-key')), 'NEW');
      await _type(tester, 'path', '/srv/new');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(roster.records['operations/NEW'], {'path': '/srv/new'});
      expect(
        find.textContaining('Saved as roster revision 13'),
        findsOneWidget,
      );

      await tester.tap(find.byTooltip('Edit alpha'));
      await tester.pumpAndSettle();
      expect(
        find.textContaining('alpha is no longer in the roster'),
        findsOneWidget,
        reason: 'the page shows Operations the roster here does not hold',
      );
    });

    testWidgets('without roster.manage the pages offer no change', (
      tester,
    ) async {
      final board = _signedIn(FakeRoster(), actions: const ['roster.read']);
      await _open(tester, board, 'Operations');
      expect(find.text('Add Operation'), findsNothing);
      expect(find.byTooltip('Edit alpha'), findsNothing);
    });

    testWidgets('a roster that takes no edits offers none either', (
      tester,
    ) async {
      final board = _signedIn(FakeRoster(writable: false));
      await _open(tester, board, 'Operations');
      await tester.pumpAndSettle();
      expect(find.text('Add Operation'), findsNothing);
      expect(find.byTooltip('Remove alpha'), findsNothing);
    });
  });
}
