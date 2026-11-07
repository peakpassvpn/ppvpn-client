import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/apple_push_transport.dart';

const installation = '00000000-0000-4000-8000-000000000001';
const binding = '00000000-0000-4000-8000-000000000002';
const registration = '00000000-0000-4000-8000-000000000003';
const nextRegistration = '00000000-0000-4000-8000-000000000004';
final proof = {
  'installation_id': installation,
  'installation_secret': List.filled(43, 'A').join(),
};
Map<String, Object?> request({
  String owner = 'A',
  String generation = '1',
  String token = 'aabb',
}) => {
  'accountID': owner,
  'accountGeneration': generation,
  'platform': 'ios',
  'bundleID': 'test.fixture',
  'environment': 'production',
  'token': token,
};
ApplePushReply response(String op) =>
    ApplePushReply(op == 'unbind' ? 204 : 200, switch (op) {
      'config' => {
        'configured': true,
        'bundle_id_ios': 'test.fixture',
        'bundle_id_macos': 'test.mac',
        'environments': ['production'],
      },
      'resolve' => {
        'installation_id': installation,
        'binding_id': null,
        'registration_id': null,
        'bound_to_current_user': false,
        'bound_to_current_session': false,
      },
      'bind' => {
        'installation_id': installation,
        'binding_id': binding,
        'registration_id': null,
        'bound_to_current_user': true,
        'bound_to_current_session': true,
      },
      'register' => {
        'id': 1,
        'registration_id': registration,
        'token_hash_prefix': 'fixture',
      },
      _ => {},
    });
void main() {
  test(
    'exact Resolve-Bind-Register-Close wire intent and variable OS token',
    () async {
      final calls = <(String, Map<String, Object?>)>[];
      final t = ApplePushTransport(
        proof: (_) async => proof,
        http: (owner, op, body) async {
          expect(owner, 'A');
          calls.add((op, body));
          return response(op);
        },
      )..followAccount('A');
      expect(await t.register(request()), true);
      expect(calls.map((c) => c.$1), ['config', 'resolve', 'bind', 'register']);
      expect(calls[2].$2['expected_binding_id'], null);
      expect(calls[3].$2['binding_id'], binding);
      expect(calls[3].$2['expected_registration_id'], null);
      expect(
        calls.every(
          (c) => !c.$2.containsKey('account_id') && !c.$2.containsKey('data'),
        ),
        true,
      );
      t.followAccount(null);
      expect(await t.unregister(request()), true);
      expect(calls.last.$1, 'unbind');
      expect(calls.last.$2['binding_id'], binding);
      expect(await t.register(request()), false);
    },
  );
  test(
    'lost Bind response retries identical request_id and CAS without Resolve',
    () async {
      final bodies = <Map<String, Object?>>[];
      var resolves = 0;
      final t = ApplePushTransport(
        proof: (_) async => proof,
        http: (owner, op, body) async {
          if (op == 'resolve') resolves++;
          if (op == 'bind') {
            bodies.add(body);
            if (bodies.length == 1) throw StateError('lost response');
          }
          return response(op);
        },
      )..followAccount('A');
      expect(await t.register(request()), false);
      expect(await t.register({...request(), 'request': 'native-retry'}), true);
      expect(bodies.length, 2);
      expect(bodies[0], bodies[1]);
      expect(resolves, 1);
    },
  );
  test('409 closes intent; resume cannot Resolve and steal binding', () async {
    final calls = <String>[];
    final t = ApplePushTransport(
      proof: (_) async => proof,
      http: (owner, op, body) async {
        calls.add(op);
        return op == 'bind'
            ? ApplePushReply(409, {'code': 'DEVICE_BINDING_CONFLICT'})
            : response(op);
      },
    )..followAccount('A');
    for (var i = 0; i < 5; i++) expect(await t.register(request()), false);
    expect(calls, ['config', 'resolve', 'bind']);
  });
  test(
    '503 attempts bounded to3, preserving registration request_id/input',
    () async {
      final bodies = <Map<String, Object?>>[];
      final t = ApplePushTransport(
        proof: (_) async => proof,
        http: (owner, op, body) async {
          if (op == 'register') {
            bodies.add(body);
            return ApplePushReply(503, {});
          }
          return response(op);
        },
      )..followAccount('A');
      for (var i = 0; i < 6; i++) expect(await t.register(request()), false);
      expect(bodies.length, 3);
      expect(
        bodies.every((b) => b.toString() == bodies.first.toString()),
        true,
      );
    },
  );
  test(
    'rotation keeps binding and advances exact expected registration',
    () async {
      final bodies = <Map<String, Object?>>[];
      var binds = 0;
      var unbinds = 0;
      final t = ApplePushTransport(
        proof: (_) async => proof,
        http: (owner, op, body) async {
          if (op == 'bind') binds++;
          if (op == 'unbind') unbinds++;
          if (op == 'register') {
            bodies.add(body);
            return ApplePushReply(200, {
              'registration_id': bodies.length == 1
                  ? registration
                  : nextRegistration,
            });
          }
          return response(op);
        },
      )..followAccount('A');
      expect(await t.register(request()), true);
      expect(await t.register(request(token: '001122334455')), true);
      expect(binds, 1);
      expect(unbinds, 0);
      expect(bodies[1]['expected_registration_id'], registration);
      expect(bodies[1]['request_id'], isNot(bodies[0]['request_id']));
    },
  );
  test('late old-owner Bind success retained only for queued close', () async {
    final bound = Completer<ApplePushReply>();
    final entered = Completer<void>();
    final calls = <String>[];
    final t = ApplePushTransport(
      proof: (_) async => proof,
      http: (owner, op, body) async {
        calls.add('$owner:$op');
        if (op == 'bind') {
          entered.complete();
          return bound.future;
        }
        return response(op);
      },
    )..followAccount('A');
    final pending = t.register(request());
    await entered.future;
    t.followAccount('B');
    bound.complete(response('bind'));
    expect(await pending, false);
    expect(calls.contains('A:register'), false);
    expect(await t.unregister(request()), true);
    expect(calls.last, 'A:unbind');
  });
  test(
    'same account after logout requires new native generation and intention',
    () async {
      var binds = 0;
      final t = ApplePushTransport(
        proof: (_) async => proof,
        http: (owner, op, body) async {
          if (op == 'bind') binds++;
          return response(op);
        },
      )..followAccount('A');
      expect(await t.register(request()), true);
      t.followAccount(null);
      await t.unregister(request());
      t.followAccount('A');
      expect(await t.register(request()), false);
      expect(await t.register(request(generation: '3')), true);
      expect(binds, 2);
    },
  );
  test('same user in a different session must CAS Bind', () async {
    Map<String, Object?>? bindBody;
    final t = ApplePushTransport(
      proof: (_) async => proof,
      http: (owner, op, body) async {
        if (op == 'resolve')
          return ApplePushReply(200, {
            'installation_id': installation,
            'binding_id': binding,
            'bound_to_current_user': true,
            'bound_to_current_session': false,
          });
        if (op == 'bind') bindBody = body;
        return response(op);
      },
    )..followAccount('A');
    expect(await t.register(request()), true);
    expect(bindBody!['expected_binding_id'], binding);
  });
  test(
    'existing active session updates token without replacing binding',
    () async {
      final calls = <String>[];
      final t = ApplePushTransport(
        proof: (_) async => proof,
        http: (owner, op, body) async {
          calls.add(op);
          if (op == 'resolve')
            return ApplePushReply(200, {
              'installation_id': installation,
              'binding_id': binding,
              'registration_id': registration,
              'bound_to_current_user': true,
              'bound_to_current_session': true,
            });
          if (op == 'register')
            expect(body['expected_registration_id'], registration);
          return response(op);
        },
      )..followAccount('A');
      expect(await t.register(request()), true);
      expect(calls.contains('bind'), false);
    },
  );
  test('locked store and mismatched configured topic cannot bind', () async {
    final calls = <String>[];
    final t = ApplePushTransport(
      proof: (_) async => throw StateError('locked'),
      http: (owner, op, body) async {
        calls.add(op);
        return response(op);
      },
    )..followAccount('A');
    expect(await t.register(request()), false);
    expect(calls, ['config']);
    expect(await t.register({...request(), 'bundleID': 'foreign'}), false);
    expect(calls, ['config', 'config']);
  });
  test(
    'malformed token fails before HTTP, valid token length is variable',
    () async {
      var calls = 0;
      final t = ApplePushTransport(
        proof: (_) async => proof,
        http: (owner, op, body) async {
          calls++;
          return response(op);
        },
      )..followAccount('A');
      for (final token in ['', '0', 'nothex', List.filled(4098, 'a').join()])
        expect(await t.register(request(token: token)), false);
      expect(calls, 0);
      expect(await t.register(request(token: '01')), true);
    },
  );
  test(
    'offline updates bounded pagination and replacement-owner isolation',
    () async {
      final bodies = <Map<String, Object?>>[];
      final t = ApplePushTransport(
        proof: (_) async => proof,
        http: (owner, op, body) async {
          bodies.add(body);
          return ApplePushReply(200, {
            'items': bodies.length == 1
                ? [
                    {'id': 1},
                    {'id': 2},
                  ]
                : [
                    {'id': 3},
                  ],
            'has_more': bodies.length == 1,
          });
        },
      )..followAccount('A');
      expect(await t.updates('A'), true);
      expect(bodies, [
        {'after_id': 0},
        {'after_id': 2},
      ]);
      t.followAccount('B');
      expect(await t.updates('A'), false);
      expect(bodies.length, 2);
    },
  );
  test('empty nullable SDK updates list is valid', () async {
    final t = ApplePushTransport(
      proof: (_) async => proof,
      http: (_, _, _) async =>
          ApplePushReply(200, {'items': null, 'has_more': false}),
    )..followAccount('A');
    expect(await t.updates('A'), true);
  });
  test('unconfigured capability retries at most3 and never binds', () async {
    final calls = <String>[];
    final t = ApplePushTransport(
      proof: (_) async => proof,
      http: (_, op, _) async {
        calls.add(op);
        return ApplePushReply(200, {'configured': false});
      },
    )..followAccount('A');
    for (var n = 0; n < 6; n++) expect(await t.register(request()), false);
    expect(calls, ['config', 'config', 'config']);
  });
}
