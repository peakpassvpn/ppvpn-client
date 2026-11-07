# Read-only route queries within the capture budget

Run42397425398/source9f6ce0e5 failed its first attach on a fresh resource with
ROUTE_QUERY_TIMEOUT. Its old scoped-route list and published split-route flags
were empty, so source order identifies the first physical scoped-default query.
The external child did not settle in its existing post-spawn300ms budget. The
subprocess's internal cause is unknown. The earlier7f cycle16 detach failure
still lacks this detail and is not retroactively assigned the same cause.

Queries now use Darwin's read-only NET_RT_DUMP2 sysctl and rt_msghdr2 records.
The ABI comes from the installed Apple SDK net/route.h/sys/socket.h; sockaddr
slots have32-bit alignment, matching Apple's netstat source:
https://github.com/apple-oss-distributions/network_cmds/blob/main/netstat.tproj/route.c

The query budget remains300ms, including snapshot reading and parsing; Core3s,
client2400ms and broker600ms remain unchanged. A16MiB ceiling and64KiB headroom
bound allocation without retrying failed queries. Errors, malformed/unsupported
records, conflicting interface identities, conflicting IPv6 zones and over-budget
reads remain errors. They cannot become evidence of absence. Mutating route
commands, their300ms settlement budget, ownership fences and journal intents
remain in place. Only a successfully parsed complete table can prove an exact
route absent. The native settlement helper retains published intent on unknown
query or delete failure.

Selection distinguishes IPv4/IPv6 exact/1 routes from/0 fallback, scoped defaults
from global routes, and interface indices. Gateways retain IPv6 scope identity;
link-layer gateways cannot satisfy a numeric owned-gateway match. Down, reject
or blackhole scoped defaults return an error instead of being borrowed or treated
as absent. Existing gateway/index checks still fence physical scoped-route cleanup.

The production-entry regression calls native::query_test, a cfg(test)-only thin
wrapper around native::route_query, with real route arguments and a FIFO-gated
external helper. The fixed entry succeeds without starting that helper. A local
counterfactual restored only the frozen9f production query body: the same test
actually started the warmed helper and failed with ROUTE_QUERY_TIMEOUT. The warm
setup avoids mistaking AMFI first-launch delay for reaching the controlled FIFO.
The initial cold-helper negative had helper_started=false and is not accepted as
this controlled negative. All helper fixtures reap children and remove barriers.
