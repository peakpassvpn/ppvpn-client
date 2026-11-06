import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/traffic_history.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;

void main() {
  test('chart averages a bucket while numeric rate events stay live', () {
    final history = TrafficHistory();
    const zero = rust.UiTraffic(up: 0, down: 0);
    const burst = rust.UiTraffic(up: 100, down: 200);
    expect(history.add(zero, Duration.zero), isEmpty);
    expect(
      history.add(
        zero,
        Duration(microseconds: TrafficHistory.interval.inMicroseconds ~/ 2),
      ),
      isEmpty,
    );
    final result = history.add(burst, TrafficHistory.interval);
    expect((result.single.up, result.single.down), (50, 100));
    expect((burst.up, burst.down), (100, 200));
  });
  test('irregular event timing is weighted and split at bucket boundaries', () {
    final history = TrafficHistory();
    history.add(const rust.UiTraffic(up: 0, down: 0), Duration.zero);
    history.add(
      const rust.UiTraffic(up: 100, down: 0),
      Duration(microseconds: TrafficHistory.interval.inMicroseconds ~/ 4),
    );
    final first = history.add(
      const rust.UiTraffic(up: 200, down: 0),
      Duration(microseconds: TrafficHistory.interval.inMicroseconds * 5 ~/ 4),
    );
    expect(first.single.up, 175);
    final second = history.add(
      const rust.UiTraffic(up: 0, down: 0),
      TrafficHistory.interval * 2,
    );
    expect(second.single.up, 50);
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
