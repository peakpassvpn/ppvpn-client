// The phone app starts (#448 step 3): the bridge library loads (from the
// APK; on iOS it is linked in), the client starts in the app's own
// directory, and a fresh install shows the sign-in page. Run by
// e2e/android_smoke.sh on ci.yml's emulator, as a tablet in landscape (the
// desktop layout) and as a phone in portrait (the phone layout, step 5),
// and by e2e/ios_smoke.sh on an iPhone simulator.
//
// On Android also the system VPN: TunnelService builds the TUN and starts
// the tunnel host from the APK, which then answers on tunnel.sock in the
// app's directory, and stops again. android_smoke.sh allows the VPN
// beforehand (appops), so no consent dialog shows. No profile: the engine
// stays stopped, nothing is routed.

import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:ppvpn_flutter/main.dart' as app;
import 'package:ppvpn_flutter/vpn.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('starts signed out', (tester) async {
    await app.main();
    // The client restores nothing, then signs out: real time, its threads.
    final deadline = DateTime.now().add(const Duration(seconds: 60));
    while (find.text('Sign In with Browser').evaluate().isEmpty) {
      if (DateTime.now().isAfter(deadline)) fail('the sign-in page did not show');
      await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 200)));
      await tester.pump();
    }
  });

  testWidgets('brings the system VPN up and down', skip: !Platform.isAndroid, (tester) async {
    await tester.runAsync(() async {
      final allowed = await SystemVpn.prepare().timeout(
        const Duration(seconds: 20),
        onTimeout: () => fail('the VPN is not allowed: the consent dialog is up'),
      );
      expect(allowed, isTrue);
      await SystemVpn.start();
      await _until(VpnState.on);
      final dir = await const MethodChannel('com.peakpassvpn.ppvpn/app').invokeMethod<String>('filesDir');
      final reply = await _call('$dir/tunnel.sock', '/v1/get-version');
      expect(reply['ok'], isTrue, reason: '$reply');
      debugPrint('tunnel host: ${reply['data']}');
      await SystemVpn.stop();
      await _until(VpnState.off);
    });
  });
}

/// Waits up to 30 s for the platform to report [want].
Future<void> _until(VpnState want) async {
  final deadline = DateTime.now().add(const Duration(seconds: 30));
  while (SystemVpn.state.value != want) {
    if (DateTime.now().isAfter(deadline)) fail('the VPN stayed ${SystemVpn.state.value.name}, not ${want.name}');
    await Future<void>.delayed(const Duration(milliseconds: 200));
  }
}

/// One call to the tunnel host (crates/desktop tunnel.rs): frames of
/// [u32 BE length][JSON], the request {path, body}, the answer {ok, ...}.
Future<Map<String, dynamic>> _call(String socket, String path) async {
  final stream = await Socket.connect(InternetAddress(socket, type: InternetAddressType.unix), 0);
  try {
    final body = utf8.encode(jsonEncode({'path': path, 'body': <String, Object>{}}));
    stream.add((ByteData(4)..setUint32(0, body.length)).buffer.asUint8List());
    stream.add(body);
    await stream.flush();
    final read = BytesBuilder();
    await for (final chunk in stream.timeout(const Duration(seconds: 10))) {
      read.add(chunk);
      final bytes = read.toBytes();
      if (bytes.length < 4) continue;
      final length = ByteData.sublistView(bytes, 0, 4).getUint32(0);
      if (bytes.length >= 4 + length) {
        return jsonDecode(utf8.decode(bytes.sublist(4, 4 + length))) as Map<String, dynamic>;
      }
    }
    fail('the host closed $socket without an answer');
  } finally {
    stream.destroy();
  }
}
