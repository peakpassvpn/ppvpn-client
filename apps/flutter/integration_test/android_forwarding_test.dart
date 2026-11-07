// Real forwarding acceptance on a disposable Android emulator only.
// e2e/android_forwarding.sh owns the SS node, process/FD audit and revoke APK.
// No test account, production backend or production lifecycle hook is used.
import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:ppvpn_flutter/main.dart' as app;
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/vpn.dart';

const _enabled = bool.fromEnvironment('PPVPN_ANDROID_FORWARDING_ACCEPTANCE');
const _target = '192.0.2.10'; // Exists only behind the controlled SS node.
String? _socket;

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets(
    'Android single owner forwards TCP UDP DNS and settles teardown',
    skip: !_enabled || !Platform.isAndroid,
    (tester) async {
      await app.main();
      await tester.pump();
      await tester.runAsync(() async {
        final info = await developer.Service.getInfo();
        // DDS may advertise a HOST URI here. Keep as a diagnostic hint only;
        // host.py identifies the device VM from logcat/PID lifetime/socket FD.
        final vmHint = info.serverUri?.toString();
        final directory = await const MethodChannel('com.peakpassvpn.ppvpn/app')
            .invokeMethod<String>('filesDir');
        expect(directory, isNotNull);
        _socket = '$directory/tunnel.sock';
        final config = await _controlCall('profile');
        final run = config['run'] as String;
        final profile = config['profile'] as Map<String, dynamic>;
        await _until(() async => rust.start().auth != rust.UiAuth.restoring);
        expect(rust.start().auth, rust.UiAuth.signedOut);
        expect(rust.start().standardReady, isFalse);
        expect(await rust.localProxies(), isEmpty);
        await _controlCall('authorize-primary');
        expect(await SystemVpn.prepare(), isTrue);
        await _until(() async => SystemVpn.state.value == VpnState.off);
        await _negativeTCP();
        await _negativeUDP();
        final initial = await _controlCall('checkpoint', {
          'stage': 'initial-off',
          'reported_vm_uri': vmHint,
        });
        final deviceVm = initial['device_vm'];
        if (deviceVm != null) await _notProxy(deviceVm['port'] as int);
        Map<String, dynamic>? previousOwner;
        Object? primaryFailure;
        var activeCycle = -1;
        try {
          for (var cycle = 0; cycle < 3; cycle++) {
            activeCycle = cycle;
            await SystemVpn.start();
            await _until(() async => SystemVpn.state.value == VpnState.on);
            final idle = await _rpc('/v1/get-runtime-observation');
            expect(
              idle['running'],
              isFalse,
              reason: 'Host-on is not Engine-on',
            );
            expect(idle['active_runtime_ids'], isEmpty);
            await _rpc('/v1/apply-profile', {
              'profile': profile,
              'routing_mode': 'rules',
            });
            await _rpc('/v1/start');
            final before = await _rpc('/v1/get-runtime-observation');
            expect(before['backend'], 'sail');
            expect(before['running'], isTrue);
            expect(before['active_runtime_ids'], [before['native_runtime_id']]);
            final owner = before['owner'] as Map<String, dynamic>;
            if (previousOwner != null) {
              expect(
                owner['instance'],
                isNot(previousOwner['instance']),
                reason: 'A new host must not reuse a stopped owner identity',
              );
            }
            previousOwner = owner;
            await _controlCall('dns-baseline', {
              'cycle': cycle,
              'observation': before,
              'status': await _rpc('/v1/get-status'),
            });
            final baseline = await _rpc('/v1/get-traffic');
            final tcp = await Socket.connect(
              _target,
              18081,
              timeout: const Duration(seconds: 15),
            );
            final lines = StreamIterator(
              tcp
                  .cast<List<int>>()
                  .transform(utf8.decoder)
                  .transform(const LineSplitter()),
            );
            try {
              final token = '$run-cycle-$cycle';
              tcp.writeln('$token-tcp');
              await tcp.flush();
              expect(
                await lines.moveNext().timeout(const Duration(seconds: 15)),
                isTrue,
              );
              expect(lines.current, '$token-tcp');
              final tcpWitness = _TcpWitness(lines);
              await _udpEcho(
                '$token-udp',
                whileOpen: () async {
                  try {
                    await _dns(cycle);
                  } catch (error) {
                    // Preserve owner diagnostics before flutter test uninstalls
                    // the APK. Diagnostic failure must not replace the DNS error.
                    try {
                      await _controlCall('diagnostics', {
                        'cycle': cycle,
                        'error': error.toString(),
                        'status': await _rpc('/v1/get-status'),
                        'observation': await _rpc(
                          '/v1/get-runtime-observation',
                        ),
                        'logs': await _ownerLogs(directory!),
                      });
                    } catch (diagnosticError) {
                      developer.log(
                        'DNS diagnostic capture failed: $diagnosticError',
                      );
                    }
                    rethrow;
                  }
                  await _until(() async {
                    tcpWitness.requireOpen();
                    final traffic = await _rpc('/v1/get-traffic');
                    tcpWitness.requireOpen();
                    return (traffic['upload_bytes'] as int) >
                            (baseline['upload_bytes'] as int) &&
                        (traffic['download_bytes'] as int) >
                            (baseline['download_bytes'] as int);
                  });
                  final connections = await _witnessConnections(() async {
                    tcpWitness.requireOpen();
                    final value =
                        await _rpc('/v1/get-connections') as List<dynamic>;
                    tcpWitness.requireOpen();
                    return value;
                  });
                  tcpWitness.requireOpen();
                  final after = await _rpc('/v1/get-runtime-observation');
                  expect(after['owner'], before['owner']);
                  expect(
                    after['native_runtime_id'],
                    before['native_runtime_id'],
                  );
                  expect(
                    after['active_runtime_ids'],
                    before['active_runtime_ids'],
                  );
                  expect(rust.start().standardReady, isFalse);
                  expect(await rust.localProxies(), isEmpty);
                  final sampledTraffic = await _rpc('/v1/get-traffic');
                  tcpWitness.requireOpen();
                  await _controlCall('checkpoint', {
                    'stage': 'connected',
                    'cycle': cycle,
                    'token': token,
                    'reported_vm_uri': vmHint,
                    'observation': after,
                    'traffic': sampledTraffic,
                    'connections': connections,
                    'tcp_witness_pending': true,
                  });
                  tcpWitness.requireOpen();
                },
              );
              tcpWitness.requireOpen();
              if (cycle == 2) {
                // The rival establish() makes Android revoke this provider;
                // production TunnelService.onRevoke(), not a test method call.
                await _controlCall('revoke');
              } else {
                await _rpc('/v1/stop');
                final stopped = await _rpc('/v1/get-runtime-observation');
                expect(stopped['running'], isFalse);
                expect(stopped['active_runtime_ids'], isEmpty);
                await SystemVpn.stop();
              }
              await _until(() async => SystemVpn.state.value == VpnState.off);
              await _until(() async => !await File(_socket!).exists());
              // Reuse the one pending read: it must end only after teardown.
              await tcpWitness.requireEnded();
            } finally {
              tcp.destroy();
              await lines.cancel();
            }
            await _controlCall('checkpoint', {
              'stage': cycle == 2 ? 'revoked' : 'stopped',
              'cycle': cycle,
              'reported_vm_uri': vmHint,
            });
            if (cycle == 2) await _controlCall('release-rival');
            await _negativeTCP();
            await _negativeUDP();
          }
          await _controlCall('complete');
        } catch (error, stack) {
          primaryFailure = error;
          // Cleanup can reset fixture sockets. Preserve the original failing
          // operation before teardown and before flutter uninstalls the App.
          print(
            'android-forwarding primary-failure cycle=$activeCycle: $error',
          );
          print(stack);
          if (activeCycle >= 0) {
            try {
              await _controlCall('diagnostics', {
                'cycle': activeCycle,
                'stage': 'primary-failure',
                'error': error.toString(),
                'logs': await _ownerLogs(directory!),
              });
            } catch (diagnosticError) {
              print('android-forwarding diagnostic-failure: $diagnosticError');
            }
          }
          rethrow;
        } finally {
          Object? cleanupFailure;
          StackTrace? cleanupStack;
          for (final cleanup in <Future<void> Function()>[
            () => SystemVpn.stop(force: true),
            () async {
              await _controlCall('release-rival');
            },
          ]) {
            try {
              await cleanup();
            } catch (error, stack) {
              print('android-forwarding cleanup-failure: $error');
              cleanupFailure ??= error;
              cleanupStack ??= stack;
            }
          }
          // A cleanup error still fails a successful test, but must not
          // replace the original traffic/lifecycle failure and stack.
          if (primaryFailure == null && cleanupFailure != null) {
            Error.throwWithStackTrace(cleanupFailure, cleanupStack!);
          }
        }
      });
    },
    timeout: const Timeout(Duration(minutes: 8)),
  );
}

Future<Map<String, String>> _ownerLogs(String directory) async {
  final result = <String, String>{};
  for (final name in ['ppvpn-core.log', 'tunnel-host.log']) {
    final file = File('$directory/tunnel-state/logs/$name');
    if (!await file.exists()) continue;
    final length = await file.length();
    final tail = length > 65536 ? length - 65536 : 0;
    result[name] = utf8.decode(
      await file
          .openRead(tail)
          .fold<List<int>>(<int>[], (bytes, part) => bytes..addAll(part)),
      allowMalformed: true,
    );
  }
  return result;
}

Future<void> _notProxy(int port) async {
  Future<List<int>> exchange(List<int> request, {bool header = false}) async {
    final socket = await Socket.connect(
      '127.0.0.1',
      port,
      timeout: const Duration(seconds: 3),
    );
    final data = BytesBuilder();
    try {
      socket.add(request);
      await socket.flush();
      try {
        await for (final chunk in socket.timeout(const Duration(seconds: 3))) {
          data.add(chunk);
          if (data.length > 8192)
            fail('Unexpectedly large VM protocol response');
          if (!header ||
              ascii
                  .decode(data.toBytes(), allowInvalid: true)
                  .contains('\r\n\r\n')) {
            break;
          }
        }
      } on TimeoutException {
        if (header && data.isNotEmpty) fail('Incomplete VM HTTP response');
      } on SocketException {
        /* Rejected/reset, no tunnel. */
      }
      return data.toBytes();
    } finally {
      socket.destroy();
    }
  }

  final socks = await exchange([5, 1, 0]);
  expect(
    socks.isNotEmpty && socks.first == 5,
    isFalse,
    reason: 'Device VM exception must not negotiate SOCKS5',
  );
  final http = ascii.decode(
    await exchange(
      ascii.encode(
        'CONNECT 192.0.2.10:18081 HTTP/1.1\r\nHost: 192.0.2.10:18081\r\n\r\n',
      ),
      header: true,
    ),
    allowInvalid: true,
  );
  final status = RegExp(r'^HTTP/1\.[01] (\d{3})').firstMatch(http);
  if (status != null) {
    final code = int.parse(status[1]!);
    expect(
      code >= 200 && code < 300 || code == 407,
      isFalse,
      reason: 'Device VM exception must not serve HTTP CONNECT/proxy auth',
    );
  }
}

class _TcpWitness {
  _TcpWitness(StreamIterator<String> lines) {
    // Exactly one read, armed immediately after the exact echo. Errors are
    // observed here even before teardown awaits the result.
    _done = lines.moveNext().then<void>(
      (more) {
        _more = more;
        _completed = true;
      },
      onError: (Object error, StackTrace stack) {
        _error = error;
        _stack = stack;
        _completed = true;
      },
    );
  }
  late final Future<void> _done;
  bool _completed = false;
  bool? _more;
  Object? _error;
  StackTrace? _stack;

  void requireOpen() {
    if (_completed) {
      fail(
        'TCP witness ended before owner teardown: more=$_more error=$_error',
      );
    }
  }

  Future<void> requireEnded() async {
    await _done.timeout(const Duration(seconds: 15));
    final error = _error;
    if (error is SocketException) return; // Teardown may reset instead of FIN.
    if (error != null) Error.throwWithStackTrace(error, _stack!);
    expect(_more, isFalse);
  }
}

Future<List<dynamic>> _witnessConnections(
  Future<List<dynamic>> Function() read,
) async {
  // Engine.connections is a once-per-second cached snapshot (engine/reads.rs).
  // Global counters may already advance on unrelated Android system traffic.
  // Keep both witness sockets open and wait for their exact routed rows;
  // polling this read never retransmits data or reconnects a failed flow.
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  bool witness(dynamic value, String network, int port) =>
      value is Map &&
      value['network'] == network &&
      value['node_id'] == 'android-node' &&
      value['destination'] == '$_target:$port' &&
      value['upload_bytes'] is num &&
      value['upload_bytes'] > 0 &&
      value['download_bytes'] is num &&
      value['download_bytes'] > 0;
  List<dynamic> connections = [];
  Never expired() =>
      fail('No sampled bidirectional TCP/UDP witness within 5s: $connections');
  while (true) {
    final remaining = deadline.difference(DateTime.now());
    if (remaining <= Duration.zero) expired();
    try {
      connections = await read().timeout(remaining);
    } on TimeoutException {
      expired();
    }
    if (connections.any((c) => witness(c, 'tcp', 18081)) &&
        connections.any((c) => witness(c, 'udp', 18082))) {
      return connections;
    }
    final pause = deadline.difference(DateTime.now());
    if (pause <= Duration.zero) expired();
    await Future<void>.delayed(
      pause < const Duration(milliseconds: 100)
          ? pause
          : const Duration(milliseconds: 100),
    );
  }
}

Future<void> _until(Future<bool> Function() ready) async {
  final end = DateTime.now().add(const Duration(seconds: 40));
  while (!await ready()) {
    if (DateTime.now().isAfter(end)) fail('Android forwarding state deadline');
    await Future<void>.delayed(const Duration(milliseconds: 100));
  }
}

Future<dynamic> _rpc(
  String path, [
  Map<String, dynamic> body = const {},
]) async {
  final stream = await Socket.connect(
    InternetAddress(_socket!, type: InternetAddressType.unix),
    0,
  ).timeout(const Duration(seconds: 10));
  try {
    final request = utf8.encode(jsonEncode({'path': path, 'body': body}));
    stream.add(
      (ByteData(4)..setUint32(0, request.length)).buffer.asUint8List(),
    );
    stream.add(request);
    await stream.flush();
    final bytes = BytesBuilder();
    await for (final part in stream.timeout(const Duration(seconds: 15))) {
      bytes.add(part);
      final frame = bytes.toBytes();
      if (frame.length < 4) continue;
      final size = ByteData.sublistView(frame, 0, 4).getUint32(0);
      if (size > 1024 * 1024) fail('Oversized owner response');
      if (frame.length < size + 4) continue;
      final reply = jsonDecode(
        utf8.decode(frame.sublist(4, size + 4)),
      ) as Map<String, dynamic>;
      expect(reply['ok'], isTrue, reason: '$path: $reply');
      return reply['data'];
    }
    fail('Owner closed before replying to $path');
  } finally {
    stream.destroy();
  }
}

Future<Map<String, dynamic>> _controlCall(
  String path, [
  Map<String, dynamic> body = const {},
]) async {
  // Raw loopback HTTP keeps fixture control independent of App cleartext
  // policy; production manifest/security settings are deliberately unchanged.
  final socket = await Socket.connect(
    '10.0.2.2',
    18090,
    timeout: const Duration(seconds: 10),
  );
  try {
    final payload = utf8.encode(jsonEncode(body));
    final bytes = BytesBuilder();
    // Subscribe before writing: a reset during flush must reach this awaited
    // response future, rather than an unobserved Socket stream error.
    final response = socket.timeout(const Duration(seconds: 30)).forEach((
      chunk,
    ) {
      bytes.add(chunk);
      if (bytes.length > 1024 * 1024) fail('Oversized fixture response');
    });
    socket.add(
      ascii.encode(
        'POST /$path HTTP/1.0\r\nHost: 10.0.2.2\r\n'
        'Content-Type: application/json\r\nContent-Length: ${payload.length}\r\n\r\n',
      ),
    );
    socket.add(payload);
    await Future.wait<void>([socket.flush(), response], eagerError: true);
    final text = utf8.decode(bytes.toBytes());
    final split = text.indexOf('\r\n\r\n');
    expect(split, greaterThan(0), reason: text);
    expect(
      text.split('\r\n').first,
      matches(r'^HTTP/1\.[01] 200 '),
      reason: text,
    );
    return jsonDecode(text.substring(split + 4)) as Map<String, dynamic>;
  } finally {
    socket.destroy();
  }
}

Future<void> _negativeTCP() async {
  Socket? socket;
  try {
    socket = await Socket.connect(
      _target,
      18081,
      timeout: const Duration(seconds: 2),
    );
    fail('TEST-NET echo reachable while owner is stopped (bypass)');
  } on SocketException {
    /* Expected, including connect timeout. */
  } on TimeoutException {
    /* Also accept the Dart timer form of an off-state connect timeout. */
  } finally {
    socket?.destroy();
  }
}

Future<Datagram> _datagram(
  InternetAddress destination,
  int port,
  List<int> payload,
  bool Function(Datagram) accept, {
  Duration timeout = const Duration(seconds: 20),
  Future<void> Function(Datagram)? whileOpen,
}) async {
  final socket = await RawDatagramSocket.bind(InternetAddress.anyIPv4, 0);
  final result = Completer<Datagram>();
  final subscription = socket.listen((event) {
    if (event != RawSocketEvent.read) return;
    Datagram? packet;
    while ((packet = socket.receive()) != null) {
      if (accept(packet!) && !result.isCompleted) result.complete(packet);
    }
  }, onError: result.completeError);
  try {
    expect(socket.send(payload, destination, port), payload.length);
    final packet = await result.future.timeout(timeout);
    if (whileOpen != null) await whileOpen(packet);
    return packet;
  } finally {
    await subscription.cancel();
    socket.close();
  }
}

Future<void> _negativeUDP() async {
  try {
    await _datagram(
      InternetAddress(_target),
      18082,
      ascii.encode('off-negative'),
      (_) => true,
      timeout: const Duration(seconds: 2),
    );
    fail('TEST-NET UDP echo reachable while owner is stopped (bypass)');
  } on TimeoutException {
    /* Expected: no tunnel, no routed node. */
  } on SocketException {
    /* Kernel may reject the route immediately. */
  }
}

Future<void> _udpEcho(
  String token, {
  required Future<void> Function() whileOpen,
}) async {
  final payload = utf8.encode(token);
  final packet = await _datagram(
    InternetAddress(_target),
    18082,
    payload,
    (p) =>
        p.port == 18082 && utf8.decode(p.data, allowMalformed: true) == token,
    // Observe the routed UDP flow while its App socket is still alive.
    // Closing it before get-connections races legitimate flow teardown.
    whileOpen: (packet) async {
      expect(packet.address.address, _target);
      expect(packet.data, payload);
      await whileOpen();
    },
  );
  expect(packet.address.address, _target);
  expect(packet.data, payload);
}

Future<void> _dns(int cycle) async {
  final id = 0x6a00 + cycle;
  final query = BytesBuilder()
    ..add(
      (ByteData(12)
            ..setUint16(0, id)
            ..setUint16(2, 0x0100)
            ..setUint16(4, 1))
          .buffer
          .asUint8List(),
    );
  final domain = ['example.com', 'example.org', 'example.net'][cycle];
  for (final label in domain.split('.')) {
    query.add([label.length]);
    query.add(ascii.encode(label));
  }
  query.add([0, 0, 1, 0, 1]); // A IN, through the actual VPN DNS VIP.
  final response = await _datagram(
    InternetAddress('10.60.159.90'),
    53,
    query.toBytes(),
    (p) =>
        p.data.length >= 12 && ByteData.sublistView(p.data).getUint16(0) == id,
  );
  final header = ByteData.sublistView(response.data);
  expect(response.address.address, '10.60.159.90');
  expect(response.port, 53);
  expect(header.getUint16(2) & 0x800f, 0x8000, reason: 'DNS QR=1 / NOERROR');
  expect(header.getUint16(6), greaterThan(0), reason: 'An actual DNS answer');
}
