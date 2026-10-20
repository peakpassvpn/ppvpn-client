// Sign-in (handoff "1. 登录"): signed out, waiting for the browser with the
// device code and its countdown, and the three errors.

import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app_state.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';

class SignInPage extends StatelessWidget {
  const SignInPage({super.key, required this.app});

  final AppState app;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) {
      final code = snapshot.deviceCode;
      final Widget body;
      if (snapshot.auth == rust.UiAuth.restoring) {
        body = Spinner(color: Tokens.of(context).accent, size: 20);
      } else if (snapshot.auth == rust.UiAuth.awaitingBrowser && code != null) {
        body = _Waiting(app: app, code: code);
      } else {
        body = _SignedOut(app: app, error: _signInError(snapshot.error));
      }
      return Center(
        child: SingleChildScrollView(
          padding: Tokens.contentPadding,
          child: ConstrainedBox(constraints: const BoxConstraints(maxWidth: 420), child: body),
        ),
      );
    },
  );
}

enum _SignInError { expired, denied, network }

_SignInError? _signInError(rust.UiError? error) => switch (error?.code) {
  'AuthExpired' => _SignInError.expired,
  'AuthDenied' => _SignInError.denied,
  'NetworkUnreachable' || 'ServerUnavailable' => _SignInError.network,
  _ => null,
};

class BrandIcon extends StatelessWidget {
  const BrandIcon({super.key, this.size = Tokens.brandIconSize});

  final double size;

  @override
  Widget build(BuildContext context) => Container(
    width: size,
    height: size,
    alignment: Alignment.center,
    decoration: BoxDecoration(
      borderRadius: BorderRadius.circular(size * 0.24),
      gradient: const LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [Color(0xFF1F4390), Color(0xFF3B5BA9)],
      ),
    ),
    // Placeholder, as in the handoff: the brand double-P mark replaces it.
    child: Text(
      'PP',
      style: TextStyle(fontSize: size * 0.36, fontWeight: FontWeight.w800, color: const Color(0xFFFFFFFF)),
    ),
  );
}

class _SignedOut extends StatelessWidget {
  const _SignedOut({required this.app, required this.error});

  final AppState app;
  final _SignInError? error;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final (title, description) = switch (error) {
      null => (s('loginTitle'), s('loginSub')),
      _SignInError.expired => (s('errExpiredT'), s('errExpiredD')),
      _SignInError.denied => (s('errDeniedT'), s('errDeniedD')),
      _SignInError.network => (s('errNetT'), s('errNetD')),
    };
    final button = switch (error) {
      null => s('loginBtn'),
      _SignInError.network => s('tryAgain'),
      _ => s('signInAgain'),
    };
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        error == null
            ? const BrandIcon()
            : Icon(FLucideIcons.circleAlert, size: 44, color: error == _SignInError.network ? t.warning : t.danger),
        const SizedBox(height: 16),
        Semantics(header: true, child: Text(title, style: t.statusTitle, textAlign: TextAlign.center)),
        const SizedBox(height: 6),
        Text(description, style: t.secondary, textAlign: TextAlign.center),
        const SizedBox(height: 20),
        SizedBox(
          height: Tokens.largeControlHeight,
          child: FButton(onPress: app.signIn, mainAxisSize: MainAxisSize.min, child: Text(button)),
        ),
      ],
    );
  }
}

class _Waiting extends StatefulWidget {
  const _Waiting({required this.app, required this.code});

  final AppState app;
  final rust.UiDeviceCode code;

  @override
  State<_Waiting> createState() => _WaitingState();
}

class _WaitingState extends State<_Waiting> {
  late DateTime _expires;
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _start();
  }

  @override
  void didUpdateWidget(_Waiting old) {
    super.didUpdateWidget(old);
    if (old.code.userCode != widget.code.userCode) _start();
  }

  void _start() {
    _expires = DateTime.now().add(Duration(seconds: widget.code.expiresInSecs));
    _tick?.cancel();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final left = _expires.difference(DateTime.now());
    final seconds = left.isNegative ? 0 : left.inSeconds;
    final countdown = '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Semantics(header: true, child: Text(s('waitTitle'), style: t.statusTitle, textAlign: TextAlign.center)),
        const SizedBox(height: 6),
        Text(s('waitSub'), style: t.secondary, textAlign: TextAlign.center),
        const SizedBox(height: 20),
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              widget.code.userCode,
              style: TextStyle(
                fontFamily: Tokens.monoFamily,
                fontSize: 36,
                fontWeight: FontWeight.w600,
                letterSpacing: 3.6,
                color: t.text,
              ),
            ),
            const SizedBox(width: 12),
            CopyButton(text: widget.code.userCode),
          ],
        ),
        const SizedBox(height: 8),
        Text(
          seconds == 0 ? s('errExpiredT') : s('expiresIn', {'t': countdown}),
          style: t.note.copyWith(color: seconds == 0 ? t.danger : null),
        ),
        const SizedBox(height: 20),
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            SizedBox(
              height: Tokens.largeControlHeight,
              child: FButton(
                variant: FButtonVariant.outline,
                mainAxisSize: MainAxisSize.min,
                onPress: widget.app.cancelSignIn,
                child: Text(s('cancel')),
              ),
            ),
            const SizedBox(width: 8),
            SizedBox(
              height: Tokens.largeControlHeight,
              child: FButton(
                mainAxisSize: MainAxisSize.min,
                onPress: () => rust.openUrl(url: widget.code.verificationUrl),
                child: Text(s('reopen')),
              ),
            ),
          ],
        ),
      ],
    );
  }
}
