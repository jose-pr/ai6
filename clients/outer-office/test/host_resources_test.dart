import 'package:flutter_test/flutter_test.dart';
import 'package:outer_office/main.dart';

void main() {
  test('a host with no load average, as on Windows, shows no load', () {
    const windows = HostResources(sampledAt: 1000, cpus: 12, cpuPercent: 67.3);
    expect(windows.description, isNot(contains('load')));
    expect(windows.description, contains('67.3% busy'));
  });

  test('a host with a load average shows it', () {
    const linux = HostResources(sampledAt: 1000, load1m: 1.5);
    expect(linux.description, contains('load 1.50'));
  });
}
