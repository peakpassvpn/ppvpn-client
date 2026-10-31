#!/bin/sh
# deb prerm (remove | upgrade | deconfigure ...) and rpm %preun (0 = erase, 1 = upgrade).
set -e

# Running push agents keep the old binary; nothing should be left behind. They come back
# with the next login or app start. (/proc directly: procps is not installed everywhere.)
for cmdline in /proc/[0-9]*/cmdline; do
  case "$(tr '\0' ' ' < "$cmdline" 2>/dev/null || true)" in
    /usr/lib/ppvpn/ppvpn-push-agent*)
      pid="${cmdline#/proc/}"
      kill -TERM "${pid%/cmdline}" 2>/dev/null || true
      ;;
  esac
done

# On removal (not upgrade), take the Enhanced Mode service down with the app:
# nothing else could remove it once the helpers are gone.
case "$1" in
  remove | purge | 0)
    if [ -x /usr/lib/ppvpn/ppvpn-service-uninstall ]; then
      /usr/lib/ppvpn/ppvpn-service-uninstall || true
    fi
    ;;
esac

exit 0
