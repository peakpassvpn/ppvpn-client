// Measurement hooks for apps/flutter/scripts/compare.py (#387); none is
// active unless its variable is set.
//
// - PPVPN_FLUTTER_TIMING=FILE, PPVPN_FLUTTER_T0_MS=epoch ms of the launch:
//   each startup phase appends `phase ms-since-launch` to FILE.
// - PPVPN_FLUTTER_TRAY_BENCH=1: the tray rebuilds its icon and menu every
//   second (the worst case; normally it changes only with the snapshot).
// - PPVPN_FLUTTER_SYNTHETIC_TRAFFIC=1 is read by the Rust bridge: it pushes
//   a traffic sample every second through the same path as the client's.

import 'dart:io';

final _env = Platform.environment;
final _timing = _env['PPVPN_FLUTTER_TIMING'];
final _t0 = int.tryParse(_env['PPVPN_FLUTTER_T0_MS'] ?? '');

final bool trayBench = _env['PPVPN_FLUTTER_TRAY_BENCH'] == '1';

/// Notes that [phase] has been reached.
void mark(String phase) {
  final file = _timing;
  final t0 = _t0;
  if (file == null || t0 == null) return;
  final ms = DateTime.now().millisecondsSinceEpoch - t0;
  try {
    File(file).writeAsStringSync('$phase $ms\n', mode: FileMode.append, flush: true);
  } on FileSystemException {
    // A measurement aid only.
  }
}
