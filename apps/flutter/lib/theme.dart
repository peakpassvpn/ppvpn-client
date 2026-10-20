// Design tokens of the prototype, in one place (#387). From the desktop
// design handoff: the brand and semantic colours as they are; sizes,
// spacing, radii and the window size are the macOS column, used on every
// platform. Controls are forui's neutral ones. A cross-platform design
// (desktop and mobile) is being drafted and will replace these values.

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

class Tokens {
  const Tokens._({
    required this.brightness,
    required this.accent,
    required this.onAccent,
    required this.accentSoft,
    required this.success,
    required this.successSoft,
    required this.warning,
    required this.warningSoft,
    required this.danger,
    required this.dangerSoft,
    required this.idle,
    required this.window,
    required this.group,
    required this.border,
    required this.text,
    required this.textSecondary,
    required this.textSubtle,
    required this.sidebar,
    required this.badge,
    required this.onBadge,
  });

  final Brightness brightness;

  // Brand and semantic colours (handoff "Design Tokens").
  final Color accent;
  final Color onAccent;
  final Color accentSoft;
  final Color success;
  final Color successSoft;
  final Color warning;
  final Color warningSoft;
  final Color danger;
  final Color dangerSoft;

  /// Not connected, timeouts.
  final Color idle;

  // Surfaces: the handoff's illustrative macOS values (the native apps use
  // the system's; Flutter draws its own).
  final Color window;
  final Color group;
  final Color border;
  final Color text;
  final Color textSecondary;

  /// Row descriptions and notes: the weakest text.
  final Color textSubtle;

  final Color sidebar;

  /// Unread badge and its digits.
  final Color badge;
  final Color onBadge;

  static const light = Tokens._(
    brightness: Brightness.light,
    accent: Color(0xFF1F4390),
    onAccent: Color(0xFFFFFFFF),
    accentSoft: Color(0x1F1F4390),
    success: Color(0xFF16A34A),
    successSoft: Color(0x1F16A34A),
    warning: Color(0xFFB45309),
    warningSoft: Color(0x21D97706),
    danger: Color(0xFFBA1A1A),
    dangerSoft: Color(0x1ABA1A1A),
    idle: Color(0x8C000000),
    window: Color(0xFFEFEFEF),
    group: Color(0xFFFFFFFF),
    border: Color(0x1A000000),
    text: Color(0xD9000000),
    textSecondary: Color(0x8C000000),
    textSubtle: Color(0x66000000),
    sidebar: Color(0xFFE6E6E6),
    badge: Color(0xFFBA1A1A),
    onBadge: Color(0xFFFFFFFF),
  );

  static const dark = Tokens._(
    brightness: Brightness.dark,
    accent: Color(0xFFB4C5FF),
    onAccent: Color(0xFF002A78),
    accentSoft: Color(0x29B4C5FF),
    success: Color(0xFF4ADE80),
    successSoft: Color(0x244ADE80),
    warning: Color(0xFFFFB59D),
    warningSoft: Color(0x24FFB59D),
    danger: Color(0xFFFFB4AB),
    dangerSoft: Color(0x24FFB4AB),
    idle: Color(0x8CFFFFFF),
    window: Color(0xFF1F1F1F),
    group: Color(0xFF2A2A2A),
    border: Color(0x1FFFFFFF),
    text: Color(0xE6FFFFFF),
    textSecondary: Color(0x8CFFFFFF),
    textSubtle: Color(0x66FFFFFF),
    sidebar: Color(0xFF262626),
    badge: Color(0xFFFFB4AB),
    onBadge: Color(0xFF690005),
  );

  // Sizes (macOS column of the handoff).
  // Design v17: 960×640 (the sign-in page's two columns).
  static const windowSize = Size(960, 640);
  static const windowMinSize = Size(640, 460);
  /// The strip above the content (and above the sidebar items): macOS's
  /// unified toolbar height, where the traffic lights sit.
  static const topBarHeight = 52.0;
  static const captionButtonWidth = 46.0;

  // Sidebar navigation (#387): 200 wide, 34-high items.
  static const sidebarWidth = 200.0;
  static const sidebarItemHeight = 34.0;
  static const contentPadding = EdgeInsets.symmetric(vertical: 20, horizontal: 24);
  static const sectionGap = 18.0;
  static const rowMinHeight = 28.0;
  static const controlHeight = 22.0;
  static const largeControlHeight = 30.0;
  static const switchSize = Size(32, 18);
  static const groupRadius = 10.0;
  static const buttonRadius = 6.0;
  static const menuItemHeight = 22.0;
  static const statusIconSize = 44.0;

  // Sign-in (design v17): 56 either side of the content, a 40×52 box per
  // device-code character at 26.
  static const signInPadding = 56.0;
  static const codeBox = Size(40, 52);
  static const codeFontSize = 26.0;
  static const codeGap = 6.0;

  // The sign-in brand panel (design v17).
  static const signInBrandWidth = 380.0;
  static const signInBrandPadding = EdgeInsets.fromLTRB(32, 16, 32, 28);
  static const trafficLightsHeight = 12.0;

  /// Room below the panel's text for the mountains.
  static const signInMountains = 150.0;
  static const brandIconSize = 76.0;

  // Type (macOS: 13 body, 20 bold status title, 11 row descriptions).
  static const fontSize = 13.0;
  static const monoFamily = 'monospace';

  TextStyle get body => TextStyle(fontSize: fontSize, height: 1.3, color: text);
  TextStyle get statusTitle => TextStyle(fontSize: 20, fontWeight: FontWeight.w700, height: 1.2, color: text);
  TextStyle get nodeName => TextStyle(fontSize: fontSize, fontWeight: FontWeight.w600, color: text);
  TextStyle get rowTitle => TextStyle(fontSize: fontSize, color: text);
  TextStyle get secondary => TextStyle(fontSize: fontSize, color: textSecondary);
  TextStyle get note => TextStyle(fontSize: 11, color: textSubtle);
  TextStyle get sectionTitle => TextStyle(fontSize: fontSize, fontWeight: FontWeight.w600, color: text);
  TextStyle get rate => TextStyle(fontSize: 22, fontFamily: monoFamily, fontFeatures: const [FontFeature.tabularFigures()], color: text);
  TextStyle get mono => TextStyle(fontSize: 12, fontFamily: monoFamily, color: text);

  /// forui's neutral theme with our accent as its primary colour.
  FThemeData get forui {
    final base = brightness == Brightness.light ? FColors.neutralLight : FColors.neutralDark;
    return FThemeData(
      touch: false,
      colors: base.copyWith(
        primary: accent,
        primaryForeground: onAccent,
        background: window,
        foreground: text,
        border: border,
        card: group,
        destructive: danger,
        error: danger,
      ),
    );
  }

  static Tokens of(BuildContext context) =>
      MediaQuery.platformBrightnessOf(context) == Brightness.dark ? dark : light;
}

/// A grouped section: the macOS form group (radius 10, hairline border).
class Group extends StatelessWidget {
  const Group({super.key, required this.child, this.padding = const EdgeInsets.symmetric(horizontal: 12, vertical: 4)});

  final Widget child;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Container(
      padding: padding,
      decoration: BoxDecoration(
        color: t.group,
        borderRadius: BorderRadius.circular(Tokens.groupRadius),
        border: Border.all(color: t.border, width: 0.5),
      ),
      child: child,
    );
  }
}
