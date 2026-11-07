// Opt-in on a designated isolated device with an already authenticated test
// account. Never runs in normal CI, clears auth, or takes over an active VPN.
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:ppvpn_flutter/main.dart' as app;
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/vpn.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  const enabled =
      bool.fromEnvironment('PPVPN_LIVE_VPN_ACCEPTANCE') &&
      bool.fromEnvironment('PPVPN_ISOLATED_TEST_DEVICE');
  testWidgets(
    'real VPN forwarding and repeated isolated lifecycle',
    skip: !enabled || !(Platform.isIOS || Platform.isAndroid),
    (tester) async {
      final url = Uri.parse(
        const String.fromEnvironment('PPVPN_VPN_ACCEPTANCE_URL'),
      );
      expect(url.scheme, 'https');
      expect(url.host, isNotEmpty);
      expect(url.userInfo, isEmpty);
      await app.main();
      await tester.pump();
      final state = tester
          .widget<app.PrototypeApp>(find.byType(app.PrototypeApp))
          .app!;
      await tester.runAsync(() async {
        const expectedOrigin = String.fromEnvironment(
          'PPVPN_EXPECTED_API_ORIGIN',
        );
        expect(expectedOrigin, isNotEmpty);
        expect(
          Uri.parse(rust.siteUrl(path: '/')).origin,
          expectedOrigin,
          reason:
              'The running app must use the intended production API origin.',
        );
        await _until(() => state.snapshot.value.auth != rust.UiAuth.restoring);
        expect(
          state.snapshot.value.auth,
          rust.UiAuth.signedIn,
          reason:
              'Sign in normally on the designated test device before this run.',
        );
        expect(state.snapshot.value.profileRevision, isNotNull);
        expect(
          state.snapshot.value.phase,
          rust.UiPhase.off,
          reason: 'Refuse to take over an existing client connection.',
        );
        expect(await SystemVpn.prepare(), isTrue);
        await Future<void>.delayed(const Duration(seconds: 1));
        expect(
          SystemVpn.state.value,
          VpnState.off,
          reason: 'Refuse to stop or replace an existing provider.',
        );
        expect(await rust.localProxies(), isEmpty);
        await rust.probeAll(method: rust.UiProbeMethod.tcp);
        await rust.probeAll(method: rust.UiProbeMethod.icmp);
        expect(SystemVpn.state.value, VpnState.off);
        var startedHere = false;
        try {
          for (var cycle = 0; cycle < 3; cycle++) {
            startedHere = true;
            await state.connect();
            expect(state.actionError.value, isNull);
            await _until(
              () => state.snapshot.value.phase == rust.UiPhase.connected,
            );
            await _until(() => SystemVpn.state.value == VpnState.on);
            final before = await _rpc('/v1/get-runtime-observation');
            expect(before['backend'], 'sail');
            expect(before['running'], isTrue);
            expect(before['active_runtime_ids'], hasLength(1));
            if (Platform.isIOS) {
              expect(
                (await _rpc('/v1/get-capture-status'))['phase'],
                'attached',
              );
            }
            final client = HttpClient()..findProxy = (_) => 'DIRECT';
            try {
              final request = await client
                  .getUrl(url)
                  .timeout(const Duration(seconds: 20));
              final response = await request.close().timeout(
                const Duration(seconds: 20),
              );
              expect(response.statusCode, lessThan(400));
              await response
                  .take(1)
                  .drain<void>()
                  .timeout(const Duration(seconds: 20));
            } finally {
              client.close(force: true);
            }
            await rust.probeAll(method: rust.UiProbeMethod.connect);
            final after = await _rpc('/v1/get-runtime-observation');
            expect(after['owner'], before['owner']);
            expect(after['native_runtime_id'], before['native_runtime_id']);
            expect(await rust.localProxies(), isEmpty);
            await state.disconnect();
            await _until(() => SystemVpn.state.value == VpnState.off);
            startedHere = false;
          }
        } finally {
          if (startedHere) await state.disconnect();
        }
      });
    },
    timeout: const Timeout(Duration(minutes: 6)),
  );
}

Future<void> _until(bool Function() ready) async {
  final deadline = DateTime.now().add(const Duration(seconds: 45));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline))
      fail('VPN acceptance state deadline exceeded');
    await Future<void>.delayed(const Duration(milliseconds: 100));
  }
}

Future<Map<String, dynamic>> _rpc(String path) async {
  final dir = await const MethodChannel('com.peakpassvpn.ppvpn/app')
      .invokeMethod<String>('filesDir');
  // Native filesDir is APP_DIR; Flutter's ClientConfig data_dir is APP_DIR/data.
  final socket = Platform.isIOS ? '$dir/data/tunnel.sock' : '$dir/tunnel.sock';
  final stream = await Socket.connect(
    InternetAddress(socket, type: InternetAddressType.unix),
    0,
  );
  try {
    final body = utf8.encode(
      jsonEncode({'path': path, 'body': <String, Object>{}}),
    );
    stream.add((ByteData(4)..setUint32(0, body.length)).buffer.asUint8List());
    stream.add(body);
    await stream.flush();
    final read = BytesBuilder();
    await for (final chunk in stream.timeout(const Duration(seconds: 10))) {
      read.add(chunk);
      final bytes = read.toBytes();
      if (bytes.length < 4) continue;
      final length = ByteData.sublistView(bytes, 0, 4).getUint32(0);
      if (length > 1024 * 1024)
        fail('Unexpectedly large local diagnostic response');
      if (bytes.length < 4 + length) continue;
      final response = jsonDecode(
        utf8.decode(bytes.sublist(4, 4 + length)),
      ) as Map<String, dynamic>;
      expect(response['ok'], isTrue);
      return response['data'] as Map<String, dynamic>;
    }
    fail('Tunnel diagnostic socket closed without a reply');
  } finally {
    stream.destroy();
  }
}
