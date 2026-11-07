import 'dart:convert';
import 'dart:math';

/// Internal bridge response; the actual backend JSON has no data wrapper.
class ApplePushReply {
  ApplePushReply(this.status, this.body);
  final int status;
  final Map<String, Object?> body;
  static ApplePushReply decode(String json) {
    final value = jsonDecode(json) as Map<String, dynamic>;
    return ApplePushReply(
      value['status'] as int,
      value['body'] == null
          ? {}
          : Map<String, Object?>.from(value['body'] as Map),
    );
  }
}

typedef ApplePushHttp = Future<ApplePushReply> Function(
  String owner,
  String operation,
  Map<String, Object?> body,
);
typedef ApplePushProof = Future<Map<String, Object?>> Function(
  Map<String, Object?> scope,
);

String _uuid() {
  final random = Random.secure();
  final b = List.generate(16, (_) => random.nextInt(256));
  b[6] = (b[6] & 15) | 64;
  b[8] = (b[8] & 63) | 128;
  final h = b.map((v) => v.toRadixString(16).padLeft(2, '0')).join();
  return '${h.substring(0, 8)}-${h.substring(8, 12)}-${h.substring(12, 16)}-${h.substring(16, 20)}-${h.substring(20)}';
}

bool _id(Object? v) =>
    v is String &&
    RegExp(r'^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$').hasMatch(v);

class _Operation {
  _Operation(this.name, this.body);
  final String name;
  // Kept immutable for response-loss retries; never printed or persisted here.
  final Map<String, Object?> body;
  int calls = 0;
  ApplePushReply? success;
  bool closed = false;
  Future<ApplePushReply?> run(ApplePushHttp http, String owner) async {
    if (success != null) return success;
    if (closed || calls >= 3) return null;
    calls++;
    try {
      final reply = await http(owner, name, Map.of(body));
      if (reply.status >= 200 && reply.status < 300) return success = reply;
      // Never Resolve again after a CAS conflict or manufacture a new intent.
      if (reply.status < 500 && reply.status != 429) closed = true;
    } catch (_) {
      /* No proof, token, backend details or bearer in logs. */
    }
    return null;
  }
}

class _Intent {
  _Intent(this.owner, this.epoch, this.scope);
  final String owner;
  final int epoch;
  final Map<String, Object?> scope;
  Map<String, Object?>? proof;
  final config = _Operation('config', {});
  _Operation? resolve;
  _Operation? bind;
  _Operation? registration;
  _Operation? rotationResolve;
  String? rotationToken;
  _Operation? unbind;
  String? binding;
  String? registrationID;
  bool blocked = false;
}

/// One bounded CAS intent per native account generation. Transient retries keep
/// request_id/input unchanged. 409 never starts another Resolve/Bind. A new OS
/// token updates the existing binding, while logout closes that binding first.
/// The caller serializes registration/removal effects (ApplePush does so).
class ApplePushTransport {
  ApplePushTransport({
    required this.http,
    required this.proof,
    String Function()? requestID,
  }) : requestID = requestID ?? _uuid;
  final ApplePushHttp http;
  final ApplePushProof proof;
  final String Function() requestID;
  String? _owner;
  int _epoch = 0;
  final Map<String, _Intent> _intents = {};

  void followAccount(String? owner) {
    if (_owner == owner) return;
    _owner = owner;
    _epoch++;
  }

  bool _current(_Intent i) => _owner == i.owner && _epoch == i.epoch;
  String _key(Map<String, Object?> r) =>
      '${r['accountID']}/${r['accountGeneration']}/${r['platform']}/${r['bundleID']}/${r['environment']}';

  Future<bool> register(Map<String, Object?> request) async {
    final owner = request['accountID'];
    final token = request['token'];
    if (owner is! String ||
        owner != _owner ||
        request['accountGeneration'] is! String ||
        token is! String ||
        token.length < 2 ||
        token.length > 4096 ||
        token.length.isOdd ||
        !RegExp(r'^[0-9a-fA-F]+$').hasMatch(token) ||
        !['ios', 'macos'].contains(request['platform']) ||
        !['sandbox', 'production'].contains(request['environment']) ||
        request['bundleID'] is! String)
      return false;
    final key = _key(request);
    // Bound retention; old session revocation is the server's durable fence.
    if (!_intents.containsKey(key) && _intents.length >= 64) {
      _intents.removeWhere(
        (_, old) => old.epoch != _epoch && old.unbind?.success != null,
      );
      if (_intents.length >= 64) return false;
    }
    final i = _intents.putIfAbsent(
      key,
      () => _Intent(owner, _epoch, {
        'platform': request['platform'],
        'bundleID': request['bundleID'],
        'environment': request['environment'],
      }),
    );
    if (!_current(i) || i.blocked || i.unbind != null) return false;
    try {
      final config = await i.config.run(http, owner);
      if (!_current(i) || config == null) return false;
      final bundleKey = request['platform'] == 'ios'
          ? 'bundle_id_ios'
          : 'bundle_id_macos';
      if (config.body['configured'] != true) {
        // Bounded capability retry does not mutate a binding.
        i.config.success = null;
        return false;
      }
      if (config.body[bundleKey] != request['bundleID'] ||
          config.body['environments'] is! List ||
          !(config.body['environments'] as List).contains(
            request['environment'],
          ))
        return false;
      i.proof ??= await proof(i.scope);
      if (!_current(i)) return false;
      final p = i.proof!;
      if (!_id(p['installation_id']) ||
          p['installation_secret'] is! String ||
          !RegExp(r'^[A-Za-z0-9_-]{43}$')
              .hasMatch(p['installation_secret'] as String))
        return false;
      i.resolve ??= _Operation('resolve', {
        ...p,
        'platform': request['platform'],
        'bundle_id': request['bundleID'],
        'environment': request['environment'],
      });
      final resolved = await i.resolve!.run(http, owner);
      if (!_current(i) || resolved == null) return false;
      if (resolved.body['installation_id'] != p['installation_id']) {
        i.blocked = true;
        return false;
      }
      if (i.binding == null) {
        if (resolved.body['bound_to_current_session'] == true &&
            resolved.body['bound_to_current_user'] == true) {
          final binding = resolved.body['binding_id'];
          if (!_id(binding)) return false;
          i.binding = binding as String;
          final registration = resolved.body['registration_id'];
          if (registration != null && !_id(registration)) return false;
          i.registrationID = registration as String?;
        } else {
          final expected = resolved.body['binding_id'];
          if (expected != null && !_id(expected)) return false;
          i.bind ??= _Operation('bind', {
            ...p,
            'expected_binding_id': expected,
            'request_id': requestID(),
          });
          final bound = await i.bind!.run(http, owner);
          if (bound == null) {
            if (i.bind!.closed) i.blocked = true;
            return false;
          }
          // Retain the completed old binding for queued logout even if its
          // success is now stale. Never send its next registration step.
          if (bound.body['installation_id'] != p['installation_id'] ||
              !_id(bound.body['binding_id']) ||
              bound.body['bound_to_current_session'] != true ||
              bound.body['bound_to_current_user'] != true) {
            i.blocked = true;
            return false;
          }
          i.binding = bound.body['binding_id'] as String;
          i.registrationID = null;
        }
      }
      if (!_current(i)) return false;
      final prior = i.registration;
      if (prior != null &&
          prior.body['token'] != token &&
          prior.success == null) {
        // A new OS token may reconcile an uncertain prior registration by
        // reading the exact existing binding. This never creates/replaces a
        // binding, and a changed/closed owner stops the intent permanently.
        if (i.rotationToken != token) {
          i.rotationToken = token;
          i.rotationResolve = _Operation('resolve', Map.of(i.resolve!.body));
        }
        final observed = await i.rotationResolve!.run(http, owner);
        if (!_current(i) || observed == null) return false;
        if (observed.body['installation_id'] != p['installation_id'] ||
            observed.body['binding_id'] != i.binding ||
            observed.body['bound_to_current_user'] != true ||
            observed.body['bound_to_current_session'] != true ||
            (observed.body['registration_id'] != null &&
                !_id(observed.body['registration_id']))) {
          i.blocked = true;
          return false;
        }
        i.registrationID = observed.body['registration_id'] as String?;
      }
      if (prior == null || prior.body['token'] != token) {
        i.registration = _Operation('register', {
          ...p,
          'binding_id': i.binding,
          'expected_registration_id': i.registrationID,
          'request_id': requestID(),
          'token': token,
        });
      }
      final registered = await i.registration!.run(http, owner);
      if (registered == null) {
        if (i.registration!.closed) i.blocked = true;
        return false;
      }
      if (!_id(registered.body['registration_id'])) {
        i.blocked = true;
        return false;
      }
      i.registrationID = registered.body['registration_id'] as String;
      return _current(i);
    } catch (_) {
      return false;
    }
  }

  Future<bool> unregister(Map<String, Object?> request) async {
    final i = _intents[_key(request)];
    if (i == null || i.proof == null || i.binding == null) return false;
    i.unbind ??= _Operation('unbind', {...i.proof!, 'binding_id': i.binding});
    final result = await i.unbind!.run(http, i.owner);
    // Never drop a failed close and turn resume into a new binding attempt.
    i.blocked = true;
    return result != null;
  }

  /// Foreground/offline reconciliation, bounded to10 pages and deduplicated
  /// per call. Restarting from0 avoids treating a message ID as an update
  /// sequence (older messages may change after a higher ID was observed).
  Future<bool> updates(String owner) async {
    final epoch = _epoch;
    var after = 0;
    final seen = <int>{};
    for (var page = 0; page < 10; page++) {
      if (_owner != owner || _epoch != epoch) return false;
      try {
        final reply = await http(owner, 'updates', {'after_id': after});
        if (_owner != owner ||
            _epoch != epoch ||
            reply.status != 200 ||
            (reply.body['items'] != null && reply.body['items'] is! List))
          return false;
        for (final raw in (reply.body['items'] as List? ?? const [])) {
          if (raw is! Map || raw['id'] is! int || (raw['id'] as int) <= after)
            return false;
          seen.add(raw['id'] as int);
        }
        if (reply.body['has_more'] == false) return true;
        if (seen.isEmpty || seen.reduce(max) <= after) return false;
        after = seen.reduce(max);
      } catch (_) {
        return false;
      }
    }
    return false;
  }
}
