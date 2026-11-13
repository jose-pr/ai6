import 'package:flutter/foundation.dart' show immutable;

/// What a Report is tied to, as the gateway says it.
@immutable
class ReportLinkSummary {
  const ReportLinkSummary({required this.kind, required this.id});

  factory ReportLinkSummary.fromJson(Map<String, dynamic> json) =>
      ReportLinkSummary(kind: json['kind'] as String, id: json['id'] as String);

  /// `mission`, `report`, `decision` or `evidence`.
  final String kind;

  /// The Mission, Report or request ID, or what names the evidence.
  final String id;
}

/// How an assessor judged what a Report says, in the probability yardstick's words.
@immutable
class ReportJudgement {
  const ReportJudgement({required this.likelihood, required this.confidence});

  factory ReportJudgement.fromJson(Map<String, dynamic> json) =>
      ReportJudgement(
        likelihood: json['likelihood'] as String,
        confidence: json['confidence'] as String,
      );

  /// A yardstick word such as `likely`, as the gateway spells it (`snake_case`).
  final String likelihood;

  /// `high`, `moderate` or `low`.
  final String confidence;

  /// `likely (55-75%), moderate confidence`: the band is shown beside the words, never in place.
  String get words {
    final band = reportLikelihoods
        .where((entry) => entry.$1 == likelihood)
        .map((entry) => entry.$3)
        .firstOrNull;
    return '${likelihoodWords(likelihood)}'
        '${band == null ? '' : ' ($band)'}, $confidence confidence';
  }
}

/// The yardstick: wire word, its words and its percentage band, lowest first.
const reportLikelihoods = <(String, String, String)>[
  ('remote_chance', 'remote chance', 'under 5%'),
  ('highly_unlikely', 'highly unlikely', '10-20%'),
  ('unlikely', 'unlikely', '25-35%'),
  ('realistic_possibility', 'realistic possibility', '40-50%'),
  ('likely', 'likely', '55-75%'),
  ('highly_likely', 'highly likely', '80-90%'),
  ('almost_certain', 'almost certain', 'over 95%'),
];

/// How sure an assessor is.
const reportConfidences = ['high', 'moderate', 'low'];

/// The yardstick's words for a wire word; the wire word with spaces when it is not one.
String likelihoodWords(String wire) =>
    reportLikelihoods
        .where((entry) => entry.$1 == wire)
        .map((entry) => entry.$2)
        .firstOrNull ??
    wire.replaceAll('_', ' ');

/// What the scope's charge or a person decided to do with a Report.
@immutable
class ReportAssessmentSummary {
  const ReportAssessmentSummary({
    required this.verdict,
    required this.by,
    required this.at,
    this.comment,
    this.judgement,
    this.waitsFor,
    this.until,
  });

  factory ReportAssessmentSummary.fromJson(Map<String, dynamic> json) =>
      ReportAssessmentSummary(
        verdict: json['verdict'] as String,
        by: json['by'] as String,
        at: (json['at'] as num).toDouble(),
        comment: json['comment'] as String?,
        judgement: json['judgement'] == null
            ? null
            : ReportJudgement.fromJson(
                json['judgement'] as Map<String, dynamic>,
              ),
        waitsFor: json['waits_for'] as String?,
        until: (json['until'] as num?)?.toDouble(),
      );

  /// `mission`, `duplicate`, `related`, `no_action`, `awaiting`, `later` or `decision`.
  final String verdict;

  /// A person's ID, or `charge:operation:ID`.
  final String by;
  final double at;

  /// The assessor's own words, kept apart from the Report's.
  final String? comment;
  final ReportJudgement? judgement;

  /// What `awaiting` asked, or the condition `later` waits for.
  final String? waitsFor;

  /// When `awaiting` or `later` brings the Report back, as Unix seconds.
  final double? until;
}

/// How a Report ended.
@immutable
class ReportResolutionSummary {
  const ReportResolutionSummary({
    required this.by,
    required this.at,
    this.note,
    this.outcome,
  });

  factory ReportResolutionSummary.fromJson(Map<String, dynamic> json) =>
      ReportResolutionSummary(
        by: json['by'] as String,
        at: (json['at'] as num).toDouble(),
        note: json['note'] as String?,
        outcome: json['outcome'] as String?,
      );

  final String by;
  final double at;
  final String? note;

  /// How it came out; what a source's reliability is earned from.
  final String? outcome;
}

/// The words for each assessment.
const reportVerdicts = {
  'mission': 'Mission',
  'duplicate': 'Duplicate',
  'related': 'Related',
  'no_action': 'No action',
  'awaiting': 'Awaiting an answer',
  'later': 'Later',
  'decision': 'Decision',
};

/// The words for each way a Report closes.
const reportOutcomes = {
  'mission_done': 'Mission done',
  'mission_failed': 'Mission failed',
  'decision_approved': 'Decision approved',
  'decision_rejected': 'Decision rejected',
  'duplicate': 'Duplicate',
  'related': 'Related',
  'no_action': 'No action',
};

/// A Report as the gateway shows it to this person: whole, its tearline only, or its markings
/// alone, per [sight].
@immutable
class ReportSummary {
  const ReportSummary({
    required this.id,
    required this.scope,
    required this.sight,
    this.classification,
    this.handling,
    this.compartments = const [],
    this.kind,
    this.severity,
    this.state,
    this.priority,
    this.deadline,
    this.credibility,
    this.created,
    this.updated,
    this.untrusted = true,
    this.title,
    this.text,
    this.filer,
    this.source,
    this.reliability,
    this.subSource,
    this.from,
    this.links = const [],
    this.tearline,
    this.assessment,
    this.resolution,
  });

  factory ReportSummary.fromJson(Map<String, dynamic> json) => ReportSummary(
    id: json['id'] as String,
    scope: json['scope'] as String,
    sight: json['sight'] as String? ?? 'content',
    classification: json['classification'] as String?,
    handling: json['handling'] as String?,
    compartments: [
      for (final name in json['compartments'] as List<dynamic>? ?? const [])
        name as String,
    ],
    kind: json['kind'] as String?,
    severity: json['severity'] as String?,
    state: json['state'] as String?,
    priority: json['priority'] as String?,
    deadline: (json['deadline'] as num?)?.toDouble(),
    credibility: (json['credibility'] as num?)?.toInt(),
    created: (json['created'] as num?)?.toDouble(),
    updated: (json['updated'] as num?)?.toDouble(),
    untrusted: json['untrusted'] as bool? ?? true,
    title: json['title'] as String?,
    text: json['text'] as String?,
    filer: json['filer'] as Map<String, dynamic>?,
    source: json['source'] as String?,
    reliability: json['reliability'] as String?,
    subSource: json['sub_source'] as String?,
    from: (json['from'] as num?)?.toDouble(),
    links: [
      for (final link in json['links'] as List<dynamic>? ?? const [])
        ReportLinkSummary.fromJson(link as Map<String, dynamic>),
    ],
    tearline: (json['tearline'] as Map<String, dynamic>?)?['text'] as String?,
    assessment: json['assessment'] == null
        ? null
        : ReportAssessmentSummary.fromJson(
            json['assessment'] as Map<String, dynamic>,
          ),
    resolution: json['resolution'] == null
        ? null
        : ReportResolutionSummary.fromJson(
            json['resolution'] as Map<String, dynamic>,
          ),
  );

  final String id;

  /// `operation:ID`, `station:ID` or `m`.
  final String scope;

  /// `content`, `tearline` or `markings`: how much of it this person sees.
  final String sight;
  final String? classification;
  final String? handling;
  final List<String> compartments;
  final String? kind;
  final String? severity;

  /// `open`, `assessed` or `closed`.
  final String? state;
  final String? priority;
  final double? deadline;

  /// `1` (confirmed by other sources) to `6` (cannot be judged); unset until assessed.
  final int? credibility;
  final double? created;
  final double? updated;

  /// Whether its words came from anything but a person.
  final bool untrusted;
  final String? title;

  /// The filed text; the tearline's, when [sight] is `tearline`.
  final String? text;

  /// `{"type": "agent", "agent", "run", "mission"}`, `source`, `charge` or `person`.
  final Map<String, dynamic>? filer;

  /// The filer's source key, which `GET /v1/reports/sources/{key}` takes.
  final String? source;

  /// The source's reliability letter, `A` to `F`.
  final String? reliability;
  final String? subSource;

  /// When the information is from, as Unix seconds.
  final double? from;
  final List<ReportLinkSummary> links;

  /// What those cleared for the Report's own marking read in its place.
  final String? tearline;
  final ReportAssessmentSummary? assessment;
  final ReportResolutionSummary? resolution;

  /// The Operation ID of an Operation scope; null for a Station's or M's.
  String? get operationId => scope.startsWith('operation:')
      ? scope.substring('operation:'.length)
      : null;

  /// The kind of filer: `source`, `agent`, `charge` or `person`.
  String get origin => filer?['type'] as String? ?? 'unknown';

  /// The Mission an Agent filed it in, when it did.
  String? get filedInMission =>
      origin == 'agent' ? (filer?['mission'] as String?) : null;

  /// The person's ID, when a person filed it.
  String? get filerPerson =>
      origin == 'person' ? (filer?['person'] as String?) : null;

  /// A Source item's Report, which a `mission` assessment queues as a brief.
  bool get isSourceItem => kind == 'source-item';

  bool get isOpen => state == 'open';

  /// The Missions it is tied to.
  Iterable<String> get missions => [
    for (final link in links)
      if (link.kind == 'mission') link.id,
  ];

  /// Whether the person sees all of it.
  bool get whole => sight == 'content';
}

/// What one read of `GET /v1/reports` answered: the Reports and the lists they are marked from.
@immutable
class ReportsRead {
  const ReportsRead({
    required this.reports,
    this.priorities = const [],
    this.classifications = const [],
  });

  factory ReportsRead.fromJson(Map<String, dynamic> json) => ReportsRead(
    reports: [
      for (final report in json['reports'] as List<dynamic>? ?? const [])
        ReportSummary.fromJson(report as Map<String, dynamic>),
    ],
    priorities: [
      for (final name in json['priorities'] as List<dynamic>? ?? const [])
        name as String,
    ],
    classifications: [
      for (final name in json['classifications'] as List<dynamic>? ?? const [])
        name as String,
    ],
  );

  final List<ReportSummary> reports;

  /// The deployment's priorities, most urgent first.
  final List<String> priorities;

  /// Its classifications, lowest first.
  final List<String> classifications;
}

/// Every Report this person reads, and which scopes could not be read.
@immutable
class ReportsListing {
  const ReportsListing({
    required this.reports,
    this.priorities = const [],
    this.classifications = const [],
    this.unread = const {},
    this.refused = 0,
  });

  final List<ReportSummary> reports;
  final List<String> priorities;
  final List<String> classifications;

  /// The scopes that failed, each with why.
  final Map<String, String> unread;

  /// How many scopes the gateway refused (403): the account may not read them, which is no fault.
  final int refused;
}

/// A source's standing, as `GET /v1/reports/sources/{key}` says it.
@immutable
class ReportSourceStanding {
  const ReportSourceStanding({
    required this.source,
    required this.reliability,
    required this.rated,
    required this.counted,
    required this.confirmed,
    required this.minReports,
    required this.counts,
    required this.confirms,
    required this.bands,
    required this.reports,
  });

  factory ReportSourceStanding.fromJson(Map<String, dynamic> json) {
    final rule = json['rule'] as Map<String, dynamic>? ?? const {};
    List<String> words(Object? list) => [
      for (final word in list as List<dynamic>? ?? const []) word as String,
    ];
    return ReportSourceStanding(
      source: json['source'] as String,
      reliability: json['reliability'] as String,
      rated: json['rated'] as bool? ?? false,
      counted: (json['counted'] as num?)?.toInt() ?? 0,
      confirmed: (json['confirmed'] as num?)?.toInt() ?? 0,
      minReports: (rule['min_reports'] as num?)?.toInt() ?? 0,
      counts: words(rule['counts']),
      confirms: words(rule['confirms']),
      bands: [
        for (final band in rule['bands'] as List<dynamic>? ?? const [])
          (band as num).toInt(),
      ],
      reports: [
        for (final report in json['reports'] as List<dynamic>? ?? const [])
          ReportSummary.fromJson(report as Map<String, dynamic>),
      ],
    );
  }

  final String source;

  /// `A` to `F`; `F` until the rule rates it.
  final String reliability;

  /// Whether enough closed Reports are counted for the rule to rate it.
  final bool rated;
  final int counted;
  final int confirmed;

  /// The deployment's rule: counted Reports before a rating, the outcomes that count and that
  /// confirm, and the percent confirmed for `A`, `B`, `C` and `D`.
  final int minReports;
  final List<String> counts;
  final List<String> confirms;
  final List<int> bands;

  /// The closed Reports behind it that this person may read.
  final List<ReportSummary> reports;
}
