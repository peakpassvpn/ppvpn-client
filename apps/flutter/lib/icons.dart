// Material Symbols Rounded, from the same pinned source as @ppvpn/design.
// Web semantic names are kept here so both shells use the same symbols.
import 'package:flutter/widgets.dart';

abstract final class AppIcons {
  static const arrowDown = IconData(
    0xe000,
    fontFamily: 'PPVPN Material',
  ); // arrow_downward
  static const bell = IconData(
    0xe001,
    fontFamily: 'PPVPN Material',
  ); // notifications
  static const calendar = IconData(
    0xe002,
    fontFamily: 'PPVPN Material',
  ); // calendar_month
  static const calendarClock = IconData(
    0xe003,
    fontFamily: 'PPVPN Material',
  ); // event_upcoming
  static const calendarX = IconData(
    0xe004,
    fontFamily: 'PPVPN Material',
  ); // event_busy
  static const check = IconData(0xe005, fontFamily: 'PPVPN Material'); // check
  static const checkCheck = IconData(
    0xe006,
    fontFamily: 'PPVPN Material',
  ); // done_all
  static const chevronDown = IconData(
    0xe007,
    fontFamily: 'PPVPN Material',
  ); // expand_more
  static const chevronLeft = IconData(
    0xe008,
    fontFamily: 'PPVPN Material',
  ); // chevron_left
  static const chevronRight = IconData(
    0xe009,
    fontFamily: 'PPVPN Material',
  ); // chevron_right
  static const chevronUp = IconData(
    0xe00a,
    fontFamily: 'PPVPN Material',
  ); // expand_less
  static const circleAlert = IconData(
    0xe00b,
    fontFamily: 'PPVPN Material',
  ); // error
  static const circleCheck = IconData(
    0xe00c,
    fontFamily: 'PPVPN Material',
  ); // check_circle
  static const clock = IconData(
    0xe00d,
    fontFamily: 'PPVPN Material',
  ); // schedule
  static const creditCard = IconData(
    0xe00e,
    fontFamily: 'PPVPN Material',
  ); // credit_card
  static const download = IconData(
    0xe00f,
    fontFamily: 'PPVPN Material',
  ); // download
  static const folderOpen = IconData(
    0xe010,
    fontFamily: 'PPVPN Material',
  ); // folder_open
  static const globe = IconData(0xe011, fontFamily: 'PPVPN Material'); // public
  static const inbox = IconData(0xe012, fontFamily: 'PPVPN Material'); // inbox
  static const info = IconData(0xe013, fontFamily: 'PPVPN Material'); // info
  static const languages = IconData(
    0xe014,
    fontFamily: 'PPVPN Material',
  ); // translate
  static const link = IconData(0xe015, fontFamily: 'PPVPN Material'); // link
  static const link2Off = IconData(
    0xe016,
    fontFamily: 'PPVPN Material',
  ); // link_off
  static const list = IconData(
    0xe017,
    fontFamily: 'PPVPN Material',
  ); // list_alt
  static const logOut = IconData(
    0xe018,
    fontFamily: 'PPVPN Material',
  ); // logout
  static const mailOpen = IconData(
    0xe019,
    fontFamily: 'PPVPN Material',
  ); // drafts
  static const mapPin = IconData(
    0xe01a,
    fontFamily: 'PPVPN Material',
  ); // location_on
  static const megaphone = IconData(
    0xe01b,
    fontFamily: 'PPVPN Material',
  ); // campaign
  static const minus = IconData(0xe01c, fontFamily: 'PPVPN Material'); // remove
  static const monitor = IconData(
    0xe01d,
    fontFamily: 'PPVPN Material',
  ); // desktop_windows
  static const moon = IconData(
    0xe01e,
    fontFamily: 'PPVPN Material',
  ); // dark_mode
  static const power = IconData(
    0xe01f,
    fontFamily: 'PPVPN Material',
  ); // power_settings_new
  static const receipt = IconData(
    0xe020,
    fontFamily: 'PPVPN Material',
  ); // receipt_long
  static const route = IconData(0xe021, fontFamily: 'PPVPN Material'); // route
  static const search = IconData(
    0xe022,
    fontFamily: 'PPVPN Material',
  ); // search
  static const server = IconData(0xe023, fontFamily: 'PPVPN Material'); // dns
  static const settings = IconData(
    0xe024,
    fontFamily: 'PPVPN Material',
  ); // settings
  static const shield = IconData(
    0xe025,
    fontFamily: 'PPVPN Material',
  ); // verified_user
  static const shoppingBag = IconData(
    0xe026,
    fontFamily: 'PPVPN Material',
  ); // shopping_bag
  static const split = IconData(
    0xe027,
    fontFamily: 'PPVPN Material',
  ); // call_split
  static const square = IconData(0xe028, fontFamily: 'PPVPN Material'); // stop
  static const sunMoon = IconData(
    0xe029,
    fontFamily: 'PPVPN Material',
  ); // contrast
  static const user = IconData(0xe02a, fontFamily: 'PPVPN Material'); // person
  static const userX = IconData(
    0xe02b,
    fontFamily: 'PPVPN Material',
  ); // person_remove
  static const users = IconData(0xe02c, fontFamily: 'PPVPN Material'); // group
  static const x = IconData(0xe02d, fontFamily: 'PPVPN Material'); // close
  static const zap = IconData(0xe02e, fontFamily: 'PPVPN Material'); // bolt
  static const arrowUpRight = IconData(
    0xe02f,
    fontFamily: 'PPVPN Material',
  ); // north_east
  static const mail = IconData(0xe030, fontFamily: 'PPVPN Material'); // mail
  static const copy = IconData(
    0xe031,
    fontFamily: 'PPVPN Material',
  ); // content_copy
}

IconData filledIcon(IconData icon) => switch (icon) {
  AppIcons.shield => const IconData(
    0xe025,
    fontFamily: 'PPVPN Material Filled',
  ),
  AppIcons.globe => const IconData(0xe011, fontFamily: 'PPVPN Material Filled'),
  AppIcons.user => const IconData(0xe02a, fontFamily: 'PPVPN Material Filled'),
  _ => icon,
};
