import 'dart:ui' as ui;

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/pages/overview.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Future<List<int>> pixels(List<rust.UiTraffic> samples, double shift) async {
    final recorder = ui.PictureRecorder();
    final canvas = Canvas(recorder);
    TrafficSparklinePainter(
      samples,
      up: const Color(0xff4455ff),
      down: const Color(0xff00cc88),
      peak: 100,
      shift: shift,
    ).paint(canvas, const Size(290, 48));
    final picture = recorder.endRecording();
    final image = await picture.toImage(290, 48);
    final data = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
    final result = data!.buffer.asUint8List().toList();
    image.dispose();
    picture.dispose();
    return result;
  }

  test('new traffic samples do not reshape either visible curve edge', () async {
    var samples = [
      for (var i = 0; i < AppState.historyLength; i++)
        rust.UiTraffic(up: (i * 23) % 100, down: (i * 47) % 100),
    ];
    // Alternating spikes and idle traffic exercise the real-device case.
    for (final value in [100, 0, 100, 0, 10, 90]) {
      final before = await pixels(samples, 0);
      samples = [
        ...samples.skip(1),
        rust.UiTraffic(up: 100 - value, down: value),
      ];
      final after = await pixels(samples, 1);
      var maxAlphaChange = 0;
      var alphaChange = 0;
      for (var i = 3; i < before.length; i += 4) {
        final delta = (after[i] - before[i]).abs();
        if (delta > maxAlphaChange) maxAlphaChange = delta;
        alphaChange += delta;
      }
      // Offscreen path tessellation can slightly change antialiasing coverage.
      // A changed endpoint tangent moves the stroke by entire pixels.
      expect(maxAlphaChange, lessThanOrEqualTo(4));
      expect(alphaChange, lessThan(64), reason: 'sample $value');
    }
  });
}
