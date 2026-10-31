// Network diagnostics (#521 step A): what the system resolver and the
// domestic DoH resolvers (AliDNS, DNSPod) answer for the service's API
// host and the profile's node hosts, and whether each address completes
// TCP and TLS (rust/src/api.rs diagnose_reachability, ppvpn-account
// reachability). The DoH resolvers are asked only when the user presses
// the button here; the intro says so. The report is plain text, monospace,
// with a copy button for pasting into a ticket. It changes nothing.

import 'dart:async';

import 'package:flutter/widgets.dart';

import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';

class DiagnosticsView extends StatefulWidget {
  const DiagnosticsView({super.key});

  @override
  State<DiagnosticsView> createState() => _DiagnosticsViewState();
}

class _DiagnosticsViewState extends State<DiagnosticsView> {
  bool _running = false;
  String? _report;

  Future<void> _run() async {
    setState(() => _running = true);
    String report;
    try {
      report = await rust.diagnoseReachability();
    } catch (error) {
      report = '$error';
    }
    if (!mounted) return;
    setState(() {
      _running = false;
      _report = report;
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final report = _report;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(s('diagnosticsIntro'), style: t.sub),
        const SizedBox(height: 12),
        Row(
          children: [
            Button(
              kind: ButtonKind.primary,
              height: Tokens.controlHeight,
              label: _running ? s('diagnosticsRunning') : s('diagnosticsRun'),
              onPress: _running ? null : () => unawaited(_run()),
            ),
            const Spacer(),
            if (report != null && !_running) CopyButton(text: report),
          ],
        ),
        if (report != null) ...[
          const SizedBox(height: 12),
          Container(
            padding: const EdgeInsets.all(12),
            decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(8)),
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Text(report, style: t.logLine),
            ),
          ),
        ],
      ],
    );
  }
}
