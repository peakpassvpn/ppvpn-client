// A test-only switch for e2e/linux_enhanced.sh (#401 step 2); off unless
// its variable is set.
//
// - PPVPN_FLUTTER_E2E=enhanced: once signed in with a profile and the
//   standard core ready, turns the connection on in enhanced mode, as the
//   overview's switch does. Prints `E2E ready`, `E2E phase <phase>` on every
//   change (with the error code, if any) and `E2E action-error <error>` to
//   stdout. Lines on stdin: `connect`, `disconnect`, `quit`. SIGTERM (and
//   `quit`) disconnects, waits for the connection to be off, prints
//   `E2E quit` and quits as the tray's Quit does.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'app_state.dart';
import 'src/rust/api.dart' as rust;

void _say(String line) => stdout.writeln('E2E $line');

/// Starts the switch's driver when PPVPN_FLUTTER_E2E asks for it; [quit] is
/// the tray's Quit.
void startE2e(AppState app, Future<void> Function() quit) {
  final mode = Platform.environment['PPVPN_FLUTTER_E2E'];
  if (mode == null || mode.isEmpty) return;
  if (mode != 'enhanced') {
    _say('action-error unknown PPVPN_FLUTTER_E2E=$mode');
    return;
  }
  const enhanced = rust.UiConnectionMode.enhanced;
  rust.UiPhase? phase;
  var ready = false;
  var quitting = false;

  void onSnapshot() {
    final s = app.snapshot.value;
    if (s.phase != phase) {
      phase = s.phase;
      final error = s.error;
      _say('phase ${s.phase.name}${error == null ? '' : ' (${error.code}: ${error.detail})'}');
    }
    if (!ready && s.auth == rust.UiAuth.signedIn && s.profile == rust.UiProfile.ready && s.standardReady) {
      ready = true;
      _say('ready');
      unawaited(app.connectIn(enhanced));
    }
  }

  void onActionError() {
    final error = app.actionError.value;
    if (error != null) _say('action-error $error');
  }

  Future<void> finish() async {
    if (quitting) return;
    quitting = true;
    if (app.snapshot.value.phase != rust.UiPhase.off) await app.disconnect();
    final deadline = DateTime.now().add(const Duration(seconds: 30));
    while (app.snapshot.value.phase != rust.UiPhase.off && DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
    _say('quit');
    await stdout.flush();
    await quit();
    exit(0);
  }

  app.snapshot.addListener(onSnapshot);
  app.actionError.addListener(onActionError);
  onSnapshot();
  ProcessSignal.sigterm.watch().listen((_) => unawaited(finish()));
  stdin.transform(utf8.decoder).transform(const LineSplitter()).listen((line) {
    switch (line.trim()) {
      case 'connect':
        unawaited(app.connectIn(enhanced));
      case 'disconnect':
        unawaited(app.disconnect());
      case 'quit':
        unawaited(finish());
    }
  });
}
