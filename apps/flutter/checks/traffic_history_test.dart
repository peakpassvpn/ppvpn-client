import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/traffic_history.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;

void main() {
  test('chart averages ten seconds without changing rate events', () {
    final history = TrafficHistory();
    const zero = rust.UiTraffic(up: 0, down: 0);
    const burst = rust.UiTraffic(up: 100, down: 200);
    expect(history.add(zero, Duration.zero), isEmpty);
    for (var second = 1; second < 10; second++) {
      expect(
        history.add(second <= 5 ? zero : burst, Duration(seconds: second)),
        isEmpty,
      );
    }
    final result = history.add(burst, const Duration(seconds: 10));
    expect((result.single.up, result.single.down), (50, 100));
    expect((burst.up, burst.down), (100, 200));
  });
  test('irregular event timing is weighted and split at bucket boundaries', () {
    final history = TrafficHistory();
    history.add(const rust.UiTraffic(up: 0, down: 0), Duration.zero);
    history.add(
      const rust.UiTraffic(up: 100, down: 0),
      const Duration(milliseconds: 2500),
    );
    history.add(
      const rust.UiTraffic(up: 200, down: 0),
      const Duration(milliseconds: 5500),
    );
    final first = history.add(
      const rust.UiTraffic(up: 200, down: 0),
      const Duration(seconds: 11),
    );
    expect(first.single.up, 175);
    history.add(
      const rust.UiTraffic(up: 0, down: 0),
      const Duration(seconds: 15),
    );
    final second = history.add(
      const rust.UiTraffic(up: 0, down: 0),
      const Duration(seconds: 20),
    );
    expect(second.single.up, 20);
  });
  test('background gaps do not paint stale high traffic', () {
    final history = TrafficHistory();
    history.add(const rust.UiTraffic(up: 100, down: 100), Duration.zero);
    final gap = history.add(
      const rust.UiTraffic(up: 100, down: 100),
      const Duration(minutes: 10),
    );
    expect(gap.length, 35);
    expect(gap.every((s) => s.up == 0 && s.down == 0), isTrue);
  });
}
