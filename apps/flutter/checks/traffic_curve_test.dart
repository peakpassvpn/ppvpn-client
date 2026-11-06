import 'dart:ui' as ui;

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/pages/overview.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Future<List<int>> pixels(
    List<rust.UiTraffic> samples,
    double shift, {
    double peak = 100,
  }) async {
    // Use whole-pixel steps to isolate endpoint geometry from raster rounding.
    const width = (AppState.historyVisibleLength - 1) * 30;
    final recorder = ui.PictureRecorder();
    final canvas = Canvas(recorder);
    TrafficSparklinePainter(
      samples,
      up: const Color(0xff4455ff),
      down: const Color(0xff00cc88),
      peak: peak,
      shift: shift,
    ).paint(canvas, Size(width.toDouble(), 48));
    final picture = recorder.endRecording();
    final image = await picture.toImage(width, 48);
    final data = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
    final result = data!.buffer.asUint8List().toList();
    image.dispose();
    picture.dispose();
    return result;
  }

  test(
    'traffic spike remains visible while the previous scale catches up',
    () async {
      final samples = List.filled(
        AppState.historyLength,
        const rust.UiTraffic(up: 5000, down: 10000),
      );
      for (final peak in [1.0, 100.0, 1000.0, 10000.0]) {
        final rgba = await pixels(samples, 0, peak: peak);
        final opaqueStroke = [
          for (var i = 3; i < rgba.length; i += 4)
            if (rgba[i] >= 200) i,
        ];
        expect(
          opaqueStroke.length,
          greaterThan(100),
          reason: 'the waveform must not leave the canvas at scale $peak',
        );
      }
    },
  );

  test(
    'newest traffic reaches the visible edge without future samples',
    () async {
      final samples = List.filled(
        AppState.historyLength,
        const rust.UiTraffic(up: 0, down: 0),
      );
      samples[samples.length - 1] = const rust.UiTraffic(up: 0, down: 10000);
      final rgba = await pixels(samples, 0, peak: 10000);
      const width = (AppState.historyVisibleLength - 1) * 30;
      var visible = false;
      for (var y = 0; y < 44; y++) {
        for (var x = width - 10; x < width; x++) {
          if (rgba[(y * width + x) * 4 + 3] >= 200) visible = true;
        }
      }
      expect(visible, isTrue);
    },
  );

  test(
    'new traffic samples do not reshape either visible curve edge',
    () async {
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
        // Clipping a different offscreen segment can change a few alpha levels
        // of antialiasing; total coverage must still remain almost identical.
        // A changed endpoint tangent moves the stroke by entire pixels.
        expect(maxAlphaChange, lessThanOrEqualTo(8));
        expect(alphaChange, lessThan(64), reason: 'sample $value');
      }
    },
  );
}
