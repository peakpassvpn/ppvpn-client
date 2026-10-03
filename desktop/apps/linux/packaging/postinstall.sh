#!/bin/sh
# deb postinst (configure) and rpm %post (1 = install, 2 = upgrade).
set -e

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true

# An Enhanced Mode service installed from an older package still runs the old
# ppvpn-service and ppvpn-core copies in /usr/lib/ppvpn-service: replace them.
if [ -f /etc/systemd/system/ppvpn-service.service ] && [ -x /usr/lib/ppvpn/ppvpn-service-install ]; then
  /usr/lib/ppvpn/ppvpn-service-install \
    || echo "ppvpn: could not update the system service; turning Enhanced Mode on reinstalls it" >&2
fi

exit 0
