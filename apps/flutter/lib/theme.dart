// Design tokens of the prototype, all in one place (#387): the unified
// design as signed off on 2026-10-20. Colours for light and dark, shadows,
// type and sizes; pixel values follow the design sources (Main,
// DesktopDark, DesktopNodes, DesktopLogin). Pages take everything from
// here and hard-code none of it.

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

class Tokens {
  const Tokens._({
    required this.brightness,
    required this.bg,
    required this.card,
    required this.sunk,
    required this.hair,
    required this.line,
    required this.t1,
    required this.t2,
    required this.t3,
    required this.accent,
    required this.accent2,
    required this.onAccent,
    required this.accentSoft,
    required this.ok,
    required this.okSoft,
    required this.warn,
    required this.warnSoft,
    required this.bad,
    required this.badSoft,
    required this.onBad,
    required this.sidebar,
    required this.navSelected,
    required this.accountCard,
    required this.switchOff,
    required this.thumbOff,
    required this.hop,
    required this.cardShadow,
    required this.popoverShadow,
    required this.navSelectedShadow,
    required this.avatar,
    required this.onAvatar,
  });

  final Brightness brightness;

  /// Window background.
  final Color bg;

  /// Cards and bars.
  final Color card;

  /// Inset fields: address rows, code boxes, the table header.
  final Color sunk;

  /// The 1 px outline of cards.
  final Color hair;

  /// Dividers and control borders.
  final Color line;

  /// Text: body, secondary, notes.
  final Color t1;
  final Color t2;
  final Color t3;

  final Color accent;

  /// The lighter accent: the avatar gradient, the upload line.
  final Color accent2;
  final Color onAccent;
  final Color accentSoft;

  /// Connected, latency under 100 ms.
  final Color ok;
  final Color okSoft;

  /// Latency 100–199 ms, notices.
  final Color warn;
  final Color warnSoft;

  /// Errors, latency 200 ms and more, the unread badge.
  final Color bad;
  final Color badSoft;

  /// Text on [bad] (the badge's digits).
  final Color onBad;

  final Color sidebar;
  final Color navSelected;
  final Color accountCard;
  final Color switchOff;
  final Color thumbOff;

  /// A route hop's fill.
  final Color hop;

  final List<BoxShadow> cardShadow;
  final List<BoxShadow> popoverShadow;
  final List<BoxShadow> navSelectedShadow;

  /// The avatar's gradient and its letter.
  final List<Color> avatar;
  final Color onAvatar;

  static const light = Tokens._(
    brightness: Brightness.light,
    bg: Color(0xFFF3F5F8),
    card: Color(0xFFFFFFFF),
    sunk: Color(0xFFF6F7F9),
    hair: Color(0x0F0F172A),
    line: Color(0xFFE3E6EB),
    t1: Color(0xFF14171C),
    t2: Color(0xFF4B5361),
    t3: Color(0xFF646C78),
    accent: Color(0xFF1F4390),
    accent2: Color(0xFF3D63C9),
    onAccent: Color(0xFFFFFFFF),
    accentSoft: Color(0x171F4390),
    ok: Color(0xFF15803D),
    okSoft: Color(0x1A15803D),
    warn: Color(0xFFB45309),
    warnSoft: Color(0x1AB45309),
    bad: Color(0xFFBA1A1A),
    badSoft: Color(0x17BA1A1A),
    onBad: Color(0xFFFFFFFF),
    sidebar: Color(0xFFECEFF4),
    navSelected: Color(0xFFFFFFFF),
    accountCard: Color(0xA6FFFFFF),
    switchOff: Color(0xFFD5D9E0),
    thumbOff: Color(0xFFFFFFFF),
    hop: Color(0xE6FFFFFF),
    cardShadow: [
      BoxShadow(color: Color(0x0A0F172A), offset: Offset(0, 1), blurRadius: 2),
      BoxShadow(color: Color(0x0D0F172A), offset: Offset(0, 2), blurRadius: 8),
    ],
    popoverShadow: [
      BoxShadow(color: Color(0x0D0F172A), offset: Offset(0, 1), blurRadius: 2),
      BoxShadow(color: Color(0x120F172A), offset: Offset(0, 8), blurRadius: 24),
    ],
    navSelectedShadow: [BoxShadow(color: Color(0x0F0F172A), offset: Offset(0, 1), blurRadius: 2)],
    avatar: [Color(0xFF3D63C9), Color(0xFF1F4390)],
    onAvatar: Color(0xFFFFFFFF),
  );

  /// Dark: cards rely on their outline; the shadow only darkens below.
  static const dark = Tokens._(
    brightness: Brightness.dark,
    bg: Color(0xFF0D0F13),
    card: Color(0xFF161A21),
    sunk: Color(0xFF11141A),
    hair: Color(0x12FFFFFF),
    line: Color(0xFF2A2F37),
    t1: Color(0xFFE8EAED),
    t2: Color(0xFFB0B7C1),
    t3: Color(0xFF8F97A2),
    accent: Color(0xFFB4C5FF),
    accent2: Color(0xFF8FA8F5),
    onAccent: Color(0xFF002A78),
    accentSoft: Color(0x21B4C5FF),
    ok: Color(0xFF4ADE80),
    okSoft: Color(0x214ADE80),
    warn: Color(0xFFFFB59D),
    warnSoft: Color(0x21FFB59D),
    bad: Color(0xFFFFB4AB),
    badSoft: Color(0x21FFB4AB),
    onBad: Color(0xFF690005),
    sidebar: Color(0xFF12151B),
    navSelected: Color(0xFF232831),
    accountCard: Color(0x0AFFFFFF),
    switchOff: Color(0xFF363C46),
    thumbOff: Color(0xFFC9CED6),
    hop: Color(0x0AFFFFFF),
    cardShadow: [BoxShadow(color: Color(0x59000000), offset: Offset(0, 2), blurRadius: 10)],
    popoverShadow: [BoxShadow(color: Color(0x73000000), offset: Offset(0, 8), blurRadius: 24)],
    navSelectedShadow: [],
    avatar: [Color(0xFFB4C5FF), Color(0xFF7F9CF0)],
    onAvatar: Color(0xFF002A78),
  );

  /// Tier tag 「优化」: teal on 10 %. 「专线」 uses the accent.
  static const tierOptimized = Color(0xFF0F766E);

  /// The primary button's gradient, top to bottom.
  static const primaryGradient = LinearGradient(
    begin: Alignment.topCenter,
    end: Alignment.bottomCenter,
    colors: [Color(0xFF2A52A8), Color(0xFF1F4390)],
  );

  /// The brand gradient (sign-in panel), CSS 160°.
  static const brandGradient = LinearGradient(
    begin: Alignment(-0.34, -0.94),
    end: Alignment(0.34, 0.94),
    stops: [0, 0.45, 1],
    colors: [Color(0xFF2C56B4), Color(0xFF1F4390), Color(0xFF14306E)],
  );

  /// The connected orb, CSS 145°.
  static const connectedOrb = LinearGradient(
    begin: Alignment(-0.57, -0.82),
    end: Alignment(0.57, 0.82),
    colors: [Color(0xFF22A052), Color(0xFF15803D)],
  );

  // --- sizes ---------------------------------------------------------------

  static const windowSize = Size(960, 640);
  static const windowMinSize = Size(760, 540);

  /// Content header height; the macOS traffic lights sit in the sidebar's.
  static const topBarHeight = 52.0;
  static const captionButtonWidth = 46.0;

  static const sidebarWidth = 200.0;
  static const sidebarPadding = EdgeInsets.fromLTRB(10, 0, 10, 12);

  /// The traffic lights' row at the top of the sidebar.
  static const sidebarLightsHeight = 44.0;
  static const navItemHeight = 34.0;
  static const navGap = 2.0;

  /// Page content: the overview's column (max 720, cards 14 apart).
  static const pagePadding = EdgeInsets.fromLTRB(20, 2, 20, 20);
  static const pageInset = 24.0;
  static const pageMaxWidth = 720.0;
  static const cardGap = 14.0;

  static const controlHeight = 32.0;
  static const primaryHeight = 36.0;
  static const rowHeight = 58.0;
  static const tableRowHeight = 42.0;
  static const tableHeaderHeight = 34.0;

  static const cardRadius = 12.0;
  static const controlRadius = 8.0;
  static const segmentRadius = 7.0;

  static const switchSize = Size(36, 20);
  static const flagSize = Size(20, 15);
  static const orbSize = 52.0;
  static const rowIconSize = 32.0;

  // Sign-in (DesktopLogin).
  static const signInBrandWidth = 380.0;
  static const signInBrandPadding = EdgeInsets.fromLTRB(32, 16, 32, 28);
  static const trafficLightsHeight = 12.0;
  static const signInPadding = 56.0;
  static const codeBox = Size(40, 52);
  static const codeFontSize = 26.0;
  static const codeGap = 6.0;
  static const signInMountains = 150.0;

  // --- type ----------------------------------------------------------------

  /// Latin from Inter (forui bundles it); Chinese from the system, no CJK
  /// font bundled. Digits are tabular everywhere.
  static const sans = 'packages/forui/Inter';
  static const cjk = ['PingFang SC', 'Microsoft YaHei UI', 'Noto Sans CJK SC', 'Noto Sans SC'];

  /// Numbers and addresses (bundled: assets/fonts).
  static const mono = 'JetBrains Mono';

  TextStyle _text(double size, double height, FontWeight weight, Color color, {double tracking = 0}) => TextStyle(
    fontFamily: sans,
    fontFamilyFallback: cjk,
    fontSize: size,
    height: height / size,
    fontWeight: weight,
    letterSpacing: tracking * size,
    color: color,
    fontFeatures: const [FontFeature.tabularFigures()],
  );

  /// Body 14/20.
  TextStyle get body => _text(14, 20, FontWeight.w400, t1);
  TextStyle get bodyStrong => _text(14, 20, FontWeight.w600, t1);

  /// Row titles, 14/20 · 500.
  TextStyle get rowTitle => _text(14, 20, FontWeight.w500, t1);

  /// Content header title, 17/24 · 700.
  TextStyle get pageTitle => _text(17, 24, FontWeight.w700, t1, tracking: -0.01);

  /// The connection card's status, 24/30 · 700.
  TextStyle get heroTitle => _text(24, 30, FontWeight.w700, t1, tracking: -0.02);

  /// Sign-in heading, 26/32 · 700.
  TextStyle get signInTitle => _text(26, 32, FontWeight.w700, t1, tracking: -0.02);

  /// 13 · 500: nav items, dropdowns, buttons.
  TextStyle get control => _text(13, 18, FontWeight.w500, t2);
  TextStyle get sectionTitle => _text(13, 18, FontWeight.w600, t1);
  TextStyle get sub => _text(13, 18, FontWeight.w400, t2);

  /// Secondary, 12/16.
  TextStyle get note => _text(12, 16, FontWeight.w400, t3);

  /// Labels and badges, 11/16 · 600.
  TextStyle get label => _text(11, 16, FontWeight.w600, t1);

  TextStyle monoText(double size, {FontWeight weight = FontWeight.w400, Color? color, double? height}) => TextStyle(
    fontFamily: mono,
    fontFamilyFallback: cjk,
    fontSize: size,
    height: height == null ? null : height / size,
    fontWeight: weight,
    color: color ?? t1,
  );

  /// Rates, 22/28 · 600.
  TextStyle get rate => monoText(22, weight: FontWeight.w600, height: 28).copyWith(letterSpacing: -0.44);

  /// forui's neutral theme on our colours.
  FThemeData get forui {
    final base = brightness == Brightness.light ? FColors.neutralLight : FColors.neutralDark;
    return FThemeData(
      touch: false,
      colors: base.copyWith(
        primary: accent,
        primaryForeground: onAccent,
        background: bg,
        foreground: t1,
        border: line,
        card: card,
        secondary: accentSoft,
        secondaryForeground: accent,
        muted: sunk,
        mutedForeground: t3,
        destructive: bad,
        error: bad,
      ),
    );
  }

  static Tokens of(BuildContext context) =>
      MediaQuery.platformBrightnessOf(context) == Brightness.dark ? dark : light;

  /// A card: radius 12, the 1 px hair outline, the card shadow.
  BoxDecoration cardDecoration({Color? color, Gradient? gradient}) => BoxDecoration(
    color: gradient == null ? (color ?? card) : null,
    gradient: gradient,
    borderRadius: BorderRadius.circular(cardRadius),
    border: Border.all(color: hair),
    boxShadow: cardShadow,
  );
}

/// A card ([Tokens.cardDecoration]) around [child].
class CardBox extends StatelessWidget {
  const CardBox({super.key, required this.child, this.padding = EdgeInsets.zero});

  final Widget child;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) => Container(
    padding: padding,
    clipBehavior: Clip.antiAlias,
    decoration: Tokens.of(context).cardDecoration(),
    child: child,
  );
}
