// The phone app starts (#448 step 3): the bridge library loads (from the
// APK; on iOS it is linked in), the client starts in the app's own
// directory, and a fresh install shows the sign-in page. Run by
// e2e/android_smoke.sh on ci.yml's emulator, as a tablet in landscape (the
// desktop layout) and as a phone in portrait (the phone layout, step 5),
// and by e2e/ios_smoke.sh on an iPhone simulator.

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:ppvpn_flutter/main.dart' as app;

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
}
