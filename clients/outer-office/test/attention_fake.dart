/// The gateway's attention summary as a fake serves it: [items] and [coverage] in the wire
/// shape of `GET /v1/user/attention`.
Map<String, Object?> attentionBody({
  List<Map<String, Object?>> items = const [],
  List<Map<String, Object?>> coverage = const [],
  double checkedAt = 1_900_000_000,
}) => {
  'checked_at': checkedAt,
  'scope': {'kind': 'all', 'label': 'All accessible Operations'},
  'items': items,
  'omitted': 0,
  'coverage': coverage,
  'complete': !coverage.any((read) => (read['unavailable'] as List).isNotEmpty),
  'headline': '',
};

/// One item of the summary.
Map<String, Object?> attentionEntry({
  required String id,
  required String kind,
  required String title,
  required Map<String, Object?> route,
  String condition = 'current',
  String scope = 'operation:operation-alpha',
  String? detail,
  double? since,
  double checkedAt = 1_900_000_000,
}) => {
  'id': id,
  'kind': kind,
  'condition': condition,
  'title': title,
  'detail': ?detail,
  'entity': {'kind': 'brief', 'id': id},
  'route': route,
  'scope': scope,
  'checked_at': checkedAt,
  'since': ?since,
};

/// A source read as [status] (`checked`, `not_permitted` or `unavailable`) for one scope.
Map<String, Object?> attentionRead(
  String source,
  String label,
  String status, {
  String scope = 'operation:operation-alpha',
}) => {
  'source': source,
  'label': label,
  'status': status,
  'checked': status == 'checked' ? [scope] : <String>[],
  'not_permitted': status == 'not_permitted' ? [scope] : <String>[],
  'unavailable': status == 'unavailable' ? [scope] : <String>[],
  'checked_at': 1_900_000_000.0,
};
