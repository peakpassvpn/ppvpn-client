#!/bin/sh
# Runs a command with the host's D-Bus out of reach, for a test core in a
# network namespace: run as `ip netns exec <ns> sh no-host-dbus.sh <cmd>`.
# D-Bus is not isolated by network namespaces, so a core that sets its
# TUN's DNS through systemd-resolved would, from a namespace, give the host's
# resolved the namespace's interface index: the host interface with that
# index would get the TUN's DNS, and keep it after a kill. `ip netns exec`
# runs in a mount namespace of its own; an empty tmpfs over /run/dbus there
# hides the system bus from the command and nothing else; no bus address
# is passed either (as Desktop's tests do, #162).
mount -t tmpfs -o size=64k,mode=0755 no-host-dbus /run/dbus 2>/dev/null || true
unset DBUS_SYSTEM_BUS_ADDRESS DBUS_SESSION_BUS_ADDRESS
exec "$@"
