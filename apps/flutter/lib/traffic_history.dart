import 'src/rust/api.dart' as rust;

/// Time-weighted chart buckets. Numeric rates keep using the original events.
class TrafficHistory {
  static const interval = Duration(seconds: 2);
  Duration? _last;
  var _used = 0;
  var _up = 0;
  var _down = 0;

  List<rust.UiTraffic> add(rust.UiTraffic sample, Duration at) {
    final previous = _last;
    _last = at;
    if (previous == null || at <= previous) return const [];
    var remaining = (at - previous).inMicroseconds;
    if (remaining > const Duration(seconds: 6).inMicroseconds) {
      // Resume after suspension: do not stretch the last visible rate across
      // the whole gap. The native rate already averages its real interval.
      _used = _up = _down = 0;
      return List.filled(
        (remaining ~/ interval.inMicroseconds).clamp(1, 35),
        const rust.UiTraffic(up: 0, down: 0),
      );
    }
    final buckets = <rust.UiTraffic>[];
    while (remaining > 0) {
      final part = remaining < interval.inMicroseconds - _used
          ? remaining
          : interval.inMicroseconds - _used;
      _up += sample.up * part;
      _down += sample.down * part;
      _used += part;
      remaining -= part;
      if (_used == interval.inMicroseconds) {
        buckets.add(
          rust.UiTraffic(
            up: (_up / _used).round(),
            down: (_down / _used).round(),
          ),
        );
        _used = _up = _down = 0;
      }
    }
    return buckets;
  }
}
