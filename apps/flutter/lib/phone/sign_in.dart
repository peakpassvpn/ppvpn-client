// The phone sign-in (#448 step 5): the brand on top (the brand gradient
// under the status bar, the mark, 「PPVPN」 and the title line), then the
// desktop's sign-in content at the page padding; the device code's boxes
// shrink to the width there.

import 'package:flutter/widgets.dart';

import '../app_state.dart';
import '../pages/sign_in.dart';
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';

class PhoneSignIn extends StatelessWidget {
  const PhoneSignIn({super.key, required this.app});

  final AppState app;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    const white = Color(0xFFFFFFFF);
    final top = MediaQuery.paddingOf(context).top;
    return ColoredBox(
      color: t.card,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: EdgeInsets.fromLTRB(Tokens.phonePadding, top + 20, Tokens.phonePadding, 24),
            decoration: const BoxDecoration(gradient: Tokens.brandGradient),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    const BrandMark(size: 36),
                    const SizedBox(width: 10),
                    Text(S.current('app'), style: t.phoneTitle.copyWith(fontSize: 22, height: 28 / 22, color: white)),
                  ],
                ),
                const SizedBox(height: 16),
                Semantics(
                  header: true,
                  child: Text(S.current('signInTitle'), style: t.heroTitle.copyWith(color: white)),
                ),
                const SizedBox(height: 6),
                Text(S.current('signInLine'), style: t.phoneNote.copyWith(color: const Color(0xC7FFFFFF))),
              ],
            ),
          ),
          Expanded(
            child: MediaQuery.removePadding(
              context: context,
              removeTop: true,
              child: SignInPage(app: app, inset: Tokens.phonePadding),
            ),
          ),
        ],
      ),
    );
  }
}
