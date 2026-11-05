The PP mark is copied unchanged from ppvpn-web `apps/web/public/img/icon.png`
(commit 4e419bc2dc33ae4a5f3d4430dc1b44a9266d510b). It is the shared Web
identity, not the mountain symbol previously drawn by this client.

`lib/widgets.dart` uses it directly; `scripts/app-icons.swift` renders the same
mark onto the opaque iOS icon canvas. Android launcher and launch assets use
the same bitmap. Desktop launcher assets already use the PP mark.
