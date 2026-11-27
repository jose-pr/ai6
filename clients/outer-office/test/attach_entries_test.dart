import 'package:flutter_test/flutter_test.dart';
import 'package:outer_office/charges/attach_entries.dart';
import 'package:outer_office/charges/attachment.dart';

Attachment _attachment(List<AttachFrame> frames) {
  final attachment = Attachment(
    token: 't',
    target: const ChargeTarget('m'),
    connect: () => throw StateError('not connected'),
  );
  attachment.frames.addAll(frames);
  return attachment;
}

AttachFrame _frame(int seq, String kind, Map<String, Object?> data) =>
    AttachFrame(seq: seq, at: seq.toDouble(), kind: kind, data: data);

AttachFrame _text(int seq, String text, [String kind = 'text']) =>
    _frame(seq, 'event', {'kind': kind, 'text': text});

void main() {
  test('the pieces of one answer are one entry', () {
    final entries = attachEntries(
      _attachment([
        _frame(1, 'prompt', {'from': 'eve', 'text': 'Hi'}),
        _frame(2, 'turn', {'state': 'begun', 'by': 'eve'}),
        _text(3, 'Hel'),
        _text(4, 'lo, '),
        _text(5, 'there.'),
        _frame(6, 'turn', {'state': 'ended', 'by': 'eve'}),
      ]),
    );
    expect(
      [for (final e in entries) (e.role, e.text)],
      [('person', 'Hi'), ('agent', 'Hello, there.')],
    );
    expect(
      entries[1].at,
      3,
      reason: 'the entry keeps the time its first piece came',
    );
  });

  test('a tool, a turn or another speaker ends the entry', () {
    final entries = attachEntries(
      _attachment([
        _text(1, 'One '),
        _text(2, 'thing.'),
        _frame(3, 'event', {
          'kind': 'tool',
          'tools': ['list_missions'],
        }),
        _text(4, 'Then '),
        _text(5, 'another.'),
        _frame(6, 'turn', {'state': 'ended', 'by': 'eve'}),
        _frame(7, 'turn', {'state': 'begun', 'by': 'eve'}),
        _text(8, 'A new turn.'),
        _text(9, 'A plan', 'plan'),
        _text(10, 'Done', 'completed'),
      ]),
    );
    expect(
      [for (final e in entries) (e.role, e.text)],
      [
        ('agent', 'One thing.'),
        ('tools', 'list_missions'),
        ('agent', 'Then another.'),
        ('agent', 'A new turn.'),
        ('agent', 'A plan'),
        ('notice', 'Done'),
      ],
    );
  });
}
